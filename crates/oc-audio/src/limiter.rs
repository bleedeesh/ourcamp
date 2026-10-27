//! Output peak limiter.
//!
//! Local volumes go up to 200 % and several people can talk at once, so the summed
//! signal can exceed full scale. Hard clipping sounds like crackling; the limiter
//! instead lowers the gain just enough to keep every sample under the ceiling and
//! then lets it recover smoothly.
//!
//! Design: stereo-linked, zero-lookahead, instant attack, exponential release.
//! Instant attack means the ceiling is a hard guarantee — the gain for a sample is
//! computed from that same sample before it is applied — at the cost of a little
//! distortion on extreme transients, which is the right trade for a voice chat.

/// Brick-wall peak limiter operating on interleaved frames.
#[derive(Debug, Clone)]
pub struct Limiter {
    ceiling: f32,
    gain: f32,
    release_coeff: f32,
}

impl Limiter {
    /// Creates a limiter.
    ///
    /// * `ceiling_db` — maximum output peak in dBFS (e.g. `-1.0`).
    /// * `release_ms` — time constant for the gain to recover after a peak.
    #[must_use]
    pub fn new(ceiling_db: f32, release_ms: f32, sample_rate: u32) -> Self {
        let release_samples = (release_ms.max(0.0) / 1000.0) * sample_rate as f32;
        let release_coeff = if release_samples <= 0.0 {
            1.0
        } else {
            1.0 - (-1.0 / release_samples).exp()
        };
        Self {
            ceiling: db_to_linear(ceiling_db.min(0.0)),
            gain: 1.0,
            release_coeff,
        }
    }

    /// Current gain reduction in dB (0 when the limiter is idle). Useful for metering.
    #[must_use]
    pub fn gain_reduction_db(&self) -> f32 {
        -linear_to_db(self.gain)
    }

    /// Limits `buf` in place. `buf` holds interleaved frames of `channels` samples.
    ///
    /// Non-finite samples (NaN/inf from a broken decoder) are replaced with silence so
    /// one bad packet cannot blow up the whole output.
    ///
    /// # Panics
    /// Panics if `channels` is zero.
    pub fn process_interleaved(&mut self, buf: &mut [f32], channels: usize) {
        assert!(channels > 0, "channel count must be non-zero");
        for frame in buf.chunks_mut(channels) {
            let mut peak = 0.0_f32;
            for s in frame.iter_mut() {
                if !s.is_finite() {
                    *s = 0.0;
                }
                peak = peak.max(s.abs());
            }
            let desired = if peak > self.ceiling {
                self.ceiling / peak
            } else {
                1.0
            };
            if desired < self.gain {
                self.gain = desired;
            } else {
                self.gain += (desired - self.gain) * self.release_coeff;
            }
            for s in frame.iter_mut() {
                *s *= self.gain;
            }
        }
    }
}

/// Converts decibels to a linear amplitude factor.
#[must_use]
pub fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Converts a linear amplitude factor to decibels (−∞ for zero).
#[must_use]
pub fn linear_to_db(linear: f32) -> f32 {
    20.0 * linear.log10()
}

#[cfg(test)]
// Exact float comparisons are intentional here: the expected values (silence, untouched
// samples) are produced without any arithmetic rounding.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random noise so the test needs no RNG dependency.
    fn noise(len: usize, amplitude: f32) -> Vec<f32> {
        let mut state: u32 = 0x1234_5678;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as f32 / u32::MAX as f32 * 2.0 - 1.0) * amplitude
            })
            .collect()
    }

    #[test]
    fn output_never_exceeds_ceiling() {
        let mut lim = Limiter::new(-1.0, 50.0, 48_000);
        let ceiling = db_to_linear(-1.0);
        let mut buf = noise(48_000 * 2, 4.0);
        lim.process_interleaved(&mut buf, 2);
        let max = buf.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(max <= ceiling + 1e-6, "peak {max} above ceiling {ceiling}");
    }

    #[test]
    fn quiet_signal_passes_unchanged() {
        let mut lim = Limiter::new(-1.0, 50.0, 48_000);
        let original = noise(4800, 0.3);
        let mut buf = original.clone();
        lim.process_interleaved(&mut buf, 1);
        assert_eq!(buf, original);
        assert!(lim.gain_reduction_db().abs() < 1e-6);
    }

    #[test]
    fn gain_recovers_after_a_peak() {
        let mut lim = Limiter::new(-1.0, 10.0, 48_000);
        let mut burst = vec![2.0_f32; 100];
        lim.process_interleaved(&mut burst, 1);
        assert!(lim.gain_reduction_db() > 6.0);
        let mut silence = vec![0.0_f32; 48_000];
        lim.process_interleaved(&mut silence, 1);
        assert!(
            lim.gain_reduction_db() < 0.01,
            "gain should recover in silence"
        );
    }

    #[test]
    fn stereo_channels_are_linked() {
        let mut lim = Limiter::new(0.0, 50.0, 48_000);
        let mut frame = [2.0_f32, 0.5];
        lim.process_interleaved(&mut frame, 2);
        assert!((frame[0] - 1.0).abs() < 1e-6);
        assert!(
            (frame[1] - 0.25).abs() < 1e-6,
            "quiet channel reduced by the same gain"
        );
    }

    #[test]
    fn non_finite_samples_become_silence() {
        let mut lim = Limiter::new(-1.0, 50.0, 48_000);
        let mut buf = [f32::NAN, f32::INFINITY, 0.5, f32::NEG_INFINITY];
        lim.process_interleaved(&mut buf, 1);
        assert_eq!(buf[0], 0.0);
        assert_eq!(buf[1], 0.0);
        assert!((buf[2] - 0.5).abs() < 1e-6);
        assert_eq!(buf[3], 0.0);
    }

    #[test]
    fn db_conversions() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(-6.020_6) - 0.5).abs() < 1e-4);
        assert!((linear_to_db(2.0) - 6.020_6).abs() < 1e-3);
    }
}
