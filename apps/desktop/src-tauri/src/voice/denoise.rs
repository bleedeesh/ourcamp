//! The neural noise suppressor, `DeepFilterNet3` (ADR-0073): takes away what WebRTC's cannot,
//! such as claps and sounds from another room. Runs on the microphone's thread between the echo
//! cancellation and the gain control.

use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::{ArrayView2, ArrayViewMut2};

/// The only rate the model knows.
pub const RATE: u32 = 48_000;

pub struct Denoiser {
    model: DfTract,
}

impl Denoiser {
    /// Unpacks the model built into the app; about 100 ms in an optimized build.
    ///
    /// # Errors
    /// If the model does not load.
    pub fn new() -> Result<Self, String> {
        let params = RuntimeParams::default_with_ch(1);
        let model = DfTract::new(DfParams::default(), &params).map_err(|e| e.to_string())?;
        Ok(Self { model })
    }

    /// Cleans 10 ms at [`RATE`] in place. The sound comes out 30 ms later than it went in: 10 ms
    /// of the transform and 20 ms the model looks ahead.
    ///
    /// # Errors
    /// If the model fails or `frame` is not 10 ms; `frame` is left as it was.
    pub fn process(&mut self, frame: &mut [f32], scratch: &mut [f32]) -> Result<(), String> {
        // The model does not refuse a wrong size, it panics.
        let hop = self.model.hop_size;
        if frame.len() != hop || scratch.len() != hop {
            return Err(format!("{} samples, the model takes {hop}", frame.len()));
        }
        let shape = (1, hop);
        let noisy = ArrayView2::from_shape(shape, &*frame).map_err(|e| e.to_string())?;
        let clean = ArrayViewMut2::from_shape(shape, &mut *scratch).map_err(|e| e.to_string())?;
        self.model
            .process(noisy, clean)
            .map_err(|e| e.to_string())?;
        frame.copy_from_slice(scratch);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_audio::meter::rms_dbfs;
    use std::f32::consts::TAU;

    const FRAME: usize = 480;

    fn rate() -> f32 {
        48_000.0
    }

    /// `seconds` of `sound` through the model; lined up with what went in.
    fn cleaned(seconds: usize, sound: impl Fn(usize) -> f32) -> (Vec<f32>, Vec<f32>) {
        let mut denoiser = Denoiser::new().unwrap();
        let mut scratch = [0.0; FRAME];
        let total = seconds * 48_000;
        let noisy: Vec<f32> = (0..total).map(sound).collect();
        let mut out = Vec::with_capacity(total);
        for chunk in noisy.as_chunks::<FRAME>().0 {
            let mut frame = *chunk;
            denoiser.process(&mut frame, &mut scratch).unwrap();
            out.extend_from_slice(&frame);
        }
        out.drain(..30 * 48);
        (noisy, out)
    }

    fn noise(i: usize) -> f32 {
        // A fixed sequence, so that the test is the same each time.
        let mut x = u32::try_from(i).unwrap().wrapping_mul(2_654_435_761) ^ 0x1234_5678;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        f32::from(u16::try_from(x >> 16).unwrap()) / 32_768.0 - 1.0
    }

    /// The simplest sound with the shape of a voice: harmonics of 140 Hz, rising and falling
    /// three times a second. The model trusts it less than real speech, which it leaves almost
    /// untouched (measured on a recording, ADR-0073), so this only guards against silence.
    fn voice(i: usize) -> f32 {
        let t = f32::from(u16::try_from(i % 48_000).unwrap()) / rate();
        let syllables = (TAU * 3.0 * t).sin().max(0.0);
        let tone: f32 = (1_u8..=20)
            .map(|k| {
                let k = f32::from(k);
                // The first formant, about 500 Hz.
                let shape = 1.0 / (1.0 + ((k * 140.0 - 500.0) / 300.0).powi(2));
                shape * (TAU * k * 140.0 * t).sin()
            })
            .sum();
        0.1 * syllables * tone
    }

    #[test]
    fn a_clap_in_a_quiet_room_is_gone() {
        let clap_every = 48_000;
        let clap = |i: usize| {
            let since = f32::from(u16::try_from(i % clap_every).unwrap()) / rate();
            0.3 * (since / 0.001).min(1.0) * (-since / 0.012).exp() * noise(i)
        };
        let (noisy, out) = cleaned(5, clap);
        let (before, after) = (rms_dbfs(&noisy[96_000..]), rms_dbfs(&out[96_000..]));
        assert!(
            after < before - 40.0,
            "before {before} dB, after {after} dB"
        );
    }

    #[test]
    fn a_voice_is_not_taken_for_noise() {
        let (noisy, out) = cleaned(5, voice);
        let (before, after) = (rms_dbfs(&noisy[96_000..]), rms_dbfs(&out[96_000..]));
        assert!(
            after > before - 15.0,
            "before {before} dB, after {after} dB"
        );
    }

    #[test]
    fn a_frame_of_the_wrong_size_is_refused_and_left_alone() {
        let mut denoiser = Denoiser::new().unwrap();
        let mut frame = [0.5; 100];
        let mut scratch = [0.0; FRAME];
        assert!(denoiser.process(&mut frame, &mut scratch).is_err());
        assert!(frame.iter().all(|&s| s >= 0.5));
    }
}
