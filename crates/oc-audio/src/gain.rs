//! Volume control primitives.
//!
//! Volumes are linear amplitude factors in `0.0..=MAX_GAIN`. The UI shows them as
//! percent (100 % = unchanged, 200 % = twice the amplitude ≈ +6 dB), the same scale
//! users know from other voice apps. Anything louder is the limiter's job, not the
//! slider's.
//!
//! Gains are written from the UI thread and read on the audio thread, so they live in
//! an [`AtomicGain`]. On the audio thread a [`GainRamp`] glides to a new value over a
//! few milliseconds instead of jumping, which would otherwise be audible as a click.

use std::sync::atomic::{AtomicU32, Ordering};

/// Highest allowed linear gain (200 %).
pub const MAX_GAIN: f32 = 2.0;

/// Converts a UI percent value (0–200) to a linear gain, clamping out-of-range input.
#[must_use]
pub fn percent_to_gain(percent: f32) -> f32 {
    sanitize(percent / 100.0)
}

/// Converts a linear gain back to a UI percent value.
#[must_use]
pub fn gain_to_percent(gain: f32) -> f32 {
    sanitize(gain) * 100.0
}

/// Clamps a gain to `0.0..=MAX_GAIN`; NaN becomes silence rather than poisoning the mix.
#[must_use]
pub fn sanitize(gain: f32) -> f32 {
    if gain.is_nan() {
        0.0
    } else {
        gain.clamp(0.0, MAX_GAIN)
    }
}

/// An `f32` gain shared between threads without locks.
#[derive(Debug)]
pub struct AtomicGain(AtomicU32);

impl AtomicGain {
    /// Creates a gain, clamped to the valid range.
    #[must_use]
    pub fn new(gain: f32) -> Self {
        Self(AtomicU32::new(sanitize(gain).to_bits()))
    }

    /// Stores a new gain (clamped).
    pub fn set(&self, gain: f32) {
        self.0.store(sanitize(gain).to_bits(), Ordering::Relaxed);
    }

    /// Loads the current gain.
    #[must_use]
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

impl Default for AtomicGain {
    fn default() -> Self {
        Self::new(1.0)
    }
}

/// Linear per-sample glide from the current gain to a target gain.
#[derive(Debug, Clone)]
pub struct GainRamp {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
    ramp_len: u32,
}

impl GainRamp {
    /// Creates a ramp resting at `initial` that takes `ramp_len` samples to reach a new
    /// target. A `ramp_len` of zero makes changes instantaneous.
    #[must_use]
    pub fn new(initial: f32, ramp_len: u32) -> Self {
        let initial = sanitize(initial);
        Self {
            current: initial,
            target: initial,
            step: 0.0,
            remaining: 0,
            ramp_len,
        }
    }

    /// Number of samples for a ramp of `ms` milliseconds at `sample_rate`.
    #[must_use]
    pub fn len_for(ms: f32, sample_rate: u32) -> u32 {
        (ms.max(0.0) / 1000.0 * sample_rate as f32).round() as u32
    }

    /// Sets a new target. Restarts the glide from wherever the gain currently is.
    pub fn set_target(&mut self, target: f32) {
        let target = sanitize(target);
        if (target - self.target).abs() <= f32::EPSILON {
            return;
        }
        self.target = target;
        if self.ramp_len == 0 {
            self.current = target;
            self.remaining = 0;
        } else {
            self.remaining = self.ramp_len;
            self.step = (target - self.current) / self.ramp_len as f32;
        }
    }

    /// Advances one sample and returns the gain to apply to it.
    #[inline]
    pub fn next_gain(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target;
            } else {
                self.current += self.step;
            }
        }
        self.current
    }

    /// Whether the ramp has reached its target.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.remaining == 0
    }

    /// The gain that will be applied once the ramp settles.
    #[must_use]
    pub fn target(&self) -> f32 {
        self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn percent_conversion_round_trips_and_clamps() {
        assert!(approx(percent_to_gain(100.0), 1.0));
        assert!(approx(percent_to_gain(50.0), 0.5));
        assert!(approx(percent_to_gain(200.0), 2.0));
        assert!(approx(percent_to_gain(350.0), MAX_GAIN));
        assert!(approx(percent_to_gain(-20.0), 0.0));
        assert!(approx(percent_to_gain(f32::NAN), 0.0));
        assert!(approx(gain_to_percent(percent_to_gain(137.0)), 137.0));
    }

    #[test]
    fn atomic_gain_clamps_on_store() {
        let g = AtomicGain::default();
        assert!(approx(g.get(), 1.0));
        g.set(5.0);
        assert!(approx(g.get(), MAX_GAIN));
        g.set(f32::NAN);
        assert!(approx(g.get(), 0.0));
    }

    #[test]
    fn ramp_reaches_target_in_exactly_ramp_len_samples() {
        let mut r = GainRamp::new(0.0, 4);
        r.set_target(1.0);
        let seq: Vec<f32> = (0..6).map(|_| r.next_gain()).collect();
        assert!(approx(seq[0], 0.25));
        assert!(approx(seq[1], 0.5));
        assert!(approx(seq[2], 0.75));
        assert!(approx(seq[3], 1.0));
        assert!(approx(seq[5], 1.0));
        assert!(r.is_settled());
    }

    #[test]
    fn ramp_is_monotonic_without_overshoot() {
        let mut r = GainRamp::new(1.6, 480);
        r.set_target(0.2);
        let mut prev = 1.6;
        for _ in 0..1000 {
            let g = r.next_gain();
            assert!(g <= prev + 1e-6, "ramp went up while heading down");
            assert!(g >= 0.2 - 1e-6, "ramp overshot the target");
            prev = g;
        }
        assert!(approx(prev, 0.2));
    }

    #[test]
    fn retargeting_mid_ramp_continues_from_current_value() {
        let mut r = GainRamp::new(0.0, 10);
        r.set_target(1.0);
        for _ in 0..5 {
            r.next_gain();
        }
        r.set_target(0.0);
        let first = r.next_gain();
        assert!(
            first < 0.5 && first > 0.4,
            "should glide down from ~0.5, got {first}"
        );
    }

    #[test]
    fn zero_length_ramp_is_instant() {
        let mut r = GainRamp::new(1.0, 0);
        r.set_target(0.3);
        assert!(approx(r.next_gain(), 0.3));
        assert!(r.is_settled());
    }

    #[test]
    fn ramp_length_from_milliseconds() {
        assert_eq!(GainRamp::len_for(10.0, 48_000), 480);
        assert_eq!(GainRamp::len_for(-5.0, 48_000), 0);
    }
}
