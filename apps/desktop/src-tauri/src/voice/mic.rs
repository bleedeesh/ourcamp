//! Microphone pipeline: capture → APM (AEC3 + noise suppression + AGC) → LiveKit.
//!
//! One thread owns the capture stream and the APM. Every 10 ms frame it first feeds
//! the APM everything the speakers played since the last frame (the mixer's
//! [`OutputTap`]), then processes the microphone frame and hands it to the
//! [`NativeAudioSource`] of the published track. See
//! docs/decisions/0009-mic-processing-webrtc-apm.md.
//!
//! ```text
//! cpal input ──ring──► oc-mic thread: APM.process_stream ─► NativeAudioSource ─► Opus
//! mixer output ─tap──►               APM.process_reverse_stream (echo reference)
//! ```

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::AudioSourceOptions;
use livekit::webrtc::native::apm::AudioProcessingModule;
use oc_audio::gate::{MicGate, Opener, DEFAULT_THRESHOLD_DB, MAX_THRESHOLD_DB, MIN_THRESHOLD_DB};
use oc_audio::meter::{rms_dbfs, SpeechDetector, SpeechDetectorConfig, SILENCE_DB};
use oc_audio::mixer::OutputTap;
use oc_audio::pcm::{f32_to_i16, i16_to_f32};

use crate::audio::devices::{talk_key_down, Alarm};
use crate::audio::input;
use crate::audio::output::DeviceLatency;
use crate::voice::denoise::{self, Denoiser};

/// Upper bound on how long the thread sleeps if the driver stops calling back.
const WAKE_INTERVAL: Duration = Duration::from_millis(20);
/// If capture piles up beyond this many 10 ms frames (thread starved), older audio is
/// dropped: talking 200 ms late is worse than losing a few milliseconds once.
const MAX_CAPTURE_BACKLOG_FRAMES: usize = 8;
/// Same for the echo reference; AEC3 copes with gaps far better than with stale input.
const MAX_REFERENCE_BACKLOG_FRAMES: usize = 10;

/// Live level of the local voice after processing, for the user's own speaking ring.
#[derive(Debug, Default)]
pub struct LocalLevel {
    level_db: AtomicU32,
    speaking: AtomicBool,
    input_db: AtomicU32,
    open: AtomicBool,
}

impl LocalLevel {
    /// Level in dBFS and speech detector state of the last frame sent.
    #[must_use]
    pub fn get(&self) -> (f32, bool) {
        (
            f32::from_bits(self.level_db.load(Ordering::Relaxed)),
            self.speaking.load(Ordering::Relaxed),
        )
    }

    /// What the microphone hears in dBFS, gate or no gate, and whether the gate is open:
    /// the settings' bar next to the threshold.
    #[must_use]
    pub fn input(&self) -> (f32, bool) {
        (
            f32::from_bits(self.input_db.load(Ordering::Relaxed)),
            self.open.load(Ordering::Relaxed),
        )
    }

    fn set(&self, level_db: f32, speaking: bool) {
        self.level_db.store(level_db.to_bits(), Ordering::Relaxed);
        self.speaking.store(speaking, Ordering::Relaxed);
    }

    fn set_input(&self, input_db: f32, open: bool) {
        self.input_db.store(input_db.to_bits(), Ordering::Relaxed);
        self.open.store(open, Ordering::Relaxed);
    }
}

/// What the processing does to the microphone, as the settings have it (ADR-0072).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Processing {
    noise_suppression: bool,
    echo_cancellation: bool,
}

/// Echo cancellation → noise suppression → gain control, in that order (ADR-0008). The suppressor
/// stands between two modules: the SDK offers no way into the middle of one. The SDK also chooses
/// a module's settings only when creating it, so new settings mean a new chain.
struct Chain {
    processing: Processing,
    /// The echo cancellation and the high pass filter, which always works (ADR-0009). Where the
    /// model cannot work, WebRTC's suppressor too: there is only ever one (ADR-0008).
    first: AudioProcessingModule,
    denoiser: Option<Denoiser>,
    gain: AudioProcessingModule,
    rate: i32,
    scratch: Vec<f32>,
}

impl Chain {
    fn new(processing: Processing, rate: u32) -> Self {
        let denoiser = (processing.noise_suppression && rate == denoise::RATE)
            .then(Denoiser::new)
            .and_then(|made| {
                made.map_err(|e| eprintln!("[oc-voice] suppressor: {e}"))
                    .ok()
            });
        let webrtc_suppressor = processing.noise_suppression && denoiser.is_none();
        Self {
            processing,
            first: AudioProcessingModule::new(
                processing.echo_cancellation,
                false,
                true,
                webrtc_suppressor,
            ),
            denoiser,
            gain: AudioProcessingModule::new(false, true, false, false),
            rate: i32::try_from(rate).unwrap_or(i32::MAX),
            scratch: vec![0.0; (rate / 100) as usize],
        }
    }

    /// What the loudspeakers just played. Without the echo cancellation it has no use.
    fn reference(&mut self, reference: &mut Frame) -> Result<(), String> {
        if !self.processing.echo_cancellation {
            return Ok(());
        }
        self.first
            .process_reverse_stream(&mut reference.i16, reference.rate_i32, 1)
            .map_err(|e| e.message)
    }

    /// Everything but the gain control, which would raise what is left and hide the rest.
    fn clean(&mut self, mic: &mut Frame, delay_ms: i32) -> Result<(), String> {
        let _ = self.first.set_stream_delay_ms(delay_ms);
        self.first
            .process_stream(&mut mic.i16, self.rate, 1)
            .map_err(|e| e.message)?;
        if let Some(denoiser) = self.denoiser.as_mut() {
            i16_to_f32(&mic.i16, &mut mic.f32);
            denoiser.process(&mut mic.f32, &mut self.scratch)?;
            f32_to_i16(&mic.f32, &mut mic.i16);
        }
        Ok(())
    }

    fn process(&mut self, mic: &mut Frame, delay_ms: i32) -> Result<(), String> {
        self.clean(mic, delay_ms)?;
        self.gain
            .process_stream(&mut mic.i16, self.rate, 1)
            .map_err(|e| e.message)
    }
}

/// How the microphone is set, from the settings and the key. One for the engine's life: every
/// microphone it opens reads it. Lock-free: the audio thread reads it each frame.
#[derive(Debug)]
pub struct MicControl {
    noise_suppression: AtomicBool,
    echo_cancellation: AtomicBool,
    key_mode: AtomicBool,
    /// The talk key's code; 0 is none, no key has it.
    talk_key: AtomicU16,
    threshold_db: AtomicU32,
}

impl Default for MicControl {
    fn default() -> Self {
        Self {
            noise_suppression: AtomicBool::new(true),
            echo_cancellation: AtomicBool::new(true),
            key_mode: AtomicBool::new(false),
            talk_key: AtomicU16::new(0),
            threshold_db: AtomicU32::new(DEFAULT_THRESHOLD_DB.to_bits()),
        }
    }
}

impl MicControl {
    pub fn set_processing(&self, noise_suppression: bool, echo_cancellation: bool) {
        self.noise_suppression
            .store(noise_suppression, Ordering::Relaxed);
        self.echo_cancellation
            .store(echo_cancellation, Ordering::Relaxed);
    }

    fn processing(&self) -> Processing {
        Processing {
            noise_suppression: self.noise_suppression.load(Ordering::Relaxed),
            echo_cancellation: self.echo_cancellation.load(Ordering::Relaxed),
        }
    }

    pub fn set_key_mode(&self, key_mode: bool) {
        self.key_mode.store(key_mode, Ordering::Relaxed);
    }

    pub fn set_talk_key(&self, code: Option<u16>) {
        self.talk_key.store(code.unwrap_or(0), Ordering::Relaxed);
    }

    /// The threshold, held to what is offered: it may come from an edited file.
    pub fn set_threshold(&self, db: f32) {
        // `clamp` keeps NaN, and NaN would keep the gate shut for good.
        let db = if db.is_nan() {
            DEFAULT_THRESHOLD_DB
        } else {
            db.clamp(MIN_THRESHOLD_DB, MAX_THRESHOLD_DB)
        };
        self.threshold_db.store(db.to_bits(), Ordering::Relaxed);
    }

    /// What opens the gate now; `is_down` tells whether a key is held.
    fn opener(&self, is_down: impl Fn(u16) -> bool) -> Opener {
        if self.key_mode.load(Ordering::Relaxed) {
            let key = self.talk_key.load(Ordering::Relaxed);
            Opener::Key {
                down: key != 0 && is_down(key),
            }
        } else {
            Opener::Voice {
                threshold_db: f32::from_bits(self.threshold_db.load(Ordering::Relaxed)),
            }
        }
    }
}

/// Home of the output tap while no pipeline uses it.
///
/// A pipeline borrows the tap through a [`TapLease`] whose `Drop` puts it back, so it
/// returns however the pipeline ends — stopped, failed to open the device, cancelled
/// mid-join, or panicked (`Drop` runs during unwinding). Without this, one failure would
/// leave the engine without an echo reference until restart.
pub type TapSlot = Arc<Mutex<Option<OutputTap>>>;

struct TapLease {
    slot: TapSlot,
    tap: Option<OutputTap>,
}

impl TapLease {
    fn take(slot: &TapSlot) -> Option<Self> {
        let tap = lock_slot(slot).take()?;
        Some(Self {
            slot: Arc::clone(slot),
            tap: Some(tap),
        })
    }

    fn tap(&mut self) -> &mut OutputTap {
        self.tap
            .as_mut()
            .expect("tap is present until the lease drops")
    }

    /// Moves to the tap of a new output, if the engine has left one in the slot.
    fn renew(&mut self) -> bool {
        match lock_slot(&self.slot).take() {
            Some(tap) => {
                self.tap = Some(tap);
                true
            }
            None => false,
        }
    }
}

impl Drop for TapLease {
    fn drop(&mut self) {
        if let Some(tap) = self.tap.take() {
            // A tap in the slot is newer than this one.
            lock_slot(&self.slot).get_or_insert(tap);
        }
    }
}

/// Leaves the tap of a new output: the running pipeline moves to it, or the next one takes it.
pub fn install_tap(slot: &TapSlot, tap: OutputTap) {
    *lock_slot(slot) = Some(tap);
}

fn lock_slot(slot: &TapSlot) -> MutexGuard<'_, Option<OutputTap>> {
    // An `Option` cannot be left half-updated, so a poisoned lock is still usable.
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running microphone pipeline. Stopping or dropping it closes the device and returns
/// the output tap to its [`TapSlot`].
pub struct MicPipeline {
    source: NativeAudioSource,
    device_name: String,
    muted: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    level: Arc<LocalLevel>,
    thread: Option<JoinHandle<()>>,
}

struct Shared {
    muted: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    level: Arc<LocalLevel>,
    control: Arc<MicControl>,
}

impl MicPipeline {
    /// Opens the named microphone (the system's if there is no such device now) and starts
    /// processing, borrowing the tap in `taps`.
    ///
    /// # Errors
    /// If the tap is in use by another pipeline or the microphone cannot be opened.
    pub fn start(
        device: Option<String>,
        taps: &TapSlot,
        output_latency: DeviceLatency,
        muted: bool,
        control: Arc<MicControl>,
        alarm: Arc<Alarm>,
    ) -> Result<Self, String> {
        let lease = TapLease::take(taps).ok_or("микрофон уже используется другим каналом")?;
        let shared = Shared {
            muted: Arc::new(AtomicBool::new(muted)),
            stop: Arc::new(AtomicBool::new(false)),
            level: Arc::new(LocalLevel::default()),
            control,
        };
        let (muted, stop, level) = (
            Arc::clone(&shared.muted),
            Arc::clone(&shared.stop),
            Arc::clone(&shared.level),
        );
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Started, String>>();
        let thread = thread::Builder::new()
            .name("oc-mic".into())
            .spawn(move || {
                run(
                    device.as_deref(),
                    lease,
                    &output_latency,
                    &shared,
                    &alarm,
                    &ready_tx,
                );
            })
            .map_err(|e| format!("не удалось запустить поток микрофона: {e}"))?;
        match ready_rx.recv() {
            Ok(Ok(started)) => Ok(Self {
                source: started.source,
                device_name: started.device_name,
                muted,
                stop,
                level,
                thread: Some(thread),
            }),
            Ok(Err(message)) => {
                join(thread);
                Err(message)
            }
            // The thread ended without answering: it panicked (logged by `join`).
            Err(_) => {
                join(thread);
                Err("поток микрофона аварийно завершился".into())
            }
        }
    }

    /// Source of the published microphone track.
    #[must_use]
    pub fn source(&self) -> NativeAudioSource {
        self.source.clone()
    }

    /// The device it opened.
    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Stops sending audio (processing keeps running so the echo canceller stays
    /// converged and unmuting is instant).
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    /// Level of the local voice.
    #[must_use]
    pub fn level(&self) -> Arc<LocalLevel> {
        Arc::clone(&self.level)
    }

    /// Closes the microphone; the tap is back in its slot when this returns.
    pub fn stop(mut self) {
        self.halt();
    }

    /// The same for a pipeline still in its place: a new one needs the tap before it can
    /// replace this one. Nothing more comes out of it.
    pub fn halt(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.stop.store(true, Ordering::Relaxed);
            thread.thread().unpark();
            join(thread);
        }
    }
}

impl Drop for MicPipeline {
    fn drop(&mut self) {
        self.halt();
    }
}

/// What a pipeline's thread answers when it is up.
struct Started {
    source: NativeAudioSource,
    device_name: String,
}

/// Waits for the pipeline thread. A panic there is a bug: it is logged loudly, but the
/// caller (a UI command) carries on — the lease has already returned the tap.
fn join(thread: JoinHandle<()>) {
    if let Err(panic) = thread.join() {
        let message = panic
            .downcast_ref::<&str>()
            .map(ToString::to_string)
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        eprintln!("[oc-voice] BUG: microphone thread panicked: {message}");
    }
}

/// Buffers for one direction of 10 ms frames.
struct Frame {
    f32: Vec<f32>,
    i16: Vec<i16>,
    rate: u32,
    /// The same rate as libwebrtc's C API takes it.
    rate_i32: i32,
}

impl Frame {
    fn new(rate: u32) -> Self {
        let len = (rate / 100) as usize;
        Self {
            f32: vec![0.0; len],
            i16: vec![0; len],
            rate,
            rate_i32: i32::try_from(rate).expect("sample rate fits in i32"),
        }
    }

    fn len(&self) -> usize {
        self.f32.len()
    }
}

fn run(
    device: Option<&str>,
    mut lease: TapLease,
    output_latency: &DeviceLatency,
    shared: &Shared,
    alarm: &Arc<Alarm>,
    ready: &mpsc::Sender<Result<Started, String>>,
) {
    let mut capture = match input::open(device, thread::current(), alarm) {
        Ok(capture) => capture,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    eprintln!(
        "[oc-voice] microphone: {} @ {} Hz",
        capture.device_name, capture.sample_rate
    );

    // Processing is ours (APM below); the source must not run its own.
    let options = AudioSourceOptions {
        echo_cancellation: false,
        noise_suppression: false,
        auto_gain_control: false,
    };
    // Queue 0: frames go straight to the encoder, no extra buffering latency.
    let source = NativeAudioSource::new(options, capture.sample_rate, 1, 0);
    let _ = ready.send(Ok(Started {
        source: source.clone(),
        device_name: capture.device_name.clone(),
    }));

    let mut chain = Chain::new(shared.control.processing(), capture.sample_rate);
    let mut mic = Frame::new(capture.sample_rate);
    // An output rate that does not give whole 10 ms frames (very rare) means no echo
    // reference; everything else still works.
    let mut reference = reference_frame(lease.tap());
    let mut detector = SpeechDetector::new(SpeechDetectorConfig::voice(capture.sample_rate));
    let mut gate = MicGate::new(capture.sample_rate);
    let mut reported_error = false;
    let mut report = |what: &str, e: &dyn std::fmt::Display| {
        if !reported_error {
            eprintln!("[oc-voice] {what}: {e}");
            reported_error = true;
        }
    };

    while !shared.stop.load(Ordering::Relaxed) {
        thread::park_timeout(WAKE_INTERVAL);

        if lease.renew() {
            reference = reference_frame(lease.tap());
        }
        let tap = lease.tap();

        let backlog = capture.samples.available();
        if backlog > mic.len() * MAX_CAPTURE_BACKLOG_FRAMES {
            capture.samples.skip(backlog - mic.len());
        }

        while capture.samples.available() >= mic.len() {
            // A switch in the settings: a new chain starts its adaptation over, and that is
            // heard for a moment only when someone flips it.
            let wanted = shared.control.processing();
            if wanted != chain.processing {
                chain = Chain::new(wanted, capture.sample_rate);
            }

            if let Some(reference) = reference.as_mut() {
                tap.discard_older_than(reference.len() * MAX_REFERENCE_BACKLOG_FRAMES);
                while tap.available() >= reference.len() {
                    tap.read(&mut reference.f32);
                    f32_to_i16(&reference.f32, &mut reference.i16);
                    if let Err(e) = chain.reference(reference) {
                        report("echo reference", &e);
                    }
                }
            }

            capture.samples.pop_slice(&mut mic.f32);
            f32_to_i16(&mic.f32, &mut mic.i16);
            let delay = output_latency.ms() + capture.latency.ms();
            let delay = i32::try_from(delay).unwrap_or(i32::MAX);
            if let Err(e) = chain.process(&mut mic, delay) {
                report("processing", &e);
            }

            // Before the mute: the settings' bar shows the voice with the microphone off too.
            i16_to_f32(&mic.i16, &mut mic.f32);
            let input = rms_dbfs(&mic.f32);
            let opener = shared.control.opener(talk_key_down);
            let open = gate.update(opener, input, mic.len() as u32);
            shared.level.set_input(input, open);

            if shared.muted.load(Ordering::Relaxed) {
                detector.reset();
                shared.level.set(SILENCE_DB, false);
                continue;
            }
            // Others' rings go by what reaches them, so yours does too.
            let level = if open { input } else { SILENCE_DB };
            let speaking = detector.update(level, mic.len() as u32);
            shared.level.set(level, speaking);
            gate.apply(&mut mic.i16);

            let frame = AudioFrame {
                data: Cow::Borrowed(&mic.i16),
                sample_rate: mic.rate,
                num_channels: 1,
                samples_per_channel: mic.len() as u32,
            };
            // With queue 0 this completes synchronously; no runtime is needed to drive it.
            if let Err(e) = tauri::async_runtime::block_on(source.capture_frame(&frame)) {
                report("send", &e.message);
            }
        }
    }
    drop(capture);
    // `lease` drops here and puts the tap back.
}

/// A frame to read `tap` into, from now on: whatever was played before is irrelevant to this
/// microphone. `None` means no echo reference, and everything else still works.
fn reference_frame(tap: &mut OutputTap) -> Option<Frame> {
    let rate = tap.sample_rate();
    tap.discard_older_than(0);
    let usable = rate.is_multiple_of(100);
    if !usable {
        eprintln!("[oc-voice] output rate {rate} Hz: echo cancellation disabled");
    }
    usable.then(|| Frame::new(rate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_audio::mixer::{mixer, MixerConfig};

    /// Ten seconds of the chain's work on noise, in 10 ms frames; the level of the last second,
    /// dBFS. `echo`: the noise is what a loudspeaker plays, and the microphone hears it 50 ms later.
    fn level_after(processing: Processing, rate: u32, echo: bool) -> f32 {
        const ECHO_FRAMES: usize = 5;
        let mut chain = Chain::new(processing, rate);
        let mut noise = 0x1234_5678_u32;
        let mut played = vec![vec![0_i16; (rate / 100) as usize]; ECHO_FRAMES + 1];
        let mut mic = Frame::new(rate);
        let mut reference = Frame::new(rate);
        let mut out = Vec::new();
        for index in 0..1000 {
            let newest = index % played.len();
            for sample in &mut played[newest] {
                // A fixed sequence, so that the test is the same each time.
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                *sample = i16::try_from(i32::try_from(noise >> 16).unwrap() - 32_768).unwrap() / 8;
            }
            mic.i16.copy_from_slice(&played[newest]);
            if echo {
                reference.i16.copy_from_slice(&played[newest]);
                chain.reference(&mut reference).unwrap();
                mic.i16.copy_from_slice(&played[(index + 1) % played.len()]);
            }
            chain.clean(&mut mic, 50).unwrap();
            if index >= 900 {
                out.extend(mic.i16.iter().map(|&s| f32::from(s) / 32_768.0));
            }
        }
        rms_dbfs(&out)
    }

    #[test]
    fn the_noise_switch_reaches_the_model() {
        let with = |noise_suppression| Processing {
            noise_suppression,
            echo_cancellation: false,
        };
        let on = level_after(with(true), denoise::RATE, false);
        let off = level_after(with(false), denoise::RATE, false);
        assert!(on < off - 20.0, "with {on} dB, without {off} dB");
    }

    #[test]
    fn where_the_model_cannot_work_webrtc_suppresses() {
        let with = |noise_suppression| Processing {
            noise_suppression,
            echo_cancellation: false,
        };
        assert!(Chain::new(with(true), 16_000).denoiser.is_none());
        assert!(Chain::new(with(true), denoise::RATE).denoiser.is_some());
        assert!(Chain::new(with(false), denoise::RATE).denoiser.is_none());
        let on = level_after(with(true), 16_000, false);
        let off = level_after(with(false), 16_000, false);
        assert!(on < off - 6.0, "with {on} dB, without {off} dB");
    }

    #[test]
    fn the_echo_switch_reaches_the_module() {
        let with = |echo_cancellation| Processing {
            noise_suppression: false,
            echo_cancellation,
        };
        let on = level_after(with(true), denoise::RATE, true);
        let off = level_after(with(false), denoise::RATE, true);
        assert!(on < off - 6.0, "with {on} dB, without {off} dB");
    }

    #[test]
    fn everything_is_on_until_the_settings_say_otherwise() {
        let control = MicControl::default();
        assert_eq!(
            control.processing(),
            Processing {
                noise_suppression: true,
                echo_cancellation: true,
            }
        );
        control.set_processing(false, true);
        assert!(!control.processing().noise_suppression);
        assert!(control.processing().echo_cancellation);
    }

    fn slot() -> TapSlot {
        let (_control, mut core) = mixer::<u8>(MixerConfig::default_48k_stereo());
        Arc::new(Mutex::new(Some(core.tap_output(10))))
    }

    #[test]
    fn the_control_says_what_opens_the_gate() {
        let control = MicControl::default();
        control.set_threshold(-30.0);
        assert_eq!(
            control.opener(|_| true),
            Opener::Voice {
                threshold_db: -30.0
            }
        );

        control.set_key_mode(true);
        assert_eq!(
            control.opener(|_| true),
            Opener::Key { down: false },
            "no key is assigned, and nothing opens it"
        );
        control.set_talk_key(Some(0x56));
        assert_eq!(control.opener(|_| false), Opener::Key { down: false });
        assert_eq!(
            control.opener(|key| key == 0x56),
            Opener::Key { down: true }
        );
        assert_eq!(
            control.opener(|key| key == 0x57),
            Opener::Key { down: false },
            "another key"
        );
    }

    #[test]
    fn a_threshold_from_an_edited_file_stays_among_those_offered() {
        let threshold = |db: f32| {
            let control = MicControl::default();
            control.set_threshold(db);
            control.opener(|_| false)
        };
        for (given, kept) in [
            (0.0, MAX_THRESHOLD_DB),
            (-200.0, MIN_THRESHOLD_DB),
            (f32::NAN, DEFAULT_THRESHOLD_DB),
        ] {
            assert_eq!(threshold(given), Opener::Voice { threshold_db: kept });
        }
    }

    #[test]
    fn lease_is_exclusive_and_returns_the_tap_on_drop() {
        let slot = slot();
        let lease = TapLease::take(&slot).expect("tap available");
        assert!(lock_slot(&slot).is_none(), "borrowed while leased");
        assert!(
            TapLease::take(&slot).is_none(),
            "a second pipeline cannot take it"
        );
        drop(lease);
        assert!(lock_slot(&slot).is_some(), "back after the lease ends");
    }

    #[test]
    fn a_lease_moves_to_the_tap_of_a_new_output() {
        let slot = slot();
        let mut lease = TapLease::take(&slot).unwrap();
        assert!(!lease.renew(), "nothing new yet");

        let (_control, mut core) = mixer::<u8>(MixerConfig {
            sample_rate: 44_100,
            ..MixerConfig::default_48k_stereo()
        });
        install_tap(&slot, core.tap_output(10));
        assert!(lease.renew());
        assert_eq!(lease.tap().sample_rate(), 44_100);
        drop(lease);
        assert_eq!(
            lock_slot(&slot).as_ref().map(OutputTap::sample_rate),
            Some(44_100),
            "back in the slot goes the new one"
        );
    }

    #[test]
    fn a_tap_left_for_the_next_pipeline_is_not_overwritten_by_the_old_one() {
        let slot = slot();
        let lease = TapLease::take(&slot).unwrap();
        let (_control, mut core) = mixer::<u8>(MixerConfig {
            sample_rate: 44_100,
            ..MixerConfig::default_48k_stereo()
        });
        install_tap(&slot, core.tap_output(10));
        drop(lease);
        assert_eq!(
            lock_slot(&slot).as_ref().map(OutputTap::sample_rate),
            Some(44_100)
        );
    }

    #[test]
    fn tap_survives_a_panicking_pipeline_thread() {
        let slot = slot();
        let lease = TapLease::take(&slot).unwrap();
        let thread = thread::spawn(move || {
            let _lease = lease;
            panic!("simulated pipeline bug");
        });
        join(thread); // logs, does not propagate
        assert!(lock_slot(&slot).is_some(), "unwinding returned the tap");
        assert!(
            TapLease::take(&slot).is_some(),
            "the next channel can use it"
        );
    }
}
