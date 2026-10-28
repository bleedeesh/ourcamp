//! Microphone capture: the cpal input callback folds every frame to mono and pushes it
//! into a lock-free ring read by the microphone pipeline (`voice::mic`).
//!
//! The stream is opened on the thread that will own it (cpal's `Stream` is not `Send`
//! everywhere); the callback wakes that thread with `unpark` whenever audio arrives.

use std::fmt;
use std::thread::Thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SampleRate, SizedSample, StreamConfig};
use oc_audio::ring::{ring_buffer, Consumer, Producer};
use oc_audio::SAMPLE_RATE;

use super::output::DeviceLatency;

/// Mono samples converted per chunk inside the callback. Allocated once per stream.
const SCRATCH_FRAMES: usize = 4096;
/// Capture ring capacity. The pipeline reads every 10 ms; this only matters if it stalls.
const RING_MS: u32 = 500;

/// Why the microphone could not be opened.
#[derive(Debug)]
pub enum InputError {
    /// The OS reports no default input device.
    NoDevice,
    /// No f32/i16 format at a rate libwebrtc accepts (a multiple of 100 Hz).
    UnsupportedFormat,
    /// Any other driver/backend error, with its message.
    Backend(String),
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => f.write_str("микрофон не найден"),
            Self::UnsupportedFormat => f.write_str("микрофон работает в неподдерживаемом формате"),
            Self::Backend(msg) => write!(f, "ошибка микрофона: {msg}"),
        }
    }
}

impl std::error::Error for InputError {}

fn backend<E: fmt::Display>(e: E) -> InputError {
    InputError::Backend(e.to_string())
}

/// A running capture stream. Dropping `stream` stops the device.
pub struct Capture {
    /// Keeps the device running; must stay on the thread that opened it.
    #[allow(
        dead_code,
        reason = "held only to keep the device running until dropped"
    )]
    pub stream: cpal::Stream,
    /// Mono samples at `sample_rate`.
    pub samples: Consumer,
    /// Device rate (a multiple of 100, so 10 ms frames are whole).
    pub sample_rate: u32,
    /// Human-readable device name.
    pub device_name: String,
    /// Time from the microphone to the callback, for the echo canceller's delay hint.
    pub latency: DeviceLatency,
}

/// Opens the default microphone. `reader` is unparked whenever new samples arrive.
///
/// # Errors
/// See [`InputError`].
pub fn open(reader: Thread) -> Result<Capture, InputError> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or(InputError::NoDevice)?;
    let device_name = device.name().unwrap_or_else(|_| "unknown".into());
    let (config, format) = choose_config(&device)?;
    let sample_rate = config.sample_rate.0;
    let (producer, samples) = ring_buffer((sample_rate * RING_MS / 1000) as usize);
    let latency = DeviceLatency::default();

    let ctx = CallbackCtx {
        producer,
        scratch: vec![0.0; SCRATCH_FRAMES],
        channels: usize::from(config.channels),
        reader,
        latency: latency.clone(),
    };
    let on_error = |err| eprintln!("[oc-audio] input stream error: {err}");
    let stream = match format {
        SampleFormat::F32 => {
            device.build_input_stream(&config, ctx.into_callback::<f32>(), on_error, None)
        }
        SampleFormat::I16 => {
            device.build_input_stream(&config, ctx.into_callback::<i16>(), on_error, None)
        }
        _ => return Err(InputError::UnsupportedFormat),
    }
    .map_err(backend)?;
    stream.play().map_err(backend)?;

    Ok(Capture {
        stream,
        samples,
        sample_rate,
        device_name,
        latency,
    })
}

/// Prefers 48 kHz (no resampling anywhere on the way to Opus) in f32, then i16, then
/// the device default if its rate gives whole 10 ms frames.
fn choose_config(device: &cpal::Device) -> Result<(StreamConfig, SampleFormat), InputError> {
    let usable = |f: SampleFormat| matches!(f, SampleFormat::F32 | SampleFormat::I16);
    let preferred = device
        .supported_input_configs()
        .map_err(backend)?
        .filter(|r| usable(r.sample_format()))
        .filter(|r| r.min_sample_rate().0 <= SAMPLE_RATE && r.max_sample_rate().0 >= SAMPLE_RATE)
        // f32 first, then the fewest channels (less to fold down).
        .min_by_key(|r| (r.sample_format() != SampleFormat::F32, r.channels()))
        .map(|r| r.with_sample_rate(SampleRate(SAMPLE_RATE)));

    let supported = if let Some(config) = preferred {
        config
    } else {
        let default = device.default_input_config().map_err(backend)?;
        if !usable(default.sample_format()) || !default.sample_rate().0.is_multiple_of(100) {
            return Err(InputError::UnsupportedFormat);
        }
        default
    };
    Ok((supported.config(), supported.sample_format()))
}

struct CallbackCtx {
    producer: Producer,
    scratch: Vec<f32>,
    channels: usize,
    reader: Thread,
    latency: DeviceLatency,
}

impl CallbackCtx {
    /// Real-time callback: convert + fold to mono into `scratch`, push, wake the reader.
    /// No locks, no allocation. If the ring is full the newest audio is dropped.
    fn into_callback<T>(mut self) -> impl FnMut(&[T], &cpal::InputCallbackInfo) + Send + 'static
    where
        T: SizedSample + Send + 'static,
        f32: FromSample<T>,
    {
        move |data: &[T], info: &cpal::InputCallbackInfo| {
            let ts = info.timestamp();
            self.latency.store(ts.callback.duration_since(&ts.capture));
            let scale = 1.0 / self.channels as f32;
            for chunk in data.chunks(SCRATCH_FRAMES * self.channels) {
                let frames = chunk.len() / self.channels;
                let mono = &mut self.scratch[..frames];
                for (o, frame) in mono.iter_mut().zip(chunk.chunks_exact(self.channels)) {
                    *o = frame.iter().map(|&s| s.to_sample::<f32>()).sum::<f32>() * scale;
                }
                self.producer.push_slice(mono);
            }
            self.reader.unpark();
        }
    }
}
