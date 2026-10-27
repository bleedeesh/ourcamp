//! Audio device output: runs [`MixerCore`] inside the cpal callback.
//!
//! cpal's `Stream` is not `Send` on every platform, so the device is opened, played and
//! kept alive on a dedicated thread. That thread hands the mixer's control half back to
//! the caller and then parks until [`Output`] is dropped.

use std::fmt;
use std::hash::Hash;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use oc_audio::meter::SpeechDetectorConfig;
use oc_audio::mixer::{mixer, Layout, MixerConfig, MixerControl, MixerCore};
use oc_audio::SAMPLE_RATE;

/// Frames processed per mixer call when the device has more than two channels and the
/// stereo mix has to be spread into a wider frame. Allocated once per stream.
const WIDE_SCRATCH_FRAMES: usize = 4096;

/// Why the output device could not be opened.
#[derive(Debug)]
pub enum OutputError {
    /// The OS reports no default output device.
    NoDevice,
    /// The device offers no 32-bit float format (all mainstream drivers do).
    UnsupportedFormat,
    /// Any other driver/backend error, with its message.
    Backend(String),
}

impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => f.write_str("не найдено устройство вывода звука"),
            Self::UnsupportedFormat => {
                f.write_str("устройство вывода не поддерживает 32-bit float")
            }
            Self::Backend(msg) => write!(f, "ошибка аудиодрайвера: {msg}"),
        }
    }
}

impl std::error::Error for OutputError {}

fn backend<E: fmt::Display>(e: E) -> OutputError {
    OutputError::Backend(e.to_string())
}

/// A running output stream. Dropping it stops the stream.
pub struct Output {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    sample_rate: u32,
    device_name: String,
}

impl Output {
    /// Sample rate the mixer runs at (the device's).
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Human-readable device name.
    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.device_name
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        // Closing the channel wakes the stream thread, which drops the stream.
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Opens the default output device and starts mixing into it.
///
/// # Errors
/// See [`OutputError`].
pub fn open<K>() -> Result<(MixerControl<K>, Output), OutputError>
where
    K: Eq + Hash + Clone + Send + 'static,
{
    let (ready_tx, ready_rx) =
        mpsc::channel::<Result<(MixerControl<K>, u32, String), OutputError>>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();

    let thread = thread::Builder::new()
        .name("oc-audio-output".into())
        .spawn(move || {
            let started = start_stream::<K>();
            let stream = match started {
                Ok((stream, control, rate, name)) => {
                    let _ = ready_tx.send(Ok((control, rate, name)));
                    stream
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            // Park until the owning `Output` is dropped (sender closed).
            let _ = stop_rx.recv();
            drop(stream);
        })
        .map_err(backend)?;

    let (control, sample_rate, device_name) = ready_rx
        .recv()
        .map_err(|_| OutputError::Backend("поток вывода звука завершился".into()))??;

    Ok((
        control,
        Output {
            stop: Some(stop_tx),
            thread: Some(thread),
            sample_rate,
            device_name,
        },
    ))
}

type Started<K> = (cpal::Stream, MixerControl<K>, u32, String);

fn start_stream<K: Eq + Hash + Clone>() -> Result<Started<K>, OutputError> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(OutputError::NoDevice)?;
    let name = device.name().unwrap_or_else(|_| "unknown".into());
    let config = choose_config(&device)?;
    let channels = usize::from(config.channels);
    let rate = config.sample_rate.0;

    let mixer_config = MixerConfig {
        sample_rate: rate,
        output: if channels == 1 {
            Layout::Mono
        } else {
            Layout::Stereo
        },
        speech: SpeechDetectorConfig::voice(rate),
        ..MixerConfig::default_48k_stereo()
    };
    let (control, core) = mixer::<K>(mixer_config);

    let stream = device
        .build_output_stream(
            &config,
            render_callback(core, channels),
            // Device errors (unplugged headset, driver reset) are logged for now;
            // device switching and UI reporting arrive with the settings window (M7).
            |err| eprintln!("[oc-audio] output stream error: {err}"),
            None,
        )
        .map_err(backend)?;
    stream.play().map_err(backend)?;
    Ok((stream, control, rate, name))
}

/// Prefers f32 / 48 kHz / stereo (Opus's native rate: no resampling of voice), then the
/// device default if it is f32.
fn choose_config(device: &cpal::Device) -> Result<StreamConfig, OutputError> {
    let preferred = device
        .supported_output_configs()
        .map_err(backend)?
        .filter(|r| r.sample_format() == SampleFormat::F32)
        .filter(|r| r.min_sample_rate().0 <= SAMPLE_RATE && r.max_sample_rate().0 >= SAMPLE_RATE)
        .min_by_key(|r| (r.channels() != 2, r.channels()))
        .map(|r| r.with_sample_rate(SampleRate(SAMPLE_RATE)));

    let supported = match preferred {
        Some(config) => config,
        None => {
            let default = device.default_output_config().map_err(backend)?;
            if default.sample_format() != SampleFormat::F32 {
                return Err(OutputError::UnsupportedFormat);
            }
            default
        }
    };
    Ok(supported.config())
}

/// Builds the real-time callback. Mono/stereo devices get the mix directly; wider
/// devices (5.1/7.1) receive it on the front pair with the other channels silent.
fn render_callback(
    mut core: MixerCore,
    channels: usize,
) -> impl FnMut(&mut [f32], &cpal::OutputCallbackInfo) + Send + 'static {
    let mut scratch = if channels > 2 {
        vec![0.0_f32; WIDE_SCRATCH_FRAMES * 2]
    } else {
        Vec::new()
    };
    move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
        if channels <= 2 {
            core.process(data);
            return;
        }
        for chunk in data.chunks_mut(WIDE_SCRATCH_FRAMES * channels) {
            let frames = chunk.len() / channels;
            let stereo = &mut scratch[..frames * 2];
            core.process(stereo);
            for (dst, src) in chunk.chunks_mut(channels).zip(stereo.chunks_exact(2)) {
                dst[0] = src[0];
                dst[1] = src[1];
                dst[2..].fill(0.0);
            }
        }
    }
}
