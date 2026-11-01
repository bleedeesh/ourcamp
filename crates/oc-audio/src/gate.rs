//! Decides when the microphone is on the air: by the voice's level or by a held key.
//!
//! A closed gate does not stop the audio, it turns it to silence: the frames keep their
//! pace, so a listener's jitter buffer has nothing to adapt to when the voice comes back.
//! The edges are short fades, so neither end clicks. See
//! docs/decisions/0069-how-the-mic-turns-on.md.

/// Lowest threshold offered, dBFS. A voice is always above it.
pub const MIN_THRESHOLD_DB: f32 = -70.0;
/// Highest threshold offered, dBFS. Above it even a loud voice would not get through.
pub const MAX_THRESHOLD_DB: f32 = -10.0;
/// Threshold until the user chooses one, dBFS: below a quiet voice after the microphone's
/// gain control, above what its noise suppression leaves of a room.
pub const DEFAULT_THRESHOLD_DB: f32 = -50.0;

/// How far below the threshold the level may fall before the gate starts closing, in dB.
const HYSTERESIS_DB: f32 = 5.0;
/// How long the voice stays on the air after falling quiet, in ms: the pause between words.
const VOICE_HOLD_MS: u32 = 400;
/// The same after the key is released, in ms: people let go on the last syllable.
const KEY_HOLD_MS: u32 = 100;
/// Length of the fade in, in ms. Short: it eats the start of a word.
const FADE_IN_MS: u32 = 5;
/// Length of the fade out, in ms.
const FADE_OUT_MS: u32 = 20;

/// What opens the gate now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Opener {
    /// The voice's level reached the threshold.
    Voice {
        /// In dBFS.
        threshold_db: f32,
    },
    /// The key is held.
    Key {
        /// Whether it is held now.
        down: bool,
    },
}

/// The gate of one microphone at one sample rate.
#[derive(Debug, Clone)]
pub struct MicGate {
    open: bool,
    /// Samples left before an open gate closes, once nothing keeps it open.
    hold_left: u32,
    voice_hold: u32,
    key_hold: u32,
    /// Current gain, 0–1, and how much it moves per sample.
    gain: f32,
    fade_in_step: f32,
    fade_out_step: f32,
}

impl MicGate {
    /// A closed gate for audio at `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        let samples = |ms: u32| sample_rate / 1000 * ms;
        Self {
            open: false,
            hold_left: 0,
            voice_hold: samples(VOICE_HOLD_MS),
            key_hold: samples(KEY_HOLD_MS),
            gain: 0.0,
            fade_in_step: 1.0 / samples(FADE_IN_MS).max(1) as f32,
            fade_out_step: 1.0 / samples(FADE_OUT_MS).max(1) as f32,
        }
    }

    /// Takes in a block's level and `block_len`, and says whether the gate is open now.
    pub fn update(&mut self, opener: Opener, level_db: f32, block_len: u32) -> bool {
        let (keeps_open, opens, hold) = match opener {
            Opener::Voice { threshold_db } => (
                level_db >= threshold_db - HYSTERESIS_DB,
                level_db >= threshold_db,
                self.voice_hold,
            ),
            Opener::Key { down } => (down, down, self.key_hold),
        };
        if opens || (self.open && keeps_open) {
            self.open = true;
            self.hold_left = hold;
        } else if self.open {
            // After a change of mode the hold may be longer than this mode's.
            self.hold_left = self.hold_left.min(hold).saturating_sub(block_len);
            self.open = self.hold_left > 0;
        }
        self.open
    }

    /// Whether the gate is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Applies the gate to a block: untouched while open, silence while closed, fades between.
    pub fn apply(&mut self, samples: &mut [i16]) {
        if self.open && self.gain >= 1.0 {
            return;
        }
        if !self.open && self.gain <= 0.0 {
            samples.fill(0);
            return;
        }
        for sample in samples {
            self.gain = if self.open {
                (self.gain + self.fade_in_step).min(1.0)
            } else {
                (self.gain - self.fade_out_step).max(0.0)
            };
            // The result is within the sample's range: the gain is at most 1.
            #[allow(clippy::cast_possible_truncation)]
            {
                *sample = (f32::from(*sample) * self.gain) as i16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    /// 10 ms, the microphone's frame.
    const BLOCK: u32 = 480;
    const VOICE: Opener = Opener::Voice {
        threshold_db: -40.0,
    };

    fn blocks_of(ms: u32) -> u32 {
        ms / 10
    }

    #[test]
    fn starts_closed_and_silent() {
        let mut gate = MicGate::new(RATE);
        let mut block = [1000; 480];
        gate.apply(&mut block);
        assert!(!gate.is_open());
        assert_eq!(block, [0; 480]);
    }

    #[test]
    fn the_voice_opens_it_at_the_threshold_and_not_below() {
        let mut gate = MicGate::new(RATE);
        assert!(!gate.update(VOICE, -40.5, BLOCK));
        assert!(gate.update(VOICE, -40.0, BLOCK));
    }

    #[test]
    fn it_stays_open_for_the_hold_after_the_voice_falls_quiet() {
        let mut gate = MicGate::new(RATE);
        gate.update(VOICE, -20.0, BLOCK);
        for _ in 0..blocks_of(VOICE_HOLD_MS) - 1 {
            assert!(gate.update(VOICE, -90.0, BLOCK));
        }
        assert!(!gate.update(VOICE, -90.0, BLOCK));
    }

    #[test]
    fn the_end_of_a_word_below_the_threshold_keeps_it_open() {
        let mut gate = MicGate::new(RATE);
        gate.update(VOICE, -20.0, BLOCK);
        for _ in 0..1000 {
            assert!(gate.update(VOICE, -44.0, BLOCK), "within the hysteresis");
        }
    }

    #[test]
    fn a_sound_within_the_hysteresis_does_not_open_a_closed_gate() {
        let mut gate = MicGate::new(RATE);
        for _ in 0..100 {
            assert!(!gate.update(VOICE, -44.0, BLOCK));
        }
    }

    #[test]
    fn the_key_opens_it_whatever_the_level() {
        let mut gate = MicGate::new(RATE);
        let up = Opener::Key { down: false };
        let down = Opener::Key { down: true };
        assert!(!gate.update(up, 0.0, BLOCK), "a loud voice without the key");
        assert!(gate.update(down, -100.0, BLOCK));
    }

    #[test]
    fn it_closes_soon_after_the_key_is_released() {
        let mut gate = MicGate::new(RATE);
        let up = Opener::Key { down: false };
        gate.update(Opener::Key { down: true }, -100.0, BLOCK);
        for _ in 0..blocks_of(KEY_HOLD_MS) - 1 {
            assert!(gate.update(up, 0.0, BLOCK), "the level does not hold it");
        }
        assert!(!gate.update(up, 0.0, BLOCK));
    }

    #[test]
    fn switching_to_the_key_does_not_leave_the_long_hold_of_the_voice() {
        let mut gate = MicGate::new(RATE);
        gate.update(VOICE, -20.0, BLOCK);
        let up = Opener::Key { down: false };
        let mut open_blocks = 0;
        while gate.update(up, -20.0, BLOCK) {
            open_blocks += 1;
        }
        assert!(open_blocks < blocks_of(KEY_HOLD_MS));
    }

    #[test]
    fn an_open_gate_leaves_the_samples_alone() {
        let mut gate = MicGate::new(RATE);
        gate.update(VOICE, -20.0, BLOCK);
        gate.apply(&mut [0; 480]); // the fade in
        let mut block = [1234; 480];
        gate.apply(&mut block);
        assert_eq!(block, [1234; 480]);
    }

    #[test]
    fn the_fade_in_rises_without_a_jump() {
        let mut gate = MicGate::new(RATE);
        gate.update(VOICE, -20.0, BLOCK);
        let mut block = [10_000; 480];
        gate.apply(&mut block);
        let step = 10_000 / (RATE / 1000 * FADE_IN_MS) as i16 + 1;
        assert!(block[0] <= step, "starts near zero: {}", block[0]);
        assert!(block
            .windows(2)
            .all(|w| w[1] >= w[0] && w[1] - w[0] <= step));
        assert_eq!(block[479], 10_000);
    }

    #[test]
    fn the_fade_out_falls_without_a_jump_and_ends_in_silence() {
        let mut gate = MicGate::new(RATE);
        gate.update(VOICE, -20.0, BLOCK);
        gate.apply(&mut [0; 480]);
        while gate.update(VOICE, -90.0, BLOCK) {}

        let step = 10_000 / (RATE / 1000 * FADE_OUT_MS) as i16 + 1;
        let mut block = [10_000; 480];
        gate.apply(&mut block);
        assert!(block[0] >= 10_000 - step, "starts near the voice");
        assert!(block
            .windows(2)
            .all(|w| w[1] <= w[0] && w[0] - w[1] <= step));

        let mut last = [10_000; 480];
        gate.apply(&mut last);
        assert_eq!(*last.last().unwrap(), 0, "silent after 20 ms");
        let mut after = [10_000; 480];
        gate.apply(&mut after);
        assert_eq!(after, [0; 480]);
    }

    #[test]
    fn a_rate_that_does_not_divide_evenly_still_works() {
        let mut gate = MicGate::new(44_100);
        gate.update(VOICE, -20.0, 441);
        let mut block = [1000; 441];
        gate.apply(&mut block);
        assert_eq!(block[440], 1000);
    }

    #[test]
    fn the_default_threshold_is_among_those_offered() {
        assert!((MIN_THRESHOLD_DB..=MAX_THRESHOLD_DB).contains(&DEFAULT_THRESHOLD_DB));
    }
}
