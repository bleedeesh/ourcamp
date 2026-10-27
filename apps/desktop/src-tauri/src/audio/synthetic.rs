//! Synthetic voices: a stand-in for remote participants until the WebRTC engine lands
//! (roadmap M2).
//!
//! Each member gets a voice-like tone (fundamental + harmonics, slight vibrato) that
//! talks in random turns. It goes through the exact same path a real decoded track will
//! — `SourceInput` → mixer → device — so per-member volume, local mute, deafen, the
//! limiter and speech detection can be tested by ear today.

use std::f32::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use oc_audio::mixer::SourceInput;

/// Generation tick. Small enough to keep the source buffers shallow.
const TICK: Duration = Duration::from_millis(10);
/// Peak amplitude of a synthetic voice (≈ −18 dBFS: a typical speaking level).
const AMPLITUDE: f32 = 0.125;
/// Attack/release of each talk burst, to avoid clicks.
const FADE_SECONDS: f32 = 0.02;

struct Voice {
    input: SourceInput,
    freq: f32,
    phase: f32,
    vibrato_phase: f32,
    envelope: f32,
    talking: bool,
    samples_left: u32,
    rng: u32,
}

impl Voice {
    fn new(user_id: &str, input: SourceInput) -> Self {
        // FNV-1a of the id: each person keeps the same pitch between sessions.
        let hash = user_id.bytes().fold(0x811c_9dc5_u32, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
        });
        Self {
            input,
            freq: 140.0 + (hash % 120) as f32,
            phase: 0.0,
            vibrato_phase: 0.0,
            envelope: 0.0,
            talking: false,
            samples_left: 0,
            rng: hash | 1,
        }
    }

    fn random(&mut self) -> f32 {
        // xorshift32 — plenty for choosing pause lengths.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f32 / u32::MAX as f32
    }

    fn render(&mut self, out: &mut [f32], rate: f32) {
        let fade_step = 1.0 / (FADE_SECONDS * rate);
        for s in out.iter_mut() {
            if self.samples_left == 0 {
                self.talking = !self.talking;
                let seconds = if self.talking {
                    0.6 + 1.6 * self.random()
                } else {
                    0.5 + 1.3 * self.random()
                };
                self.samples_left = (seconds * rate) as u32;
            }
            self.samples_left -= 1;

            let target = if self.talking { 1.0 } else { 0.0 };
            self.envelope += (target - self.envelope).clamp(-fade_step, fade_step);

            self.vibrato_phase = (self.vibrato_phase + TAU * 5.0 / rate) % TAU;
            let freq = self.freq * (1.0 + 0.02 * self.vibrato_phase.sin());
            self.phase = (self.phase + TAU * freq / rate) % TAU;
            let tone = 0.55 * self.phase.sin()
                + 0.3 * (2.0 * self.phase).sin()
                + 0.15 * (3.0 * self.phase).sin();
            *s = tone * AMPLITUDE * self.envelope;
        }
    }
}

/// Owns the generator thread and the set of synthetic voices.
pub struct SyntheticVoices {
    voices: Arc<Mutex<Vec<Voice>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SyntheticVoices {
    /// Starts the generator for a device running at `sample_rate`.
    ///
    /// # Panics
    /// Panics if the OS refuses to spawn a thread.
    #[must_use]
    pub fn start(sample_rate: u32) -> Self {
        let voices: Arc<Mutex<Vec<Voice>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let voices = Arc::clone(&voices);
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("oc-synthetic-voices".into())
                .spawn(move || generate(&voices, &stop, sample_rate))
                .expect("failed to spawn synthetic voice thread")
        };
        Self {
            voices,
            stop,
            thread: Some(thread),
        }
    }

    /// Starts a voice for `user_id` writing into `input`.
    pub fn add(&self, user_id: &str, input: SourceInput) {
        self.lock().push(Voice::new(user_id, input));
    }

    /// Stops all voices.
    pub fn clear(&self) {
        self.lock().clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Voice>> {
        // A panic while holding the lock cannot leave `Vec<Voice>` inconsistent.
        self.voices.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for SyntheticVoices {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn generate(voices: &Mutex<Vec<Voice>>, stop: &AtomicBool, sample_rate: u32) {
    let rate = sample_rate as f32;
    let max_chunk = sample_rate as usize / 10;
    let mut buf = vec![0.0_f32; max_chunk];
    let start = Instant::now();
    let mut generated: u64 = 0;

    while !stop.load(Ordering::Relaxed) {
        thread::sleep(TICK);
        // Generate exactly as many samples as wall-clock time requires, so the
        // synthetic sources neither drift ahead nor starve the mixer.
        let due = (start.elapsed().as_secs_f64() * f64::from(sample_rate)) as u64;
        let n = ((due - generated) as usize).min(max_chunk);
        generated = due;
        let chunk = &mut buf[..n];
        let mut voices = voices.lock().unwrap_or_else(PoisonError::into_inner);
        for voice in voices.iter_mut() {
            voice.render(chunk, rate);
            voice.input.write(chunk);
        }
        drop(voices);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_audio::mixer::{mixer, Layout, MixerConfig};

    #[test]
    fn voice_alternates_between_talking_and_silence_without_clipping() {
        let (mut control, _core) = mixer::<u8>(MixerConfig::default_48k_stereo());
        let input = control.add_source(1, Layout::Mono, 1.0).unwrap();
        let mut voice = Voice::new("lena", input);
        let mut out = vec![0.0_f32; 48_000 * 6];
        voice.render(&mut out, 48_000.0);
        let loud = out
            .chunks(480)
            .filter(|b| b.iter().any(|s| s.abs() > 0.05))
            .count();
        let silent = out
            .chunks(480)
            .filter(|b| b.iter().all(|s| s.abs() < 1e-4))
            .count();
        assert!(loud > 50 && silent > 50, "loud {loud}, silent {silent}");
        assert!(out.iter().all(|s| s.abs() <= AMPLITUDE + 1e-6));
    }

    #[test]
    fn pitch_is_stable_per_user() {
        let (mut control, _core) = mixer::<u8>(MixerConfig::default_48k_stereo());
        let a = Voice::new("artem", control.add_source(1, Layout::Mono, 1.0).unwrap());
        let b = Voice::new("artem", control.add_source(2, Layout::Mono, 1.0).unwrap());
        assert!((a.freq - b.freq).abs() < f32::EPSILON);
        assert!((140.0..260.0).contains(&a.freq));
    }
}
