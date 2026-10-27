//! Sample format and channel conversions at the edges of the mixer.
//!
//! The mixer and devices work in `f32`; libwebrtc (APM, Opus, decoded tracks) works in
//! interleaved `i16`. Everything here is allocation-free and safe on the audio thread.

/// Scale of `i16` full range. `-32768` maps to exactly `-1.0`.
const I16_SCALE: f32 = 32_768.0;

/// Converts `i16` samples to `f32` in `-1.0..1.0`.
///
/// # Panics
/// If the slices differ in length (a caller bug).
pub fn i16_to_f32(input: &[i16], out: &mut [f32]) {
    assert_eq!(input.len(), out.len(), "pcm length mismatch");
    for (o, &s) in out.iter_mut().zip(input) {
        *o = f32::from(s) / I16_SCALE;
    }
}

/// Converts `f32` samples to `i16`, rounding to nearest and clamping out-of-range
/// values (and NaN, to silence) instead of wrapping.
///
/// # Panics
/// If the slices differ in length (a caller bug).
pub fn f32_to_i16(input: &[f32], out: &mut [i16]) {
    assert_eq!(input.len(), out.len(), "pcm length mismatch");
    for (o, &s) in out.iter_mut().zip(input) {
        // `as` saturates on overflow and maps NaN to 0, which is exactly what we want.
        *o = (s * I16_SCALE).round().clamp(-I16_SCALE, I16_SCALE - 1.0) as i16;
    }
}

/// Averages each interleaved frame of `input` (`channels` wide) into one sample of
/// `out`. `out` must hold exactly `input.len() / channels` samples.
///
/// # Panics
/// If `channels` is zero or the lengths do not match (a caller bug).
pub fn downmix_to_mono(input: &[f32], channels: usize, out: &mut [f32]) {
    assert!(channels > 0, "zero channels");
    assert_eq!(input.len(), out.len() * channels, "pcm length mismatch");
    if channels == 1 {
        out.copy_from_slice(input);
        return;
    }
    let scale = 1.0 / channels as f32;
    for (o, frame) in out.iter_mut().zip(input.chunks_exact(channels)) {
        *o = frame.iter().sum::<f32>() * scale;
    }
}

#[cfg(test)]
// Conversions of representable values are exact.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn i16_extremes_map_to_unit_range() {
        let mut out = [0.0_f32; 4];
        i16_to_f32(&[i16::MIN, 0, 16_384, i16::MAX], &mut out);
        assert_eq!(out[0], -1.0);
        assert_eq!(out[1], 0.0);
        assert_eq!(out[2], 0.5);
        assert!(out[3] < 1.0 && out[3] > 0.9999);
    }

    #[test]
    fn f32_to_i16_rounds_and_clamps() {
        let mut out = [0_i16; 6];
        f32_to_i16(&[0.5, -1.0, 1.0, 3.0, -3.0, f32::NAN], &mut out);
        assert_eq!(out, [16_384, i16::MIN, i16::MAX, i16::MAX, i16::MIN, 0]);
    }

    #[test]
    fn round_trip_is_lossless_for_every_i16() {
        let all: Vec<i16> = (i16::MIN..=i16::MAX).collect();
        let mut f = vec![0.0_f32; all.len()];
        let mut back = vec![0_i16; all.len()];
        i16_to_f32(&all, &mut f);
        f32_to_i16(&f, &mut back);
        assert_eq!(all, back);
    }

    #[test]
    fn downmix_averages_channels() {
        let mut mono = [0.0_f32; 2];
        downmix_to_mono(&[0.2, 0.4, -1.0, 1.0], 2, &mut mono);
        assert!((mono[0] - 0.3).abs() < 1e-6);
        assert_eq!(mono[1], 0.0);

        let mut one = [0.0_f32; 1];
        downmix_to_mono(&[0.3, 0.3, 0.3, 0.3, 0.3, 0.3], 6, &mut one);
        assert!(
            (one[0] - 0.3).abs() < 1e-6,
            "5.1 microphone arrays fold down too"
        );
    }

    #[test]
    #[should_panic(expected = "length mismatch")]
    fn downmix_rejects_partial_frames() {
        downmix_to_mono(&[0.0; 3], 2, &mut [0.0; 1]);
    }
}
