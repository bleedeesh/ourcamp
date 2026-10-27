//! Level metering and "is this person talking" detection.
//!
//! The speaking ring around an avatar is driven locally from each incoming track's
//! level instead of waiting for the server to tell us, so it reacts within one audio
//! block. The detector has hysteresis (separate on/off thresholds) and a hold time so
//! the ring does not flicker between syllables.

/// Floor used instead of −∞ for silent blocks.
pub const SILENCE_DB: f32 = -100.0;

/// Root-mean-square level of `samples` in dBFS, floored at [`SILENCE_DB`].
#[must_use]
pub fn rms_dbfs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return SILENCE_DB;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    let rms = (sum_sq / samples.len() as f32).sqrt();
    if rms <= 0.0 || !rms.is_finite() {
        SILENCE_DB
    } else {
        (20.0 * rms.log10()).max(SILENCE_DB)
    }
}

/// Tunables for [`SpeechDetector`].
#[derive(Debug, Clone, Copy)]
pub struct SpeechDetectorConfig {
    /// Level (dBFS) at which a silent participant starts counting as speaking.
    pub on_threshold_db: f32,
    /// Level (dBFS) below which a speaking participant may stop counting as speaking.
    /// Must be ≤ `on_threshold_db`; the gap is the hysteresis.
    pub off_threshold_db: f32,
    /// How long the speaking state is held after the level drops, in samples.
    pub hold_samples: u32,
}

impl SpeechDetectorConfig {
    /// Sensible defaults for voice at `sample_rate`: on at −45 dBFS, off at −50 dBFS,
    /// 250 ms hold.
    #[must_use]
    pub fn voice(sample_rate: u32) -> Self {
        Self {
            on_threshold_db: -45.0,
            off_threshold_db: -50.0,
            hold_samples: sample_rate / 4,
        }
    }
}

/// Hysteresis + hold speech detector fed with per-block levels.
#[derive(Debug, Clone)]
pub struct SpeechDetector {
    config: SpeechDetectorConfig,
    speaking: bool,
    quiet_for: u32,
}

impl SpeechDetector {
    /// Creates a detector in the "not speaking" state.
    #[must_use]
    pub fn new(config: SpeechDetectorConfig) -> Self {
        let mut config = config;
        config.off_threshold_db = config.off_threshold_db.min(config.on_threshold_db);
        Self {
            config,
            speaking: false,
            quiet_for: 0,
        }
    }

    /// Feeds the level of a block of `block_len` samples and returns the new state.
    pub fn update(&mut self, level_db: f32, block_len: u32) -> bool {
        if self.speaking {
            if level_db >= self.config.off_threshold_db {
                self.quiet_for = 0;
            } else {
                self.quiet_for = self.quiet_for.saturating_add(block_len);
                if self.quiet_for >= self.config.hold_samples {
                    self.speaking = false;
                    self.quiet_for = 0;
                }
            }
        } else if level_db >= self.config.on_threshold_db {
            self.speaking = true;
            self.quiet_for = 0;
        }
        self.speaking
    }

    /// Current state.
    #[must_use]
    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    /// Forces the "not speaking" state (e.g. when the participant mutes).
    pub fn reset(&mut self) {
        self.speaking = false;
        self.quiet_for = 0;
    }
}

#[cfg(test)]
// Exact float comparisons are intentional here: the expected values (silence, untouched
// samples) are produced without any arithmetic rounding.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_known_signals() {
        assert_eq!(rms_dbfs(&[]), SILENCE_DB);
        assert_eq!(rms_dbfs(&[0.0; 64]), SILENCE_DB);
        assert!(rms_dbfs(&[1.0; 64]).abs() < 1e-4, "full-scale DC is 0 dBFS");
        let half = rms_dbfs(&[0.5, -0.5, 0.5, -0.5]);
        assert!((half + 6.0206).abs() < 1e-3);
    }

    fn config() -> SpeechDetectorConfig {
        SpeechDetectorConfig {
            on_threshold_db: -40.0,
            off_threshold_db: -50.0,
            hold_samples: 1000,
        }
    }

    #[test]
    fn turns_on_above_on_threshold_only() {
        let mut d = SpeechDetector::new(config());
        assert!(
            !d.update(-45.0, 480),
            "between thresholds does not switch on"
        );
        assert!(d.update(-30.0, 480));
    }

    #[test]
    fn stays_on_in_hysteresis_band() {
        let mut d = SpeechDetector::new(config());
        d.update(-30.0, 480);
        for _ in 0..100 {
            assert!(d.update(-45.0, 480), "between thresholds keeps speaking");
        }
    }

    #[test]
    fn holds_before_turning_off() {
        let mut d = SpeechDetector::new(config());
        d.update(-30.0, 480);
        assert!(d.update(-80.0, 480), "480 < hold");
        assert!(d.update(-80.0, 480), "960 < hold");
        assert!(!d.update(-80.0, 480), "1440 >= hold");
    }

    #[test]
    fn speech_during_hold_restarts_it() {
        let mut d = SpeechDetector::new(config());
        d.update(-30.0, 480);
        d.update(-80.0, 480);
        d.update(-45.0, 480);
        assert!(d.update(-80.0, 480));
        assert!(d.update(-80.0, 480));
        assert!(!d.update(-80.0, 480));
    }

    #[test]
    fn misordered_thresholds_are_corrected() {
        let mut d = SpeechDetector::new(SpeechDetectorConfig {
            on_threshold_db: -50.0,
            off_threshold_db: -40.0,
            hold_samples: 0,
        });
        assert!(d.update(-45.0, 480));
        assert!(
            d.update(-48.0, 480),
            "off threshold clamped to on threshold"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut d = SpeechDetector::new(config());
        d.update(-10.0, 480);
        d.reset();
        assert!(!d.is_speaking());
    }
}
