//! # oc-audio
//!
//! Real-time audio core of the OurCamp desktop client.
//!
//! Every remote voice and every stream's audio is decoded by the WebRTC engine and
//! written into a [`mixer::SourceInput`]. The [`mixer::MixerCore`] runs inside the
//! audio device callback and produces the final output:
//!
//! * per-participant and per-stream **local volume** (0–200 %) with click-free glides;
//! * local mute and global **deafen** that keep every volume intact;
//! * a **limiter** so 200 % on several loud people never crackles;
//! * pre-volume **level metering** and **speech detection** for the speaking ring.
//!
//! The crate has no dependencies and no I/O; device access lives in the desktop app.
//! Design notes: `docs/decisions/0006-own-audio-mixer.md`.
//!
//! ```
//! use oc_audio::{gain::percent_to_gain, mixer::{mixer, Layout, MixerConfig}};
//!
//! let (mut control, mut core) = mixer::<String>(MixerConfig::default_48k_stereo());
//! // Lena is quiet, so this user hears her at 150 %.
//! let mut lena = control.add_source("lena".into(), Layout::Mono, percent_to_gain(150.0)).unwrap();
//!
//! lena.write(&[0.1; 480]);           // decoder thread
//! let mut out = vec![0.0; 480 * 2];  // device callback, stereo
//! core.process(&mut out);
//! assert!((out[0] - 0.15).abs() < 1e-6);
//!
//! // Later, from the volume slider; the change glides in over ~15 ms.
//! control.set_gain(&"lena".into(), percent_to_gain(80.0)).unwrap();
//! ```

pub mod gain;
pub mod limiter;
pub mod meter;
pub mod mixer;
pub mod pcm;
pub mod ring;

/// Sample rate used throughout OurCamp (Opus's native rate).
pub const SAMPLE_RATE: u32 = 48_000;
