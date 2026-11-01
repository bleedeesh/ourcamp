//! Short sounds of what happens in a channel (ADR-0075): a dry wooden knock, chosen by ear.
//!
//! They are computed, not recorded: nothing to ship or license, and the same sound at whatever
//! rate the output device runs. Rendered once for each mixer, before it moves to the audio
//! thread.

use std::f64::consts::TAU;

/// One sound. Its value is its place in [`bank`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Cue {
    /// You turned your microphone off.
    MicOff,
    /// You turned your microphone on.
    MicOn,
    /// Someone came into your channel, or you did.
    Joined,
    /// Someone left your channel, or you did.
    Left,
    /// A stream began in your channel.
    StreamStarted,
}

impl Cue {
    /// Every sound, in the order of [`bank`].
    pub const ALL: [Self; 5] = [
        Self::MicOff,
        Self::MicOn,
        Self::Joined,
        Self::Left,
        Self::StreamStarted,
    ];

    /// The knocks: their pitch in semitones, the seconds between them, and how loud.
    /// Rising means on or in, falling means off or out.
    const fn score(self) -> Score {
        match self {
            Self::MicOff => Score {
                steps: &[0.0, -4.0],
                gap: 0.055,
                level: 0.75,
            },
            Self::MicOn => Score {
                steps: &[-4.0, 0.0],
                gap: 0.055,
                level: 0.75,
            },
            Self::Joined => Score {
                steps: &[0.0, 7.0],
                gap: 0.1,
                level: 1.0,
            },
            Self::Left => Score {
                steps: &[7.0, 0.0],
                gap: 0.11,
                level: 0.8,
            },
            Self::StreamStarted => Score {
                steps: &[0.0, 4.0, 7.0, 11.0],
                gap: 0.07,
                level: 1.0,
            },
        }
    }
}

struct Score {
    steps: &'static [f64],
    gap: f64,
    level: f64,
}

/// The lowest note, Hz.
const ROOT: f64 = 311.13;
/// The loudest a sound gets at level 1: under a voice, not over it.
const PEAK_DB: f64 = -12.0;
/// How much room is added to the knock.
const ROOM_MIX: f64 = 0.24;
/// The top is taken off above this, Hz.
const CUTOFF: f64 = 4500.0;
/// What rings after the last knock, seconds. It closes to true zero: no click at the end.
const TAIL: f64 = 0.35;
const FADE: f64 = 0.05;
/// Echoes of the room on each side, ms. They differ, which is what gives the width.
const ROOM: [[f64; 4]; 2] = [[23.1, 29.7, 37.3, 41.9], [25.3, 31.1, 35.9, 44.3]];

/// Every sound in [`Cue::ALL`] order, interleaved stereo at `rate`.
#[must_use]
pub fn bank(rate: u32) -> Vec<Vec<f32>> {
    Cue::ALL.iter().map(|cue| render(*cue, rate)).collect()
}

/// One knock at `f` Hz, `t` seconds after it struck: the hollow of the wood, the sharper shell
/// that dies sooner, and a tick of noise from the hit itself.
fn knock(f: f64, t: f64, noise: &mut Noise) -> f64 {
    let hollow = (TAU * f * t).sin() * (-t / 0.032).exp();
    let shell = (TAU * f * 2.42 * t).sin() * 0.45 * (-t / 0.014).exp();
    let tick = if t < 0.0015 { noise.next() * 0.25 } else { 0.0 };
    hollow + shell + tick
}

/// Seeded: the same sound on every run.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(self.0) / 2_147_483_648.0 - 1.0
    }
}

/// A small dark room: parallel echoes, each duller and quieter every time around.
fn room(input: &[f64], delays_ms: &[f64; 4], rate: f64) -> Vec<f64> {
    let mut out = vec![0.0; input.len()];
    // 0.35 per sample at 48 kHz, the same darkness at any other rate.
    let damping = 1.0 - 0.65_f64.powf(48_000.0 / rate);
    for ms in delays_ms {
        let feedback = 10_f64.powf(-3.0 * ms / 200.0);
        let mut line = vec![0.0; ((ms * rate / 1000.0).round() as usize).max(1)];
        let mut at = 0;
        let mut damp = 0.0;
        for (o, x) in out.iter_mut().zip(input) {
            let y = line[at];
            damp += damping * (y - damp);
            line[at] = x + damp * feedback;
            at = (at + 1) % line.len();
            *o += y / delays_ms.len() as f64;
        }
    }
    out
}

fn render(cue: Cue, rate: u32) -> Vec<f32> {
    let score = cue.score();
    let rate = f64::from(rate);
    let knocks = score.steps.len() - 1;
    let length = ((score.gap * knocks as f64 + TAIL) * rate).ceil() as usize;

    let mut dry = vec![0.0; length];
    let mut noise = Noise(12_345);
    for (index, step) in score.steps.iter().enumerate() {
        let f = ROOT * 2_f64.powf(step / 12.0);
        let start = (index as f64 * score.gap * rate).round() as usize;
        for (i, sample) in dry[start..].iter_mut().enumerate() {
            let t = i as f64 / rate;
            *sample += knock(f, t, &mut noise) * (t / 0.0005).min(1.0);
        }
    }
    let lowpass = 1.0 - (-TAU * CUTOFF / rate).exp();
    let mut low = 0.0;
    for sample in &mut dry {
        low += lowpass * (*sample - low);
        *sample = low;
    }

    let fade = FADE * rate;
    let target = 10_f64.powf(PEAK_DB / 20.0) * score.level;
    let mut stereo = vec![0.0; length * 2];
    for (side, delays) in ROOM.iter().enumerate() {
        let wet = room(&dry, delays, rate);
        let mixed: Vec<f64> = dry
            .iter()
            .zip(&wet)
            .map(|(d, w)| d + ROOM_MIX * w)
            .collect();
        // Each side reaches the same peak: the room never leans a sound to one ear.
        let peak = mixed.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
        let gain = target / peak;
        for (i, v) in mixed.iter().enumerate() {
            let closing = ((length - 1 - i) as f64 / fade).min(1.0);
            stereo[i * 2 + side] = (v * gain * closing) as f32;
        }
    }
    stereo
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak_db(samples: &[f32]) -> f32 {
        let peak = samples.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
        20.0 * peak.log10()
    }

    #[test]
    fn the_bank_holds_every_sound_in_its_place() {
        let bank = bank(48_000);
        assert_eq!(bank.len(), Cue::ALL.len());
        for (index, cue) in Cue::ALL.iter().enumerate() {
            assert_eq!(*cue as usize, index);
            assert_eq!(bank[index], render(*cue, 48_000));
        }
    }

    #[test]
    fn every_sound_is_short_and_stays_under_a_voice() {
        for cue in Cue::ALL {
            let sound = render(cue, 48_000);
            let seconds = sound.len() as f32 / 2.0 / 48_000.0;
            assert!(seconds < 0.6, "{cue:?} lasts {seconds} s");
            let want = -12.0 + 20.0 * (cue.score().level as f32).log10();
            let peak = peak_db(&sound);
            assert!((peak - want).abs() < 0.1, "{cue:?} peaks at {peak} dB");
        }
    }

    #[test]
    fn a_sound_starts_and_ends_without_a_click() {
        for cue in Cue::ALL {
            let sound = render(cue, 48_000);
            assert!(sound[0].abs() < 1e-3, "{cue:?} starts at {}", sound[0]);
            assert_eq!(sound[sound.len() - 1], 0.0, "{cue:?}");
            assert_eq!(sound[sound.len() - 2], 0.0, "{cue:?}");
        }
    }

    #[test]
    fn both_ears_get_the_same_peak_but_not_the_same_sound() {
        for cue in Cue::ALL {
            let sound = render(cue, 48_000);
            let (left, right): (Vec<f32>, Vec<f32>) = sound
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[l, r]| (l, r))
                .unzip();
            assert!((peak_db(&left) - peak_db(&right)).abs() < 0.01, "{cue:?}");
            assert_ne!(left, right, "{cue:?}");
        }
    }

    #[test]
    fn the_sounds_can_be_told_apart() {
        let bank = bank(48_000);
        for (i, a) in bank.iter().enumerate() {
            for b in &bank[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn another_rate_gives_the_same_length_and_level() {
        for cue in Cue::ALL {
            let at_48 = render(cue, 48_000);
            let at_44 = render(cue, 44_100);
            let ratio = at_44.len() as f32 / at_48.len() as f32;
            assert!(
                (ratio - 44_100.0 / 48_000.0).abs() < 0.01,
                "{cue:?}: {ratio}"
            );
            assert!((peak_db(&at_44) - peak_db(&at_48)).abs() < 0.2, "{cue:?}");
        }
    }

    #[test]
    fn the_sound_is_the_same_on_every_run() {
        assert_eq!(render(Cue::Joined, 48_000), render(Cue::Joined, 48_000));
    }
}
