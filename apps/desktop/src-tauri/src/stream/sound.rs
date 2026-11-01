//! The sound of a stream (ADR-0055): WASAPI process loopback, Windows 10 2004 and newer.
//! A screen goes with the sound of everything but this process, so viewers never hear
//! their own voices back; a window goes with the sound of its own process only.
//!
//! ```text
//! other processes ─ process loopback ─► oc-stream-sound thread ─► NativeAudioSource ─► room
//! ```

use std::borrow::Cow;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::track::{LocalAudioTrack, LocalTrack, TrackSource};
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit::Room;
use windows::core::{IUnknown, Interface, Result as WinResult, GUID, HRESULT};
use windows::Win32::Foundation::{CloseHandle, E_FAIL, HWND, WAIT_FAILED};
use windows::Win32::Media::Audio::{
    eConsole, eRender, ActivateAudioInterfaceAsync, IActivateAudioInterfaceCompletionHandler,
    IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, BLOB, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::Variant::VT_BLOB;
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

use super::SourceKind;

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
/// Samples of both channels in the 10 ms frame `WebRTC` takes.
const FRAME: usize = (RATE / 100) as usize * CHANNELS as usize;
/// Opus stereo for music and games (ADR-0049).
const BITRATE: u64 = 128_000;
/// The shared buffer, in the 100 ns units of WASAPI.
const BUFFER: i64 = 200_000;
/// How long the activation may take; it is one call that answers from another thread.
const ACTIVATION: Duration = Duration::from_secs(5);
/// Nothing plays: loopback may then give no packets at all. After this long without any,
/// silence goes out in their place, or the viewer's decoder would conceal a gap.
const QUIET: Duration = Duration::from_millis(30);

/// Whose sound goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Everything but this process and its children.
    AllBut(u32),
    /// This process and its children.
    Only(u32),
}

impl Target {
    /// The sound that goes with a source; `None` for a window that is no longer there.
    fn of(kind: SourceKind, id: u64) -> Option<Self> {
        match kind {
            SourceKind::Screen => Some(Self::AllBut(std::process::id())),
            SourceKind::Window => {
                let mut process = 0;
                // SAFETY: any value is safe to pass as a handle; a source's id on Windows
                // is its window's handle, and `process` outlives the call.
                unsafe { GetWindowThreadProcessId(HWND(id as *mut _), Some(&raw mut process)) };
                (process != 0).then_some(Self::Only(process))
            }
        }
    }
}

/// COM for this thread, for as long as the value lives.
pub(super) struct Com;

impl Com {
    pub(super) fn init() -> WinResult<Self> {
        // SAFETY: paired with `CoUninitialize` in `drop`, on the same thread.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
        Ok(Self)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: `init` succeeded on this thread (the value is not `Send`-sent anywhere).
        unsafe { CoUninitialize() };
    }
}

/// A module of its own so the lints on what `implement` generates stop here.
mod completion {
    #![allow(
        clippy::ref_as_ptr,
        clippy::inline_always,
        reason = "the code `implement` generates"
    )]

    use std::sync::mpsc;

    use windows::core::{implement, Ref, Result as WinResult};
    use windows::Win32::Media::Audio::{
        IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
        IActivateAudioInterfaceCompletionHandler_Impl,
    };

    /// Tells the waiting thread that the activation has finished. The answer comes on a thread
    /// of the system's pool, so the handler must be agile: `implement` makes it so by default.
    #[implement(IActivateAudioInterfaceCompletionHandler)]
    pub(super) struct Activated(pub(super) mpsc::Sender<()>);

    impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
        #[allow(non_snake_case, reason = "the name is the COM interface's")]
        fn ActivateCompleted(
            &self,
            _operation: Ref<'_, IActivateAudioInterfaceAsyncOperation>,
        ) -> WinResult<()> {
            let _ = self.0.send(());
            Ok(())
        }
    }
}
use completion::Activated;

/// An audio client bound to the target's sound. There is no device behind it, and it follows
/// whatever the default output becomes.
fn activate(target: Target) -> WinResult<IAudioClient> {
    let (process, mode) = match target {
        Target::AllBut(process) => (process, PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE),
        Target::Only(process) => (process, PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE),
    };
    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: process,
                ProcessLoopbackMode: mode,
            },
        },
    };
    // Never dropped: `PROPVARIANT` clears itself on drop, which would give `params`, on the
    // stack, to `CoTaskMemFree` and corrupt the heap.
    let variant = ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: (&raw mut params).cast(),
                    },
                },
            }),
        },
    });
    let (done, finished) = mpsc::channel();
    let handler: IActivateAudioInterfaceCompletionHandler = Activated(done).into();
    // SAFETY: `variant` and what it points at live until the answer below; the call copies
    // them anyway before returning.
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&raw const *variant),
            &handler,
        )
    }?;
    finished
        .recv_timeout(ACTIVATION)
        .map_err(|_| windows::core::Error::from(E_FAIL))?;
    let mut result = HRESULT(0);
    let mut client: Option<IUnknown> = None;
    // SAFETY: the operation has finished, and both pointers are to live locals.
    unsafe { operation.GetActivateResult(&raw mut result, &raw mut client) }?;
    result.ok()?;
    client
        .ok_or_else(|| windows::core::Error::from(E_FAIL))?
        .cast()
}

/// Whether process loopback works here: older systems refuse the activation. Blocks briefly.
#[must_use]
pub fn available() -> bool {
    let Ok(_com) = Com::init() else {
        return false;
    };
    activate(Target::AllBut(std::process::id())).is_ok()
}

/// `KSDATAFORMAT_SUBTYPE_PCM`.
const PCM: GUID = GUID::from_u128(0x0000_0001_0000_0010_8000_00aa_0038_9b71);
/// The bindings do not have it.
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const STEREO_MASK: u32 = 0x3;
/// How far a side or centre channel goes into a side of stereo: -3 dB, the usual share.
const SIDE: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// The channels of the default output, and their speakers (`SPEAKER_*` bits, in channel order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Speakers {
    channels: u16,
    mask: u32,
}

const STEREO: Speakers = Speakers {
    channels: 2,
    mask: STEREO_MASK,
};

impl Speakers {
    /// A device set to 7.1 is captured as 7.1: asked for stereo, Windows brings it down
    /// by 14 dB to keep any mix from clipping, and a plain stereo sound comes out far too
    /// quiet (ADR-0055). Stereo when anything about the device is unclear.
    fn of_default_output() -> Self {
        Self::read().unwrap_or(STEREO)
    }

    fn read() -> WinResult<Self> {
        // SAFETY: the format is read and freed here, once, with the allocator it came from;
        // the extensible part is read only when the format says it has one.
        unsafe {
            let devices: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let format = client.GetMixFormat()?;
            let channels = (*format).nChannels;
            let mask = if (*format).wFormatTag == WAVE_FORMAT_EXTENSIBLE {
                std::ptr::read_unaligned(
                    &raw const (*format.cast::<WAVEFORMATEXTENSIBLE>()).dwChannelMask,
                )
            } else {
                0
            };
            CoTaskMemFree(Some(format.cast()));
            Ok(Self::checked(channels, mask))
        }
    }

    /// Stereo unless the channels and the mask agree on more than two.
    fn checked(channels: u16, mask: u32) -> Self {
        if channels > 2 && mask.count_ones() == u32::from(channels) {
            Self { channels, mask }
        } else {
            STEREO
        }
    }

    /// Each channel's share of the left and the right, in channel order.
    fn shares(self) -> Vec<(f32, f32)> {
        (0..u32::BITS)
            .map(|bit| 1_u32 << bit)
            .filter(|speaker| self.mask & speaker != 0)
            .map(|speaker| match speaker {
                0x1 => (1.0, 0.0),                  // front left
                0x2 => (0.0, 1.0),                  // front right
                0x4 | 0x100 => (SIDE, SIDE),        // front centre, back centre
                0x10 | 0x40 | 0x200 => (SIDE, 0.0), // back, front-of-centre, side: left
                0x20 | 0x80 | 0x400 => (0.0, SIDE), // the same on the right
                _ => (0.0, 0.0),                    // the low frequencies, and the rest
            })
            .collect()
    }
}

/// Stereo from the device's channels. A sound in plain stereo keeps its level exactly; a full
/// surround mix may reach past the top, and clips there, as it would on stereo speakers.
fn downmix(samples: &[i16], shares: &[(f32, f32)], stereo: &mut Vec<i16>) {
    stereo.clear();
    for frame in samples.chunks_exact(shares.len()) {
        let (mut left, mut right) = (0.0_f32, 0.0_f32);
        for (sample, (to_left, to_right)) in frame.iter().zip(shares) {
            left += f32::from(*sample) * to_left;
            right += f32::from(*sample) * to_right;
        }
        // `as` saturates from float to integer: this is the clip.
        stereo.push(left as i16);
        stereo.push(right as i16);
    }
}

/// Cuts what arrives into the 10 ms frames `WebRTC` takes.
#[derive(Default)]
struct Frames {
    pending: Vec<i16>,
}

impl Frames {
    fn push(&mut self, samples: &[i16], sink: &mut dyn FnMut(&[i16])) {
        self.pending.extend_from_slice(samples);
        self.drain(sink);
    }

    fn silence(&mut self, samples: usize, sink: &mut dyn FnMut(&[i16])) {
        self.pending.resize(self.pending.len() + samples, 0);
        self.drain(sink);
    }

    fn drain(&mut self, sink: &mut dyn FnMut(&[i16])) {
        let whole = self.pending.len() / FRAME * FRAME;
        for frame in self.pending[..whole].as_chunks::<FRAME>().0 {
            sink(frame);
        }
        self.pending.drain(..whole);
    }
}

/// Samples of silence owed after `quiet` without packets, in whole frames, and the time they
/// cover. Nothing until [`QUIET`] has passed: packets a moment late are not a pause.
fn silence_due(quiet: Duration) -> (usize, Duration) {
    if quiet < QUIET {
        return (0, Duration::ZERO);
    }
    let frames = quiet.as_millis() / 10;
    (
        frames as usize * FRAME,
        Duration::from_millis(frames as u64 * 10),
    )
}

/// Reads the client until told to stop or until it fails.
fn pump(
    capture: &IAudioCaptureClient,
    event: windows::Win32::Foundation::HANDLE,
    speakers: Speakers,
    stop: &AtomicBool,
    sink: &mut dyn FnMut(&[i16]),
) -> WinResult<()> {
    let shares = speakers.shares();
    let mut stereo = Vec::new();
    let mut frames = Frames::default();
    let mut heard = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        // SAFETY: the event lives until after this function returns (`run`).
        if unsafe { WaitForSingleObject(event, 10) } == WAIT_FAILED {
            return Err(windows::core::Error::from_thread());
        }
        // SAFETY: `capture` was started; the buffer is read only between `GetBuffer` and
        // `ReleaseBuffer`, with the length the system gave for it.
        unsafe {
            while capture.GetNextPacketSize()? > 0 {
                let (mut data, mut count, mut flags) = (std::ptr::null_mut(), 0, 0);
                capture.GetBuffer(&raw mut data, &raw mut count, &raw mut flags, None, None)?;
                if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 {
                    frames.silence(count as usize * usize::from(CHANNELS), sink);
                } else {
                    // 16-bit PCM was asked for, so the buffer is aligned for it.
                    let samples = count as usize * usize::from(speakers.channels);
                    downmix(
                        std::slice::from_raw_parts(data.cast(), samples),
                        &shares,
                        &mut stereo,
                    );
                    frames.push(&stereo, sink);
                }
                capture.ReleaseBuffer(count)?;
                heard = Instant::now();
            }
        }
        let (samples, covered) = silence_due(heard.elapsed());
        if samples > 0 {
            frames.silence(samples, sink);
            heard += covered;
        }
    }
    Ok(())
}

/// Opens the target's sound and reads it. `ready` hears whether it opened.
fn run(
    target: Target,
    stop: &AtomicBool,
    ready: &mpsc::Sender<WinResult<()>>,
    sink: &mut dyn FnMut(&[i16]),
) -> WinResult<()> {
    let opened = (|| {
        let com = Com::init()?;
        let speakers = Speakers::of_default_output();
        let client = activate(target)?;
        let format = WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE,
                nChannels: speakers.channels,
                nSamplesPerSec: RATE,
                nAvgBytesPerSec: RATE * u32::from(speakers.channels) * 2,
                nBlockAlign: speakers.channels * 2,
                wBitsPerSample: 16,
                cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
            },
            Samples: WAVEFORMATEXTENSIBLE_0 {
                wValidBitsPerSample: 16,
            },
            dwChannelMask: speakers.mask,
            SubFormat: PCM,
        };
        // SAFETY: `format` lives through the call, which copies it. The rest are values.
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK
                    | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                BUFFER,
                0,
                (&raw const format).cast(),
                None,
            )?;
            let capture: IAudioCaptureClient = client.GetService()?;
            let event = CreateEventW(None, false, false, None)?;
            if let Err(e) = client.SetEventHandle(event).and_then(|()| client.Start()) {
                let _ = CloseHandle(event);
                return Err(e);
            }
            WinResult::Ok((com, client, capture, event, speakers))
        }
    })();
    let (_com, client, capture, event, speakers) = match opened {
        Ok(opened) => {
            let _ = ready.send(Ok(()));
            opened
        }
        Err(e) => {
            let _ = ready.send(Err(e.clone()));
            return Err(e);
        }
    };
    let result = pump(&capture, event, speakers, stop, sink);
    // SAFETY: the client was started and the event is ours; nothing uses them after this.
    unsafe {
        let _ = client.Stop();
        let _ = CloseHandle(event);
    }
    result
}

/// The target's sound going to `sink` in frames of 10 ms, on its own thread.
struct Loopback {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl Loopback {
    /// Blocks until the sound has opened, or has failed to.
    fn start(
        target: Target,
        mut sink: impl FnMut(&[i16]) + Send + 'static,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("oc-stream-sound".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || {
                    if let Err(e) = run(target, &stop, &ready_tx, &mut sink) {
                        eprintln!("[oc-stream] sound: {e}");
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        match ready.recv() {
            Ok(Ok(())) => Ok(Self { stop, thread }),
            Ok(Err(e)) => Err(e.message()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.thread.join();
    }
}

/// The sound of a stream, published in the room beside its picture.
pub struct StreamSound {
    capture: Loopback,
    room: Arc<Room>,
    track_sid: livekit::id::TrackSid,
}

impl StreamSound {
    /// Opens the sound that goes with the source and publishes it.
    ///
    /// # Errors
    /// No such sound here, or the room refused the track; as text.
    pub async fn start(room: Arc<Room>, kind: SourceKind, id: u64) -> Result<Self, String> {
        let target = Target::of(kind, id).ok_or("Окна больше нет")?;
        let source = new_source();
        let capture = tauri::async_runtime::spawn_blocking({
            let source = source.clone();
            move || Loopback::start(target, move |samples| send(&source, samples))
        })
        .await
        .map_err(|e| e.to_string())??;
        match publish(&room, source).await {
            Ok(track_sid) => Ok(Self {
                capture,
                room,
                track_sid,
            }),
            Err(e) => {
                capture.stop();
                Err(e)
            }
        }
    }

    /// Stops the capture and takes the track out of the room.
    pub async fn stop(self) {
        let capture = self.capture;
        let _ = tauri::async_runtime::spawn_blocking(move || capture.stop()).await;
        if let Err(e) = self
            .room
            .local_participant()
            .unpublish_track(&self.track_sid)
            .await
        {
            eprintln!("[oc-stream] sound unpublish: {e}");
        }
    }
}

/// A stereo source without queue: frames go straight on to the encoder, as the mic's do.
fn new_source() -> NativeAudioSource {
    // No processing: this is music and games, not a voice.
    let options = AudioSourceOptions {
        echo_cancellation: false,
        noise_suppression: false,
        auto_gain_control: false,
    };
    NativeAudioSource::new(options, RATE, u32::from(CHANNELS), 0)
}

fn send(source: &NativeAudioSource, samples: &[i16]) {
    let frame = AudioFrame {
        data: Cow::Borrowed(samples),
        sample_rate: RATE,
        num_channels: u32::from(CHANNELS),
        samples_per_channel: (samples.len() / usize::from(CHANNELS)) as u32,
    };
    // With queue 0 this completes synchronously; no runtime is needed to drive it.
    if let Err(e) = tauri::async_runtime::block_on(source.capture_frame(&frame)) {
        eprintln!("[oc-stream] sound send: {}", e.message);
    }
}

async fn publish(room: &Room, source: NativeAudioSource) -> Result<livekit::id::TrackSid, String> {
    let track = LocalAudioTrack::create_audio_track("screen-audio", RtcAudioSource::Native(source));
    let options = TrackPublishOptions {
        source: TrackSource::ScreenshareAudio,
        audio_encoding: Some(AudioEncoding {
            max_bitrate: BITRATE,
        }),
        // Nothing here is a voice: RED would double the rate for what a viewer's buffer
        // covers anyway, and silence is cheap in Opus's own VBR.
        dtx: false,
        red: false,
        ..TrackPublishOptions::default()
    };
    room.local_participant()
        .publish_track(LocalTrack::Audio(track), options)
        .await
        .map(|publication| publication.sid())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::audio::output;
    use crate::audio::SourceKey;
    use oc_audio::mixer::Layout;

    /// How much of `frequency` is in every `step`-th sample from `first`: the amplitude.
    pub(in crate::stream) fn tone_in(
        samples: &[i16],
        first: usize,
        step: usize,
        frequency: f64,
    ) -> f64 {
        let (mut re, mut im, mut n) = (0.0, 0.0, 0.0);
        for (i, sample) in samples.iter().skip(first).step_by(step).enumerate() {
            let phase = std::f64::consts::TAU * frequency * i as f64 / f64::from(RATE);
            re += f64::from(*sample) * phase.cos();
            im += f64::from(*sample) * phase.sin();
            n += 1.0;
        }
        2.0 * (re * re + im * im).sqrt() / f64::max(n, 1.0)
    }

    #[test]
    fn what_arrives_goes_out_in_frames_of_ten_ms() {
        let mut frames = Frames::default();
        let mut out = Vec::new();
        let mut sink = |frame: &[i16]| out.push(frame.to_vec());
        frames.push(&vec![1; FRAME - 2], &mut sink);
        frames.push(&[2; 4], &mut sink);
        frames.silence(FRAME, &mut sink);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|f| f.len() == FRAME));
        assert_eq!(&out[0][FRAME - 2..], &[2, 2], "in the order they came");
        assert_eq!(out[1][..2], [2, 2], "the rest comes first in the next one");
        assert_eq!(out[1][2], 0);
        assert_eq!(frames.pending.len(), 2, "part of a frame waits");
    }

    const SURROUND: u32 = 0x63F; // 7.1: FL FR FC LFE BL BR SL SR

    #[test]
    fn a_device_counts_as_surround_only_if_its_format_agrees() {
        let surround = Speakers {
            channels: 8,
            mask: SURROUND,
        };
        assert_eq!(Speakers::checked(8, SURROUND), surround);
        assert_eq!(Speakers::checked(2, STEREO_MASK), STEREO);
        assert_eq!(Speakers::checked(1, 0x4), STEREO, "mono goes out as stereo");
        assert_eq!(
            Speakers::checked(8, 0),
            STEREO,
            "no mask, no idea which is which"
        );
        assert_eq!(Speakers::checked(6, SURROUND), STEREO, "they disagree");
    }

    fn down(mask: u32, samples: &[i16]) -> Vec<i16> {
        let speakers = Speakers {
            channels: mask.count_ones() as u16,
            mask,
        };
        let mut stereo = Vec::new();
        downmix(samples, &speakers.shares(), &mut stereo);
        stereo
    }

    #[test]
    fn stereo_on_a_surround_device_keeps_its_level() {
        assert_eq!(
            down(SURROUND, &[1000, -2000, 0, 0, 0, 0, 0, 0]),
            [1000, -2000]
        );
    }

    #[test]
    fn the_other_channels_go_to_their_sides_and_the_low_ones_nowhere() {
        // Only front centre: both sides at -3 dB.
        assert_eq!(down(SURROUND, &[0, 0, 1000, 0, 0, 0, 0, 0]), [707, 707]);
        assert_eq!(down(SURROUND, &[0, 0, 0, 1000, 0, 0, 0, 0]), [0, 0], "LFE");
        assert_eq!(down(SURROUND, &[0, 0, 0, 0, 1000, 0, 0, 0]), [707, 0], "BL");
        assert_eq!(down(SURROUND, &[0, 0, 0, 0, 0, 1000, 0, 0]), [0, 707], "BR");
        assert_eq!(down(SURROUND, &[0, 0, 0, 0, 0, 0, 1000, 0]), [707, 0], "SL");
        assert_eq!(down(SURROUND, &[0, 0, 0, 0, 0, 0, 0, 1000]), [0, 707], "SR");
    }

    #[test]
    fn a_loud_surround_mix_clips_instead_of_wrapping_around() {
        let loud = [i16::MAX; 8];
        assert_eq!(down(SURROUND, &loud), [i16::MAX, i16::MAX]);
        let low = [i16::MIN; 8];
        assert_eq!(down(SURROUND, &low), [i16::MIN, i16::MIN]);
    }

    #[test]
    fn silence_is_owed_only_after_a_real_pause() {
        assert_eq!(silence_due(Duration::from_millis(29)).0, 0);
        assert_eq!(
            silence_due(Duration::from_millis(35)),
            (3 * FRAME, Duration::from_millis(30)),
            "whole frames; the rest stays owed",
        );
    }

    #[test]
    fn a_screen_leaves_this_process_out_and_a_closed_window_has_no_sound() {
        assert_eq!(
            Target::of(SourceKind::Screen, 0),
            Some(Target::AllBut(std::process::id()))
        );
        assert_eq!(Target::of(SourceKind::Window, 1), None);
    }

    fn listen(target: Target, seconds: u64) -> Vec<i16> {
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&heard);
        let capture = Loopback::start(target, move |samples| {
            sink.lock().unwrap().extend_from_slice(samples);
        })
        .unwrap();
        std::thread::sleep(Duration::from_secs(seconds));
        capture.stop();
        let samples = heard.lock().unwrap().clone();
        samples
    }

    /// Plays a tone too quiet to notice (-50 dBFS) on the real output. Its level must come
    /// through whatever the device's format: this is what catches a surround device's loss.
    #[test]
    #[ignore = "needs a Windows desktop with an output device"]
    fn loopback_hears_the_chosen_process_and_leaves_out_the_other() {
        const TONE: f64 = 997.0;
        let mut opened =
            output::open::<SourceKey>(None, &output::DeviceLatency::default(), &Arc::default())
                .unwrap();
        let rate = f64::from(opened.output.sample_rate());
        let mut input = opened
            .mixer
            .add_source(SourceKey::Stream, Layout::Mono, 1.0)
            .unwrap();
        let playing = Arc::new(AtomicBool::new(true));
        let player = std::thread::spawn({
            let playing = Arc::clone(&playing);
            move || {
                let mut n = 0.0_f64;
                let mut block = vec![0.0_f32; (rate / 100.0) as usize];
                // By the clock: a plain sleep of 10 ms takes longer, and the tone would break.
                let mut next = Instant::now();
                while playing.load(Ordering::Relaxed) {
                    for s in &mut block {
                        *s = 0.003 * (std::f64::consts::TAU * TONE * n / rate).sin() as f32;
                        n += 1.0;
                    }
                    input.write(&block);
                    next += Duration::from_millis(10);
                    std::thread::sleep(next.saturating_duration_since(Instant::now()));
                }
            }
        });
        std::thread::sleep(Duration::from_millis(300));

        let process = std::process::id();
        let only = listen(Target::Only(process), 2);
        let all_but = listen(Target::AllBut(process), 2);
        playing.store(false, Ordering::Relaxed);
        player.join().unwrap();
        let (left, right) = (tone_in(&only, 0, 2, TONE), tone_in(&only, 1, 2, TONE));
        let other = tone_in(&all_but, 0, 2, TONE);
        eprintln!("tone: {left:.1} / {right:.1}, without us: {other:.1}");
        assert!(only.len() >= 150 * FRAME, "got {} samples", only.len());
        assert!(all_but.len() >= 150 * FRAME, "silence still counts");
        assert!(left > 40.0, "our own sound, heard: {left}");
        assert!(right > 40.0, "in both channels: {right}");
        assert!(other < left / 4.0, "left out: {other} against {left}");
    }

    /// For the room's test: the sound path without a capture.
    pub(in crate::stream) fn test_source() -> NativeAudioSource {
        new_source()
    }

    pub(in crate::stream) fn test_send(source: &NativeAudioSource, samples: &[i16]) {
        send(source, samples);
    }

    pub(in crate::stream) async fn test_publish(
        room: &Room,
        source: NativeAudioSource,
    ) -> livekit::id::TrackSid {
        publish(room, source).await.unwrap()
    }

    pub(in crate::stream) const TEST_FRAME: usize = FRAME;
    pub(in crate::stream) const TEST_RATE: f64 = RATE as f64;
}
