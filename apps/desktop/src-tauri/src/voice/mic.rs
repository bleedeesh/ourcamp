//! Microphone pipeline: capture → APM (AEC3 + noise suppression + AGC) → LiveKit.
//!
//! One thread owns the capture stream and the APM. Every 10 ms frame it first feeds
//! the APM everything the speakers played since the last frame (the mixer's
//! [`OutputTap`]), then processes the microphone frame and hands it to the
//! [`NativeAudioSource`] of the published track. See
//! docs/decisions/0009-mic-processing-webrtc-apm.md.
//!
//! ```text
//! cpal input ──ring──► oc-mic thread: APM.process_stream ─► NativeAudioSource ─► Opus
//! mixer output ─tap──►               APM.process_reverse_stream (echo reference)
//! ```

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::AudioSourceOptions;
use livekit::webrtc::native::apm::AudioProcessingModule;
use oc_audio::meter::{rms_dbfs, SpeechDetector, SpeechDetectorConfig, SILENCE_DB};
use oc_audio::mixer::OutputTap;
use oc_audio::pcm::{f32_to_i16, i16_to_f32};

use crate::audio::input;
use crate::audio::output::DeviceLatency;

/// Upper bound on how long the thread sleeps if the driver stops calling back.
const WAKE_INTERVAL: Duration = Duration::from_millis(20);
/// If capture piles up beyond this many 10 ms frames (thread starved), older audio is
/// dropped: talking 200 ms late is worse than losing a few milliseconds once.
const MAX_CAPTURE_BACKLOG_FRAMES: usize = 8;
/// Same for the echo reference; AEC3 copes with gaps far better than with stale input.
const MAX_REFERENCE_BACKLOG_FRAMES: usize = 10;

/// Live level of the local voice after processing, for the user's own speaking ring.
#[derive(Debug, Default)]
pub struct LocalLevel {
    level_db: AtomicU32,
    speaking: AtomicBool,
}

impl LocalLevel {
    /// Level in dBFS and speech detector state of the last frame sent.
    #[must_use]
    pub fn get(&self) -> (f32, bool) {
        (
            f32::from_bits(self.level_db.load(Ordering::Relaxed)),
            self.speaking.load(Ordering::Relaxed),
        )
    }

    fn set(&self, level_db: f32, speaking: bool) {
        self.level_db.store(level_db.to_bits(), Ordering::Relaxed);
        self.speaking.store(speaking, Ordering::Relaxed);
    }
}

/// A running microphone pipeline. [`stop`](Self::stop) returns the output tap so the
/// next channel can reuse it.
pub struct MicPipeline {
    source: NativeAudioSource,
    muted: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    level: Arc<LocalLevel>,
    thread: Option<JoinHandle<OutputTap>>,
}

struct Shared {
    muted: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    level: Arc<LocalLevel>,
}

impl MicPipeline {
    /// Opens the default microphone and starts processing.
    ///
    /// # Errors
    /// If the microphone cannot be opened; the tap is handed back with the message.
    pub fn start(
        tap: OutputTap,
        output_latency: DeviceLatency,
        muted: bool,
    ) -> Result<Self, (String, OutputTap)> {
        let shared = Shared {
            muted: Arc::new(AtomicBool::new(muted)),
            stop: Arc::new(AtomicBool::new(false)),
            level: Arc::new(LocalLevel::default()),
        };
        let (muted, stop, level) = (
            Arc::clone(&shared.muted),
            Arc::clone(&shared.stop),
            Arc::clone(&shared.level),
        );
        let (ready_tx, ready_rx) = mpsc::channel::<Result<NativeAudioSource, String>>();
        // The OS refusing a thread means the process is out of resources; the tap inside
        // the closure could not be recovered anyway.
        let thread = thread::Builder::new()
            .name("oc-mic".into())
            .spawn(move || run(tap, &output_latency, &shared, &ready_tx))
            .expect("failed to spawn microphone thread");
        match ready_rx.recv() {
            Ok(Ok(source)) => Ok(Self {
                source,
                muted,
                stop,
                level,
                thread: Some(thread),
            }),
            Ok(Err(message)) => Err((message, join(thread))),
            Err(_) => Err(("поток микрофона завершился".into(), join(thread))),
        }
    }

    /// Source of the published microphone track.
    #[must_use]
    pub fn source(&self) -> NativeAudioSource {
        self.source.clone()
    }

    /// Stops sending audio (processing keeps running so the echo canceller stays
    /// converged and unmuting is instant).
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    /// Level of the local voice.
    #[must_use]
    pub fn level(&self) -> Arc<LocalLevel> {
        Arc::clone(&self.level)
    }

    /// Closes the microphone and returns the output tap.
    #[must_use]
    pub fn stop(mut self) -> OutputTap {
        self.stop.store(true, Ordering::Relaxed);
        let thread = self.thread.take().expect("stop called once");
        thread.thread().unpark();
        join(thread)
    }
}

impl Drop for MicPipeline {
    fn drop(&mut self) {
        // Only reached without `stop()` (app shutdown): close the device, drop the tap.
        if let Some(thread) = self.thread.take() {
            self.stop.store(true, Ordering::Relaxed);
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

fn join(thread: JoinHandle<OutputTap>) -> OutputTap {
    match thread.join() {
        Ok(tap) => tap,
        // A panic in the pipeline is a bug; surface it instead of limping on without AEC.
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// Buffers for one direction of 10 ms frames.
struct Frame {
    f32: Vec<f32>,
    i16: Vec<i16>,
    rate: u32,
    /// The same rate as libwebrtc's C API takes it.
    rate_i32: i32,
}

impl Frame {
    fn new(rate: u32) -> Self {
        let len = (rate / 100) as usize;
        Self {
            f32: vec![0.0; len],
            i16: vec![0; len],
            rate,
            rate_i32: i32::try_from(rate).expect("sample rate fits in i32"),
        }
    }

    fn len(&self) -> usize {
        self.f32.len()
    }
}

fn run(
    mut tap: OutputTap,
    output_latency: &DeviceLatency,
    shared: &Shared,
    ready: &mpsc::Sender<Result<NativeAudioSource, String>>,
) -> OutputTap {
    let mut capture = match input::open(thread::current()) {
        Ok(capture) => capture,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return tap;
        }
    };
    eprintln!(
        "[oc-voice] microphone: {} @ {} Hz",
        capture.device_name, capture.sample_rate
    );

    // Processing is ours (APM below); the source must not run its own.
    let options = AudioSourceOptions {
        echo_cancellation: false,
        noise_suppression: false,
        auto_gain_control: false,
    };
    // Queue 0: frames go straight to the encoder, no extra buffering latency.
    let source = NativeAudioSource::new(options, capture.sample_rate, 1, 0);
    let _ = ready.send(Ok(source.clone()));

    let mut apm = AudioProcessingModule::new(true, true, true, true);
    let mut mic = Frame::new(capture.sample_rate);
    // An output rate that does not give whole 10 ms frames (very rare) means no echo
    // reference; everything else still works.
    let mut reference = (tap.sample_rate() % 100 == 0).then(|| Frame::new(tap.sample_rate()));
    if reference.is_none() {
        eprintln!(
            "[oc-voice] output rate {} Hz: echo cancellation disabled",
            tap.sample_rate()
        );
    }
    // Whatever was played before we joined is irrelevant to this microphone.
    tap.discard_older_than(0);
    let mut detector = SpeechDetector::new(SpeechDetectorConfig::voice(capture.sample_rate));
    let mut reported_error = false;
    let mut report = |what: &str, e: &dyn std::fmt::Display| {
        if !reported_error {
            eprintln!("[oc-voice] {what}: {e}");
            reported_error = true;
        }
    };

    while !shared.stop.load(Ordering::Relaxed) {
        thread::park_timeout(WAKE_INTERVAL);

        let backlog = capture.samples.available();
        if backlog > mic.len() * MAX_CAPTURE_BACKLOG_FRAMES {
            capture.samples.skip(backlog - mic.len());
        }

        while capture.samples.available() >= mic.len() {
            if let Some(reference) = reference.as_mut() {
                tap.discard_older_than(reference.len() * MAX_REFERENCE_BACKLOG_FRAMES);
                while tap.available() >= reference.len() {
                    tap.read(&mut reference.f32);
                    f32_to_i16(&reference.f32, &mut reference.i16);
                    if let Err(e) =
                        apm.process_reverse_stream(&mut reference.i16, reference.rate_i32, 1)
                    {
                        report("echo reference", &e.message);
                    }
                }
            }

            capture.samples.pop_slice(&mut mic.f32);
            f32_to_i16(&mic.f32, &mut mic.i16);
            let delay = output_latency.ms() + capture.latency.ms();
            let _ = apm.set_stream_delay_ms(i32::try_from(delay).unwrap_or(i32::MAX));
            if let Err(e) = apm.process_stream(&mut mic.i16, mic.rate_i32, 1) {
                report("processing", &e.message);
            }

            if shared.muted.load(Ordering::Relaxed) {
                detector.reset();
                shared.level.set(SILENCE_DB, false);
                continue;
            }
            i16_to_f32(&mic.i16, &mut mic.f32);
            let level = rms_dbfs(&mic.f32);
            let speaking = detector.update(level, mic.len() as u32);
            shared.level.set(level, speaking);

            let frame = AudioFrame {
                data: Cow::Borrowed(&mic.i16),
                sample_rate: mic.rate,
                num_channels: 1,
                samples_per_channel: mic.len() as u32,
            };
            // With queue 0 this completes synchronously; no runtime is needed to drive it.
            if let Err(e) = tauri::async_runtime::block_on(source.capture_frame(&frame)) {
                report("send", &e.message);
            }
        }
    }
    drop(capture);
    tap
}
