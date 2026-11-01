//! From a screen to the room, on the stream's own thread: frames as I420 at the chosen
//! size, NV12 for the encoder, encoded frames to the SDK. The encoder is tied to that
//! thread (COM); only the capture callback and the published source cross it.
//!
//! A second, light layer has a thread of its own and works only while a viewer asks for it
//! (ADR-0065): a slow encoder there must not cost the full layer its frames.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use livekit::options::{
    DegradationPreference, TrackPublishOptions, VideoCodec, VideoEncoderBackend, VideoEncoding,
};
use livekit::publication::LocalTrackPublication;
use livekit::track::{LocalTrack, LocalVideoTrack};
use livekit::webrtc::desktop_capturer::{
    CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions, DesktopFrame,
};
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::{
    EncodedFrameType, EncodedVideoCodec, EncodedVideoFrame, I420Buffer, VideoBuffer,
};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use livekit::{Room, RoomEvent};
use tokio::sync::{oneshot, Notify};
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow};

use super::mf::{EncoderChoice, EncoderSettings, H264Encoder};
use super::sound::{Com, StreamSound};
use super::wgc::{Picture, WindowCapture};
use super::{
    bitrate, light_bitrate, light_quality, SourceKind, StreamQuality, StreamSource, ASK_LIGHT,
    FULL_TRACK, LIGHT_TRACK, STREAM_SOURCE,
};
use crate::voice::room::AbortOnDrop;

/// The first frame must come within this, or the source is not there to capture.
const FIRST_FRAME: Duration = Duration::from_secs(5);
/// `WebRTC` may ask for little on a bad link; below this the picture is mush anyway.
const MIN_BITRATE: u32 = 300_000;
/// Nobody has asked for the light layer for this long: its encoder is let go.
const LIGHT_IDLE: Duration = Duration::from_secs(10);
/// From a layer's first frame to when the connection has noticed it.
const VIDEO_SETTLES: Duration = Duration::from_millis(200);

/// Bits per second for a picture of `height` at this quality: a 1080p screen under the
/// choice of 1440p, or a small window, needs no more than its own size asks (ADR-0054).
const fn rate_for(quality: StreamQuality, height: u32) -> u32 {
    let row = match height {
        ..=720 => 720,
        721..=1080 => 1080,
        _ => 1440,
    };
    bitrate(StreamQuality {
        height: if row < quality.height {
            row
        } else {
            quality.height
        },
        fps: quality.fps,
    })
}

/// The source's size brought down to fit the quality: proportions kept, never enlarged,
/// even numbers (the encoder's chroma planes are half the size).
fn fit(width: u32, height: u32, quality: StreamQuality) -> (u32, u32) {
    let (box_w, box_h) = (quality.height * 16 / 9, quality.height);
    let scale = (f64::from(box_w) / f64::from(width))
        .min(f64::from(box_h) / f64::from(height))
        .min(1.0);
    let even = |v: f64| ((v as u32) & !1).max(2);
    (
        even(f64::from(width) * scale),
        even(f64::from(height) * scale),
    )
}

/// Where frames come from: the screen, or a test's generator.
trait Frames {
    /// The newest frame at the chosen size, waiting for one at most `wait`; `None` when
    /// none came.
    fn next(&mut self, wait: Duration) -> Option<I420Buffer>;
    /// The source will never give another frame: its window was closed.
    fn gone(&self) -> bool {
        false
    }
}

/// A frame from the capturer, at the chosen size.
fn to_i420(frame: &DesktopFrame, quality: StreamQuality) -> Option<I420Buffer> {
    picture_to_i420(
        &Picture {
            data: frame.data(),
            stride: frame.stride().cast_signed(),
            width: frame.width(),
            height: frame.height(),
        },
        quality,
    )
}

/// A picture in memory, at the chosen size.
fn picture_to_i420(picture: &Picture<'_>, quality: StreamQuality) -> Option<I420Buffer> {
    let (width, height) = (picture.width, picture.height);
    if width < 2 || height < 2 {
        return None;
    }
    let mut buffer = I420Buffer::new(width as u32, height as u32);
    let (stride_y, stride_u, stride_v) = buffer.strides();
    let (y, u, v) = buffer.data_mut();
    // What libyuv calls ARGB is BGRA in memory: what the capturers give.
    yuv_helper::argb_to_i420(
        picture.data,
        picture.stride.cast_unsigned(),
        y,
        stride_y,
        u,
        stride_u,
        v,
        stride_v,
        width,
        height,
    );
    let (to_w, to_h) = fit(width as u32, height as u32, quality);
    Some(if (to_w, to_h) == (width as u32, height as u32) {
        buffer
    } else {
        buffer.scale(to_w.cast_signed(), to_h.cast_signed())
    })
}

/// The screen or window, captured on a thread of its own: a capture call can take a whole
/// frame's time, more when a game keeps the video card busy, and the encoder must not wait
/// through that (ADR-0058).
struct Desktop {
    frames: mpsc::Receiver<I420Buffer>,
    gone: Arc<AtomicBool>,
    /// The captured window, when it is one: a source's id on Windows is its handle.
    window: Option<HWND>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Desktop {
    fn open(kind: SourceKind, id: u64, quality: StreamQuality) -> Result<Self, String> {
        let (sender, frames) = mpsc::channel();
        let gone = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (opened_tx, opened) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("oc-stream-capture".into())
            .spawn({
                let (gone, stop) = (Arc::clone(&gone), Arc::clone(&stop));
                move || capture_thread(kind, id, quality, sender, gone, &stop, &opened_tx)
            })
            .map_err(|e| format!("Не удалось начать стрим: {e}"))?;
        opened
            .recv()
            .unwrap_or_else(|_| Err("Захват остановился при запуске".to_owned()))?;
        Ok(Self {
            frames,
            gone,
            window: (kind == SourceKind::Window).then_some(HWND(id as *mut _)),
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Captures until told to stop: a window directly (ADR-0060), a screen, or a window where that
/// did not work, through the SDK.
fn capture_thread(
    kind: SourceKind,
    id: u64,
    quality: StreamQuality,
    sender: mpsc::Sender<I420Buffer>,
    gone: Arc<AtomicBool>,
    stop: &AtomicBool,
    opened: &mpsc::Sender<Result<(), String>>,
) {
    let tick = Duration::from_secs_f64(1.0 / f64::from(quality.fps));
    if kind == SourceKind::Window {
        match Com::init().and_then(|com| Ok((com, WindowCapture::open(HWND(id as *mut _), tick)?)))
        {
            Ok((_com, mut capture)) => {
                let _ = opened.send(Ok(()));
                let result = capture.run(stop, |picture| {
                    if let Some(buffer) = picture_to_i420(picture, quality) {
                        let _ = sender.send(buffer);
                    }
                });
                if let Err(e) = result {
                    eprintln!("[oc-stream] capture: {e}");
                    gone.store(true, Ordering::Relaxed);
                }
                return;
            }
            Err(e) => eprintln!("[oc-stream] direct capture: {e}; through the SDK"),
        }
    }
    let Some(mut capturer) = capturer(kind) else {
        let _ = opened.send(Err("Захват экрана на этом компьютере недоступен".to_owned()));
        return;
    };
    let Some(source) = capturer
        .get_source_list()
        .into_iter()
        .find(|s| s.id() == id)
    else {
        let _ = opened.send(Err("Этого экрана или окна больше нет".to_owned()));
        return;
    };
    capturer.start_capture(Some(source), move |result| match result {
        Ok(frame) => {
            if let Some(buffer) = to_i420(&frame, quality) {
                let _ = sender.send(buffer);
            }
        }
        // A failed frame is skipped: the next call asks again, and the stream stops by
        // itself when frames stay away at the start.
        Err(CaptureError::Temporary) => {}
        Err(CaptureError::Permanent) => gone.store(true, Ordering::Relaxed),
    });
    let _ = opened.send(Ok(()));
    let mut next = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        capturer.capture_frame();
        next += tick;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}

impl Frames for Desktop {
    /// Only the newest: frames that came while the encoder was busy are already old.
    fn next(&mut self, wait: Duration) -> Option<I420Buffer> {
        let first = self.frames.recv_timeout(wait).ok()?;
        Some(self.frames.try_iter().last().unwrap_or(first))
    }

    /// The capturer's word, and the window's own: on a closed window the capturer may only
    /// go on with temporary errors (ADR-0054). Not the list of sources: it also leaves out
    /// a window that is merely hidden or on another virtual desktop.
    fn gone(&self) -> bool {
        // SAFETY: `IsWindow` takes any value and only answers whether it is a window.
        self.gone.load(Ordering::Relaxed)
            || self
                .window
                .is_some_and(|w| !unsafe { IsWindow(Some(w)) }.as_bool())
    }
}

fn capturer(kind: SourceKind) -> Option<DesktopCapturer> {
    let source = match kind {
        SourceKind::Screen => DesktopCaptureSourceType::Screen,
        SourceKind::Window => DesktopCaptureSourceType::Window,
    };
    let mut options = DesktopCapturerOptions::new(source);
    options.set_include_cursor(true);
    DesktopCapturer::new(options)
}

/// The program file of a process, or `None` if the system will not say.
fn program_of(process: u32) -> Option<String> {
    // SAFETY: the handle is checked by `?`, used for this one question and closed here; the
    // buffer outlives the call, and `size` tells the system how long it is.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process).ok()?;
        let mut path = [0u16; 1024];
        let mut size = path.len() as u32;
        let asked = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &raw mut size,
        );
        let _ = CloseHandle(handle);
        asked.ok()?;
        Some(String::from_utf16_lossy(&path[..size as usize]))
    }
}

/// The client's or the server window's program: the names cargo gives them (ADR-0059).
fn is_ourcamp_program(path: &str) -> bool {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    ["ourcamp-desktop.exe", "ourcamp-server-app.exe"]
        .iter()
        .any(|name| file.eq_ignore_ascii_case(name))
}

/// A window of OurCamp, this copy's or another's: libwebrtc leaves out only this process's.
fn is_ourcamp_window(id: u64) -> bool {
    let mut process = 0;
    // SAFETY: any value is safe to pass as a handle; a source's id on Windows is its window's
    // handle, and `process` outlives the call.
    unsafe { GetWindowThreadProcessId(HWND(id as *mut _), Some(&raw mut process)) };
    process != 0 && program_of(process).is_some_and(|path| is_ourcamp_program(&path))
}

/// The screens and windows that can be streamed. OurCamp's own windows are not among them:
/// whoever wants to show one shows the whole screen (ADR-0059).
#[must_use]
pub fn list_sources() -> Vec<StreamSource> {
    [SourceKind::Screen, SourceKind::Window]
        .into_iter()
        .flat_map(|kind| {
            capturer(kind)
                .map(|c| c.get_source_list())
                .unwrap_or_default()
                .into_iter()
                .filter(move |s| kind == SourceKind::Screen || !is_ourcamp_window(s.id()))
                .map(move |s| StreamSource {
                    kind,
                    id: s.id(),
                    title: s.title(),
                })
        })
        .collect()
}

/// What the stream's thread says when it is ready to go.
struct Ready {
    source: NativeVideoSource,
    light: NativeVideoSource,
    encoder: String,
}

/// One video track of the stream, as the room should know it.
fn stream_options(max_bitrate: u32, fps: u32) -> TrackPublishOptions {
    TrackPublishOptions {
        source: STREAM_SOURCE,
        video_codec: VideoCodec::H264,
        // A pre-encoded source: `WebRTC` only packetizes what we send.
        video_encoder: VideoEncoderBackend::PreEncoded,
        simulcast: false,
        video_encoding: Some(VideoEncoding {
            max_bitrate: u64::from(max_bitrate),
            max_framerate: f64::from(fps),
        }),
        // Not a camera: a sharp picture matters more than a smooth one.
        degradation_preference: Some(DegradationPreference::MaintainResolution),
        ..TrackPublishOptions::default()
    }
}

/// Puts one video track of the stream in the room.
async fn publish_layer(
    room: &Room,
    name: &str,
    source: NativeVideoSource,
    max_bitrate: u32,
    fps: u32,
) -> Result<(LocalTrackPublication, LocalVideoTrack), String> {
    let track = LocalVideoTrack::create_video_track(name, RtcVideoSource::Native(source));
    let publication = room
        .local_participant()
        .publish_track(
            LocalTrack::Video(track.clone()),
            stream_options(max_bitrate, fps),
        )
        .await
        .map_err(|e| format!("Не удалось начать стрим: {e}"))?;
    Ok((publication, track))
}

/// Puts the light layer in the room, stopped until `follow_asks` finds someone asking.
async fn publish_light(
    room: &Room,
    source: NativeVideoSource,
    quality: StreamQuality,
) -> Result<(LocalTrackPublication, LocalVideoTrack), String> {
    let light = light_quality(quality);
    let published =
        publish_layer(room, LIGHT_TRACK, source, light_bitrate(quality), light.fps).await?;
    if let Err(e) = published.1.set_sending(false) {
        eprintln!("[oc-stream] light layer: {e}");
    }
    Ok(published)
}

/// Keeps `wanted` true while any viewer in the room asks for the light layer, with its sender on
/// for just that long, so that unasked it takes nothing from the start of the full one
/// (ADR-0068). Also helps
/// the connection over the start of either layer: `WebRTC` throws its estimate of the link away when
/// a video starts sending, and with a stream that uses all it is given only a probe brings it
/// back soon. Without it the full layer spends half a minute climbing from 1 Mbit/s (ADR-0065).
async fn follow_asks(
    room: Arc<Room>,
    light: LocalVideoTrack,
    wanted: Arc<AtomicBool>,
    video_began: Arc<Notify>,
) {
    let mut events = room.subscribe();
    let me = room.local_participant().identity();
    let look = || {
        let asked = room.remote_participants().values().any(|p| {
            p.attributes()
                .get(ASK_LIGHT)
                .is_some_and(|whose| whose == me.as_str())
        });
        if asked == wanted.load(Ordering::Relaxed) {
            return;
        }
        // The sender first: the first frames of the layer must not fall on a stopped one.
        if let Err(e) = light.set_sending(asked) {
            eprintln!("[oc-stream] light layer: {e}");
        }
        wanted.store(asked, Ordering::Relaxed);
        if asked {
            // Its encoder may still be up, and then its thread sees no start.
            video_began.notify_one();
        }
    };
    look();
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(
                    RoomEvent::ParticipantAttributesChanged { .. }
                    | RoomEvent::ParticipantConnected(_)
                    | RoomEvent::ParticipantDisconnected(_),
                ) => look(),
                Some(_) => {}
                None => return,
            },
            () = video_began.notified() => {
                // The estimate goes just after the first packets, not with the first frame.
                tokio::time::sleep(VIDEO_SETTLES).await;
                room.local_participant().probe_send_bandwidth().await;
            }
        }
    }
}

/// A running stream in the room.
pub struct ScreenStream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    room: Arc<Room>,
    track_sid: livekit::id::TrackSid,
    light_sid: livekit::id::TrackSid,
    /// Whether anyone asks for the light layer, kept up to date for as long as it lives.
    _asks: AbortOnDrop,
    ended: Option<oneshot::Receiver<()>>,
    sound: Option<StreamSound>,
    sound_problem: Option<String>,
}

impl ScreenStream {
    /// Starts streaming a screen or window into `room`, with its `sound` if asked. Without
    /// sound is still a stream: see [`Self::sound_problem`].
    ///
    /// # Errors
    /// The source is gone, no encoder works, or the room refused the track (forbidden to
    /// speak, ADR-0028); as text for the UI.
    pub async fn start(
        room: Arc<Room>,
        kind: SourceKind,
        id: u64,
        quality: StreamQuality,
        sound: bool,
    ) -> Result<Self, String> {
        let mut stream = Self::run(room, quality, EncoderChoice::Best, move || {
            Ok(Box::new(Desktop::open(kind, id, quality)?) as Box<dyn Frames>)
        })
        .await?;
        if sound {
            match StreamSound::start(Arc::clone(&stream.room), kind, id).await {
                Ok(sound) => stream.sound = Some(sound),
                Err(e) => {
                    eprintln!("[oc-stream] sound: {e}");
                    stream.sound_problem = Some(format!("Стрим идёт без звука: {e}"));
                }
            }
        }
        Ok(stream)
    }

    /// Why the sound that was asked for did not start.
    pub fn sound_problem(&self) -> Option<String> {
        self.sound_problem.clone()
    }

    async fn run(
        room: Arc<Room>,
        quality: StreamQuality,
        choice: EncoderChoice,
        open: impl FnOnce() -> Result<Box<dyn Frames>, String> + Send + 'static,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let go = Arc::new(AtomicBool::new(false));
        let wanted = Arc::new(AtomicBool::new(false));
        let video_began = Arc::new(Notify::new());
        let (ready_tx, ready_rx) = oneshot::channel();
        let (ended_tx, ended) = oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("oc-stream".into())
            .spawn({
                let (stop, go, wanted, video_began) = (
                    Arc::clone(&stop),
                    Arc::clone(&go),
                    Arc::clone(&wanted),
                    Arc::clone(&video_began),
                );
                move || {
                    // Dropped when the thread ends, whatever ended it.
                    let _ended = ended_tx;
                    stream_thread(
                        open,
                        quality,
                        choice,
                        &stop,
                        &go,
                        &wanted,
                        &video_began,
                        ready_tx,
                    );
                }
            })
            .map_err(|e| format!("Не удалось начать стрим: {e}"))?;
        let halt = |thread: JoinHandle<()>, stop: &AtomicBool| {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
        };
        let ready = match ready_rx.await {
            Ok(Ok(ready)) => ready,
            Ok(Err(e)) => {
                halt(thread, &stop);
                return Err(e);
            }
            Err(_) => {
                halt(thread, &stop);
                return Err("Стрим остановился при запуске".to_owned());
            }
        };
        eprintln!("[oc-stream] encoder: {}", ready.encoder);
        // The light one first: the last video published sets the connection's send cap,
        // and it must be the full one's (ADR-0062).
        let (light_publication, light_track) =
            match publish_light(&room, ready.light, quality).await {
                Ok(published) => published,
                Err(e) => {
                    halt(thread, &stop);
                    return Err(e);
                }
            };
        let (publication, _) = match publish_layer(
            &room,
            FULL_TRACK,
            ready.source,
            bitrate(quality),
            quality.fps,
        )
        .await
        {
            Ok(publication) => publication,
            Err(e) => {
                halt(thread, &stop);
                let _ = room
                    .local_participant()
                    .unpublish_track(&light_publication.sid())
                    .await;
                return Err(e);
            }
        };
        go.store(true, Ordering::Relaxed);
        let asks = AbortOnDrop(tauri::async_runtime::spawn(follow_asks(
            Arc::clone(&room),
            light_track,
            wanted,
            video_began,
        )));
        Ok(Self {
            stop,
            thread: Some(thread),
            room,
            track_sid: publication.sid(),
            light_sid: light_publication.sid(),
            _asks: asks,
            ended: Some(ended),
            sound: None,
            sound_problem: None,
        })
    }

    /// Resolves, once, when the stream's thread is over: told to stop, or by itself when
    /// the source is gone or the encoder gave up. The track stays until [`Self::stop`].
    pub fn ended(&mut self) -> oneshot::Receiver<()> {
        self.ended.take().unwrap_or_else(|| oneshot::channel().1)
    }

    /// Stops the stream and takes it out of the room.
    pub async fn stop(mut self) {
        if let Some(sound) = self.sound.take() {
            sound.stop().await;
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = tauri::async_runtime::spawn_blocking(move || thread.join()).await;
        }
        for sid in [&self.track_sid, &self.light_sid] {
            if let Err(e) = self.room.local_participant().unpublish_track(sid).await {
                eprintln!("[oc-stream] unpublish: {e}");
            }
        }
    }
}

/// The encoder and what it feeds. Remade when a window changes size.
struct Encoding {
    encoder: H264Encoder,
    width: u32,
    height: u32,
    bitrate: u32,
    nv12: Vec<u8>,
}

impl Encoding {
    fn new(
        width: u32,
        height: u32,
        fps: u32,
        bitrate: u32,
        choice: EncoderChoice,
    ) -> Result<Self, String> {
        let encoder = H264Encoder::new(
            EncoderSettings {
                width,
                height,
                fps,
                bitrate,
            },
            choice,
        )?;
        Ok(Self {
            encoder,
            width,
            height,
            bitrate,
            nv12: vec![0; (width * height * 3 / 2) as usize],
        })
    }

    /// The buffer's picture in NV12: what encoders take.
    fn fill(&mut self, buffer: &I420Buffer) {
        let (stride_y, stride_u, stride_v) = buffer.strides();
        let (y, u, v) = buffer.data();
        let (dst_y, dst_uv) = self.nv12.split_at_mut((self.width * self.height) as usize);
        yuv_helper::i420_to_nv12(
            y,
            stride_y,
            u,
            stride_u,
            v,
            stride_v,
            dst_y,
            self.width,
            dst_uv,
            self.width,
            self.width.cast_signed(),
            self.height.cast_signed(),
        );
    }

    /// One picture encoded and on its way. `key` is the wish for a key frame, until met.
    fn send(
        &mut self,
        frame: &I420Buffer,
        source: &NativeVideoSource,
        started: Instant,
        key: &mut bool,
    ) -> Result<(), String> {
        *key |= source.take_keyframe_request();
        if let Some(rate) = source.take_rate_control_request() {
            let target =
                (rate.target_bitrate_bps.min(u64::from(self.bitrate)) as u32).max(MIN_BITRATE);
            self.encoder.set_bitrate(target);
        }
        self.fill(frame);
        let timestamp = started.elapsed().as_micros() as i64;
        let encoded = self.encoder.encode(&self.nv12, timestamp, *key)?;
        *key = false;
        for unit in encoded {
            source.capture_encoded_frame(&EncodedVideoFrame {
                codec: EncodedVideoCodec::H264,
                payload: &unit.data,
                timestamp_us: unit.timestamp_us,
                frame_type: if unit.key {
                    EncodedFrameType::Key
                } else {
                    EncodedFrameType::Delta
                },
                resolution: VideoResolution {
                    width: self.width,
                    height: self.height,
                },
                frame_metadata: None,
            });
        }
        Ok(())
    }
}

/// The light layer's thread: encodes what it is given, and lets the encoder go when nothing
/// has come for a while. Ends when the sender does.
fn light_thread(
    pictures: &mpsc::Receiver<I420Buffer>,
    source: &NativeVideoSource,
    quality: StreamQuality,
    choice: EncoderChoice,
    started: Instant,
    video_began: &Notify,
) {
    let light = light_quality(quality);
    let mut encoding: Option<Encoding> = None;
    let mut key = true;
    // Long quiet makes `WebRTC` stop the video, and the next frame is a start again.
    let mut quiet = true;
    loop {
        let frame = match pictures.recv_timeout(LIGHT_IDLE) {
            Ok(frame) => frame,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                encoding = None;
                quiet = true;
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let size = (frame.width(), frame.height());
        let current = match encoding.take() {
            Some(encoding) if (encoding.width, encoding.height) == size => Ok(encoding),
            _ => {
                // Starting, or a window changed size.
                key = true;
                Encoding::new(size.0, size.1, light.fps, light_bitrate(quality), choice)
            }
        };
        let sent = current.and_then(|mut current| {
            current.send(&frame, source, started, &mut key)?;
            Ok(current)
        });
        match sent {
            Ok(current) => {
                encoding = Some(current);
                if std::mem::take(&mut quiet) {
                    video_began.notify_one();
                }
            }
            // The full layer goes on: the viewers who need this one see it stop.
            Err(e) => {
                eprintln!("[oc-stream] light layer: {e}");
                return;
            }
        }
    }
}

/// Hands pictures over to the light layer's thread, when someone wants them.
struct LightFeed<'a> {
    wanted: &'a AtomicBool,
    pictures: mpsc::SyncSender<I420Buffer>,
    quality: StreamQuality,
    /// Every how many of the full layer's frames goes one.
    every: u32,
    count: u32,
}

impl LightFeed<'_> {
    fn offer(&mut self, frame: &mut I420Buffer) {
        if !self.wanted.load(Ordering::Relaxed) {
            return;
        }
        self.count = (self.count + 1) % self.every;
        if self.count == 0 {
            let (width, height) = fit(frame.width(), frame.height(), self.quality);
            // Busy means behind: this picture is simply not needed.
            let _ = self
                .pictures
                .try_send(frame.scale(width.cast_signed(), height.cast_signed()));
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one call, in `ScreenStream::run`"
)]
fn stream_thread(
    open: impl FnOnce() -> Result<Box<dyn Frames>, String>,
    quality: StreamQuality,
    choice: EncoderChoice,
    stop: &AtomicBool,
    go: &AtomicBool,
    wanted: &AtomicBool,
    video_began: &Arc<Notify>,
    ready: oneshot::Sender<Result<Ready, String>>,
) {
    let tick = Duration::from_secs_f64(1.0 / f64::from(quality.fps));
    let mut frames = match open() {
        Ok(frames) => frames,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    // The size is known from the first frame, and the source is made for that size.
    let started = Instant::now();
    let first = loop {
        if let Some(frame) = frames.next(tick) {
            break frame;
        }
        if stop.load(Ordering::Relaxed) || started.elapsed() > FIRST_FRAME {
            let _ = ready.send(Err("Этот экран или окно не отдаёт кадры".to_owned()));
            return;
        }
        std::thread::sleep(tick);
    };
    let (width, height) = (first.width(), first.height());
    let encoding = match Encoding::new(
        width,
        height,
        quality.fps,
        rate_for(quality, height),
        choice,
    ) {
        Ok(encoding) => encoding,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let source = NativeVideoSource::new_encoded(VideoResolution { width, height });
    let light = light_quality(quality);
    let (light_width, light_height) = fit(width, height, light);
    let light_source = NativeVideoSource::new_encoded(VideoResolution {
        width: light_width,
        height: light_height,
    });
    let name = encoding.encoder.name().to_owned();
    if ready
        .send(Ok(Ready {
            source: source.clone(),
            light: light_source.clone(),
            encoder: name,
        }))
        .is_err()
    {
        return;
    }
    // Sent before the track is up, a frame would be lost, and a key frame with it.
    while !go.load(Ordering::Relaxed) {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let started = Instant::now();
    let (pictures, light_pictures) = mpsc::sync_channel(1);
    let light_began = Arc::clone(video_began);
    let light_worker = std::thread::Builder::new()
        .name("oc-stream-light".into())
        .spawn(move || {
            light_thread(
                &light_pictures,
                &light_source,
                quality,
                choice,
                started,
                &light_began,
            );
        });
    let feed = LightFeed {
        wanted,
        pictures,
        quality: light,
        every: (quality.fps / light.fps).max(1),
        count: 0,
    };
    pump(
        frames.as_mut(),
        Some(first),
        encoding,
        &source,
        quality,
        choice,
        stop,
        started,
        feed,
        video_began,
    );
    // The sender is gone with the feed, and that ends the thread.
    match light_worker {
        Ok(worker) => {
            let _ = worker.join();
        }
        Err(e) => eprintln!("[oc-stream] light layer: {e}"),
    }
}

/// Sends frames until told to stop, at the chosen pace.
#[allow(clippy::too_many_arguments, reason = "one call, right above")]
fn pump(
    frames: &mut dyn Frames,
    mut current: Option<I420Buffer>,
    mut encoding: Encoding,
    source: &NativeVideoSource,
    quality: StreamQuality,
    choice: EncoderChoice,
    stop: &AtomicBool,
    started: Instant,
    mut light: LightFeed<'_>,
    began: &Notify,
) {
    let tick = Duration::from_secs_f64(1.0 / f64::from(quality.fps));
    let mut key = true;
    let mut first = true;
    let mut next = Instant::now();
    // A window that does not change gives no frames, and the stream goes on all the same: for
    // a viewer's key frame and for the fps on their panel, as with the SDK's capturer.
    let mut last: Option<I420Buffer> = None;
    while !stop.load(Ordering::Relaxed) {
        if frames.gone() {
            eprintln!("[oc-stream] the source is gone");
            return;
        }
        if let Some(frame) = current.take().or_else(|| frames.next(tick)) {
            last = Some(frame);
        }
        if let Some(frame) = &mut last {
            if (frame.width(), frame.height()) != (encoding.width, encoding.height) {
                // A window changed size: a new encoder, starting with a key frame.
                match Encoding::new(
                    frame.width(),
                    frame.height(),
                    quality.fps,
                    rate_for(quality, frame.height()),
                    choice,
                ) {
                    Ok(fresh) => encoding = fresh,
                    Err(e) => {
                        eprintln!("[oc-stream] {e}");
                        return;
                    }
                }
                key = true;
            }
            if let Err(e) = encoding.send(frame, source, started, &mut key) {
                eprintln!("[oc-stream] {e}");
                return;
            }
            if std::mem::take(&mut first) {
                began.notify_one();
            }
            light.offer(frame);
        }
        next += tick;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            // Behind: drop the debt, do not rush to catch up.
            next = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn q(height: u32, fps: u32) -> StreamQuality {
        StreamQuality { height, fps }
    }

    #[test]
    fn our_programs_are_known_by_their_files() {
        assert!(is_ourcamp_program(
            r"C:\Program Files\OurCamp\ourcamp-desktop.exe"
        ));
        assert!(is_ourcamp_program(r"D:\a\OURCAMP-SERVER-APP.EXE"));
        assert!(is_ourcamp_program("ourcamp-desktop.exe"));
        assert!(!is_ourcamp_program(r"C:\Games\deadlock.exe"));
        assert!(!is_ourcamp_program(r"C:\ourcamp-desktop.exe\game.exe"));
        assert!(!is_ourcamp_program(r"C:\Games\not-ourcamp-desktop.exe"));
    }

    #[test]
    fn a_process_tells_its_program() {
        let own = program_of(std::process::id()).expect("our own program");
        assert!(own.to_lowercase().ends_with(".exe"), "{own}");
        assert_eq!(program_of(0), None, "no such process to ask");
    }

    #[test]
    fn something_that_is_no_window_is_not_ours() {
        assert!(!is_ourcamp_window(0));
    }

    #[test]
    fn the_picture_fits_the_quality_with_its_proportions() {
        assert_eq!(fit(2560, 1440, q(1080, 60)), (1920, 1080));
        assert_eq!(fit(2560, 1440, q(720, 30)), (1280, 720));
        assert_eq!(fit(1280, 720, q(1080, 60)), (1280, 720), "never enlarged");
        assert_eq!(
            fit(3440, 1440, q(1080, 60)),
            (1920, 802),
            "ultrawide: by its width, even"
        );
        assert_eq!(
            fit(1081, 801, q(1080, 60)),
            (1080, 800),
            "odd sizes come down to even"
        );
        assert_eq!(fit(3, 3, q(720, 30)), (2, 2));
    }

    #[test]
    fn quality_sets_the_bitrate() {
        assert_eq!(bitrate(q(1080, 60)), 20_000_000);
        assert_eq!(bitrate(q(1080, 30)), 12_000_000);
        assert_eq!(bitrate(q(720, 60)), 10_000_000);
        assert_eq!(bitrate(q(720, 30)), 6_000_000);
        assert_eq!(bitrate(q(1440, 60)), 40_000_000);
        assert_eq!(bitrate(q(1440, 30)), 20_000_000);
    }

    #[test]
    fn a_small_picture_takes_no_more_than_its_size_asks() {
        assert_eq!(rate_for(q(1440, 60), 1440), 40_000_000);
        assert_eq!(rate_for(q(1440, 60), 1080), 20_000_000, "a 1080p screen");
        assert_eq!(rate_for(q(1440, 30), 600), 6_000_000, "a small window");
        assert_eq!(rate_for(q(1080, 60), 1000), 20_000_000, "just under 1080");
        assert_eq!(rate_for(q(720, 60), 720), 10_000_000);
    }

    /// Moving stripes at a fixed size.
    struct Stripes {
        width: u32,
        height: u32,
        frame: u32,
    }

    impl Frames for Stripes {
        fn next(&mut self, _wait: Duration) -> Option<I420Buffer> {
            let mut buffer = I420Buffer::new(self.width, self.height);
            let (stride_y, _, _) = buffer.strides();
            let (y, u, v) = buffer.data_mut();
            for (row, line) in y.chunks_mut(stride_y as usize).enumerate() {
                for (x, pixel) in line.iter_mut().enumerate() {
                    *pixel = (((x + row + self.frame as usize * 8) / 16) % 2 * 160 + 40) as u8;
                }
            }
            u.fill(128);
            v.fill(128);
            self.frame += 1;
            Some(buffer)
        }
    }

    /// Stripes until the source closes, like a window being closed.
    struct Closing {
        stripes: Stripes,
        left: u32,
    }

    impl Frames for Closing {
        fn next(&mut self, _wait: Duration) -> Option<I420Buffer> {
            self.left = self.left.saturating_sub(1);
            self.stripes.next(Duration::ZERO)
        }

        fn gone(&self) -> bool {
            self.left == 0
        }
    }

    /// A streamer and a viewer in one room of a fresh camp.
    struct Audience {
        camp: oc_server::Running,
        dir: std::path::PathBuf,
        streamer: Arc<Room>,
        viewer: Room,
        events: tokio::sync::mpsc::UnboundedReceiver<livekit::RoomEvent>,
        /// Tracks that came while another was awaited.
        seen: Vec<(String, livekit::track::RemoteVideoTrack)>,
    }

    impl Audience {
        async fn gather(name: &str) -> Self {
            use crate::voice::room::tests::camp_room;
            use livekit::RoomOptions;
            use oc_camp_client::{CampClient, DeviceKey};

            let (camp, dir, params) = camp_room(name).await;
            let (streamer, _) = Room::connect(&params.url, &params.token, RoomOptions::default())
                .await
                .unwrap();
            let client = CampClient::new(&camp.links.join);
            let session = client
                .login(
                    &DeviceKey::generate(),
                    "Зритель",
                    Some(camp.links.join.token.as_str()),
                )
                .await
                .unwrap();
            let token = client.room_token(&session, "c0").await.unwrap().token;
            let (viewer, events) = Room::connect(&params.url, &token, RoomOptions::default())
                .await
                .unwrap();
            Self {
                camp,
                dir,
                streamer: Arc::new(streamer),
                viewer,
                events,
                seen: Vec::new(),
            }
        }

        /// The stream's track as the viewer gets it. The streamer's, by identity: every
        /// camp in these tests has its room `c0` on one LiveKit, where someone from another
        /// test, or from a run just ended, may still count as present with a dead track.
        async fn stream_track(&mut self) -> livekit::track::RemoteVideoTrack {
            self.track_named(FULL_TRACK).await
        }

        async fn track_named(&mut self, name: &str) -> livekit::track::RemoteVideoTrack {
            use livekit::track::RemoteTrack;
            let streamer = self.streamer.local_participant().identity();
            let seen = self
                .seen
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, t)| t.clone());
            if let Some(track) = seen {
                return track;
            }
            tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    if let Some(livekit::RoomEvent::TrackSubscribed {
                        track: RemoteTrack::Video(track),
                        publication,
                        participant,
                    }) = self.events.recv().await
                    {
                        if participant.identity() == streamer {
                            assert_eq!(publication.source(), STREAM_SOURCE);
                            if publication.name() == name {
                                return track;
                            }
                            self.seen.push((publication.name(), track));
                        }
                    }
                }
            })
            .await
            .expect("the viewer gets the stream's track")
        }

        async fn leave(self) {
            let _ = self.viewer.close().await;
            let _ = self.streamer.close().await;
            self.camp.stop().await;
            let _ = std::fs::remove_dir_all(self.dir);
        }
    }

    /// How many frames decode in `seconds`, and their size.
    async fn watch(track: &livekit::track::RemoteVideoTrack, seconds: u64) -> (u32, (u32, u32)) {
        use futures_util::StreamExt;
        use livekit::webrtc::video_stream::native::NativeVideoStream;

        let mut decoded = NativeVideoStream::new(track.rtc_track());
        let mut count = 0;
        let mut size = (0, 0);
        let _ = tokio::time::timeout(Duration::from_secs(seconds), async {
            while let Some(frame) = decoded.next().await {
                count += 1;
                size = (frame.buffer.width(), frame.buffer.height());
            }
        })
        .await;
        (count, size)
    }

    /// One picture and then nothing, like a window that does not change.
    struct Still {
        stripes: Stripes,
        given: bool,
    }

    impl Frames for Still {
        fn next(&mut self, wait: Duration) -> Option<I420Buffer> {
            if std::mem::replace(&mut self.given, true) {
                std::thread::sleep(wait);
                return None;
            }
            self.stripes.next(wait)
        }
    }

    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn a_picture_that_does_not_change_still_reaches_a_viewer() {
        tauri::async_runtime::block_on(async {
            let mut room = Audience::gather("stream-still").await;
            let stream = ScreenStream::run(
                Arc::clone(&room.streamer),
                q(720, 30),
                EncoderChoice::Best,
                || {
                    Ok(Box::new(Still {
                        stripes: Stripes {
                            width: 1280,
                            height: 720,
                            frame: 0,
                        },
                        given: false,
                    }) as Box<dyn Frames>)
                },
            )
            .await
            .unwrap();
            let track = room.stream_track().await;
            let _ = watch(&track, 1).await;
            let (count, size) = watch(&track, 3).await;
            assert_eq!(size, (1280, 720));
            assert!(count > 45, "near 30 a second, got {count}");
            stream.stop().await;
            room.leave().await;
        });
    }

    /// A viewer whose line cannot carry the stream asks, and the light layer starts; nobody
    /// asking, it stops, and the full layer goes on all the while (ADR-0065).
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn the_light_layer_comes_when_asked_and_stops_when_not() {
        use std::collections::HashMap;

        // Whether the light layer's sender is on.
        fn light_sends(room: &Audience) -> bool {
            let publications = room.streamer.local_participant().track_publications();
            let light = publications.values().find(|p| p.name() == LIGHT_TRACK);
            let Some(LocalTrack::Video(track)) = light.and_then(LocalTrackPublication::track)
            else {
                panic!("the light layer is published");
            };
            let layers = track.publishing_layers();
            assert_ne!(layers.len(), 0);
            layers.iter().all(|layer| layer.active)
        }

        tauri::async_runtime::block_on(async {
            let mut room = Audience::gather("stream-light").await;
            let stream = ScreenStream::run(
                Arc::clone(&room.streamer),
                q(720, 60),
                EncoderChoice::Best,
                || {
                    Ok(Box::new(Stripes {
                        width: 1280,
                        height: 720,
                        frame: 0,
                    }) as Box<dyn Frames>)
                },
            )
            .await
            .unwrap();
            let full = room.stream_track().await;
            let light = room.track_named(LIGHT_TRACK).await;
            let (count, _) = watch(&light, 2).await;
            assert_eq!(count, 0, "nobody has asked");
            // Past the full layer's renegotiation, which turns encodings back on.
            assert!(!light_sends(&room), "unasked, it takes no share");

            let ask = |whose: String| {
                let attributes = HashMap::from([(ASK_LIGHT.to_owned(), whose)]);
                let viewer = &room.viewer;
                async move {
                    viewer
                        .local_participant()
                        .set_attributes(attributes)
                        .await
                        .unwrap();
                }
            };
            ask(room.streamer.local_participant().identity().to_string()).await;
            let _ = watch(&light, 2).await;
            assert!(light_sends(&room), "asked");
            let (count, size) = watch(&light, 3).await;
            assert!((470..=480).contains(&size.1), "{size:?}");
            assert!(count > 45, "near 30 a second, got {count}");
            let (count, size) = watch(&full, 3).await;
            assert_eq!(size, (1280, 720));
            assert!(count > 90, "the full layer keeps its pace, got {count}");

            ask(String::new()).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
            let (count, _) = watch(&light, 2).await;
            assert_eq!(count, 0, "nobody asks any more");
            assert!(!light_sends(&room), "nobody asks any more");

            // Back before its encoder is let go: its thread sees no start then.
            ask(room.streamer.local_participant().identity().to_string()).await;
            let _ = watch(&light, 2).await;
            let (count, _) = watch(&light, 2).await;
            assert!(count > 30, "asked again, got {count}");

            stream.stop().await;
            room.leave().await;
        });
    }

    #[test]
    fn the_light_layer_is_lighter_than_any_full_one() {
        assert_eq!(light_quality(q(1440, 60)), q(720, 30));
        assert_eq!(light_quality(q(1080, 30)), q(720, 30));
        assert_eq!(light_quality(q(720, 60)), q(480, 30));
        for height in [720, 1080, 1440] {
            for fps in [30, 60] {
                assert!(light_bitrate(q(height, fps)) * 3 < bitrate(q(height, fps)) * 2);
            }
        }
    }

    /// What the connection says about a video it sends: the target, and for a failed test what
    /// tells the estimate holding it back from an encoder or a network doing so (ADR-0067).
    async fn sending(track: &LocalVideoTrack) -> (f64, String) {
        use livekit::webrtc::stats::RtcStats;
        use std::fmt::Write;

        let mut target = 0.0;
        let mut detail = String::new();
        for stats in track.get_stats().await.unwrap() {
            match stats {
                RtcStats::OutboundRtp(rtp) => {
                    let out = rtp.outbound;
                    target = out.target_bitrate;
                    let _ = write!(
                        detail,
                        "encoded {}, sent {} at {:.0} fps, limited by {:?}, send delay {:.2} s, nacks {}; ",
                        out.frames_encoded,
                        out.frames_sent,
                        out.frames_per_second,
                        out.quality_limitation_reason,
                        out.total_packet_send_delay,
                        out.nack_count,
                    );
                }
                RtcStats::RemoteInboundRtp(remote) => {
                    let _ = write!(
                        detail,
                        "rtt {:.3} s, lost {:.3}; ",
                        remote.remote_inbound.round_trip_time, remote.remote_inbound.fraction_lost,
                    );
                }
                _ => {}
            }
        }
        (target, detail)
    }

    /// The connection is already up when a stream starts, as with a voice call, so the
    /// bandwidth estimator has to find the room for it at once (ADR-0062). Fails now and then:
    /// the probe sometimes comes to nothing, and why is still open (ADR-0067).
    #[test]
    #[ignore = "needs LiveKit: pnpm livekit"]
    fn a_stream_gets_its_bitrate_at_once() {
        use livekit::track::LocalTrack;

        tauri::async_runtime::block_on(async {
            let mut room = Audience::gather("stream-ramp").await;
            tokio::time::sleep(Duration::from_secs(3)).await;
            let stream = ScreenStream::run(
                Arc::clone(&room.streamer),
                q(720, 60),
                EncoderChoice::Best,
                || {
                    Ok(Box::new(Stripes {
                        width: 1280,
                        height: 720,
                        frame: 0,
                    }) as Box<dyn Frames>)
                },
            )
            .await
            .unwrap();
            let track = room.stream_track().await;
            let _ = watch(&track, 1).await;
            let publications = room.streamer.local_participant().track_publications();
            let published = publications
                .values()
                .filter(|p| p.name() == FULL_TRACK)
                .find_map(|p| match p.track() {
                    Some(LocalTrack::Video(track)) => Some(track),
                    _ => None,
                })
                .unwrap();
            let ceiling = f64::from(bitrate(q(720, 60)));
            // Seconds, where climbing by itself can take most of a minute.
            let (mut target, mut detail) = (0.0, String::new());
            for _ in 0..16 {
                (target, detail) = sending(&published).await;
                if target > ceiling * 0.8 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            assert!(
                target > ceiling * 0.8,
                "target {target} of {ceiling}: {detail}"
            );
            stream.stop().await;
            room.leave().await;
        });
    }

    /// A real window of ours through the direct capture: its size, a new size, and its end.
    /// Needs a desktop, so not in CI.
    #[test]
    #[ignore = "needs a desktop"]
    fn a_real_window_is_captured_resized_and_lost() {
        use windows::core::w;
        use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW,
            PostQuitMessage, RegisterClassW, SetWindowPos, CW_USEDEFAULT, MSG, SWP_NOMOVE,
            SWP_NOZORDER, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WNDCLASSW, WS_OVERLAPPEDWINDOW,
            WS_VISIBLE,
        };

        extern "system" fn procedure(w: HWND, m: u32, a: WPARAM, b: LPARAM) -> LRESULT {
            if m == WM_DESTROY {
                // SAFETY: called on the thread that owns the window.
                unsafe { PostQuitMessage(0) };
                return LRESULT(0);
            }
            // SAFETY: the system's own default for what it just sent us.
            unsafe { DefWindowProcW(w, m, a, b) }
        }
        let frame_of = |capture: &mut Desktop, wanted: &dyn Fn(u32) -> bool| {
            let begun = Instant::now();
            while begun.elapsed() < Duration::from_secs(5) {
                if let Some(frame) = capture.next(Duration::from_millis(50)) {
                    if wanted(frame.height()) {
                        return Some((frame.width(), frame.height()));
                    }
                }
            }
            None
        };

        let (handle_tx, handle_rx) = mpsc::channel();
        let pump = std::thread::spawn(move || {
            // SAFETY: the class and window live for this thread's message loop; `message`
            // outlives each call.
            unsafe {
                let instance = GetModuleHandleW(None).unwrap();
                let class = WNDCLASSW {
                    lpfnWndProc: Some(procedure),
                    hInstance: instance.into(),
                    lpszClassName: w!("OcCaptureTest"),
                    ..WNDCLASSW::default()
                };
                RegisterClassW(&raw const class);
                let window = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("OcCaptureTest"),
                    w!("capture test"),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    640,
                    480,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap();
                handle_tx.send(window.0 as u64).unwrap();
                let mut message = MSG::default();
                while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
                    DispatchMessageW(&raw const message);
                }
            }
        });
        let id = handle_rx.recv().unwrap();
        let window = HWND(id as *mut _);
        let mut capture = Desktop::open(SourceKind::Window, id, q(1080, 30)).unwrap();
        let first = frame_of(&mut capture, &|_| true).expect("the first frame");
        assert!((600..=640).contains(&first.0), "{first:?}");
        assert!((440..=480).contains(&first.1), "{first:?}");

        // SAFETY: the window is alive until the `WM_CLOSE` below.
        unsafe { SetWindowPos(window, None, 0, 0, 800, 700, SWP_NOMOVE | SWP_NOZORDER) }.unwrap();
        let bigger = frame_of(&mut capture, &|h| h > 600).expect("frames of the new size");
        assert!((760..=800).contains(&bigger.0), "{bigger:?}");

        assert!(!capture.gone());
        // SAFETY: posting to a live window; its thread then closes it.
        unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
        pump.join().unwrap();
        assert!(capture.gone());
    }

    /// A viewer's fps of the real first screen at 1080p60. Needs a desktop, so not in CI;
    /// prints the numbers to look at.
    #[test]
    #[ignore = "needs a desktop and a local LiveKit: `pnpm livekit`"]
    fn the_real_screen_reaches_a_viewer_at_speed() {
        tauri::async_runtime::block_on(async {
            let mut room = Audience::gather("stream-real").await;
            let screen = list_sources()
                .into_iter()
                .find(|s| s.kind == SourceKind::Screen)
                .expect("a screen");
            // Another quality to measure: `OC_TEST_QUALITY=1440x60`.
            let (height, fps) = std::env::var("OC_TEST_QUALITY")
                .ok()
                .and_then(|v| {
                    let (h, f) = v.split_once('x')?;
                    Some((h.parse().ok()?, f.parse().ok()?))
                })
                .unwrap_or((1080, 60));
            let stream = ScreenStream::start(
                Arc::clone(&room.streamer),
                screen.kind,
                screen.id,
                q(height, fps),
                false,
            )
            .await
            .unwrap();
            let track = room.stream_track().await;
            // The first frames settle the connection: a second, then count.
            let _ = watch(&track, 1).await;
            let seconds = 6;
            let (count, size) = watch(&track, seconds).await;
            eprintln!(
                "viewer: {:.1} fps at {size:?}",
                f64::from(count) / seconds as f64
            );
            assert_eq!(size.1, height);
            assert!(
                count > fps * 2 / 3 * seconds as u32,
                "near {fps} a second, got {count}"
            );
            stream.stop().await;
            room.leave().await;
        });
    }

    /// A tone on each side, and nothing else, must arrive each on its own side: stereo is
    /// not a given, the encoder takes it from the negotiation. The streamer's microphone is
    /// published first, as in a real channel, and stays mono.
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn the_sound_of_a_stream_reaches_a_viewer_in_stereo() {
        use crate::stream::sound::tests::{
            test_publish, test_send, test_source, tone_in, TEST_FRAME, TEST_RATE,
        };
        use futures_util::StreamExt;
        use livekit::options::TrackPublishOptions;
        use livekit::track::{LocalAudioTrack, LocalTrack, RemoteTrack, TrackSource as Source};
        use livekit::webrtc::audio_source::native::NativeAudioSource;
        use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
        use livekit::webrtc::audio_stream::native::NativeAudioStream;

        const LEFT: f64 = 440.0;
        const RIGHT: f64 = 1000.0;
        tauri::async_runtime::block_on(async {
            let mut room = Audience::gather("stream-sound").await;
            let mic_source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
            let mic =
                LocalAudioTrack::create_audio_track("mic", RtcAudioSource::Native(mic_source));
            room.streamer
                .local_participant()
                .publish_track(
                    LocalTrack::Audio(mic.clone()),
                    TrackPublishOptions {
                        source: Source::Microphone,
                        ..TrackPublishOptions::default()
                    },
                )
                .await
                .expect("the microphone is published");
            let source = test_source();
            test_publish(&room.streamer, source.clone()).await;
            let sending = Arc::new(AtomicBool::new(true));
            let feeder = std::thread::spawn({
                let sending = Arc::clone(&sending);
                move || {
                    let mut n = 0.0_f64;
                    let mut next = Instant::now();
                    let mut frame = vec![0_i16; TEST_FRAME];
                    while sending.load(Ordering::Relaxed) {
                        for pair in frame.as_chunks_mut::<2>().0 {
                            let at = std::f64::consts::TAU * n / TEST_RATE;
                            pair[0] = (6000.0 * (at * LEFT).sin()) as i16;
                            pair[1] = (6000.0 * (at * RIGHT).sin()) as i16;
                            n += 1.0;
                        }
                        test_send(&source, &frame);
                        next += Duration::from_millis(10);
                        std::thread::sleep(next.saturating_duration_since(Instant::now()));
                    }
                }
            });
            let streamer = room.streamer.local_participant().identity();
            let track = tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    if let Some(livekit::RoomEvent::TrackSubscribed {
                        track: RemoteTrack::Audio(track),
                        publication,
                        participant,
                    }) = room.events.recv().await
                    {
                        if participant.identity() == streamer
                            && publication.source() == Source::ScreenshareAudio
                        {
                            return track;
                        }
                    }
                }
            })
            .await
            .expect("the sound arrives");
            let mut stream = NativeAudioStream::new(track.rtc_track(), 48_000, 2);
            let mut heard: Vec<i16> = Vec::new();
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(3) {
                let frame = tokio::time::timeout(Duration::from_secs(2), stream.next())
                    .await
                    .expect("frames keep coming")
                    .expect("the track goes on");
                // The first second settles the decoder.
                if started.elapsed() > Duration::from_secs(1) {
                    heard.extend_from_slice(&frame.data);
                }
            }
            sending.store(false, Ordering::Relaxed);
            feeder.join().unwrap();
            let on_left = (tone_in(&heard, 0, 2, LEFT), tone_in(&heard, 0, 2, RIGHT));
            let on_right = (tone_in(&heard, 1, 2, LEFT), tone_in(&heard, 1, 2, RIGHT));
            eprintln!("left {on_left:.0?}, right {on_right:.0?}");
            assert!(on_left.0 > 3000.0, "the left tone on the left: {on_left:?}");
            assert!(
                on_right.1 > 3000.0,
                "the right tone on the right: {on_right:?}"
            );
            assert!(on_left.1 < 600.0, "no right tone on the left: {on_left:?}");
            assert!(
                on_right.0 < 600.0,
                "no left tone on the right: {on_right:?}"
            );
            room.leave().await;
        });
    }

    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn a_stream_says_when_its_source_is_gone() {
        tauri::async_runtime::block_on(async {
            let room = Audience::gather("stream-source-gone").await;
            let mut stream = ScreenStream::run(
                Arc::clone(&room.streamer),
                q(720, 30),
                EncoderChoice::Best,
                || {
                    Ok(Box::new(Closing {
                        stripes: Stripes {
                            width: 1280,
                            height: 720,
                            frame: 0,
                        },
                        left: 30,
                    }) as Box<dyn Frames>)
                },
            )
            .await
            .unwrap();
            tokio::time::timeout(Duration::from_secs(10), stream.ended())
                .await
                .expect("the stream ends by itself")
                .unwrap_err();
            assert_eq!(
                room.streamer.local_participant().track_publications().len(),
                2,
                "both layers"
            );
            stream.stop().await;
            assert!(room
                .streamer
                .local_participant()
                .track_publications()
                .is_empty());
            room.leave().await;
        });
    }

    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn a_stream_reaches_another_member_and_decodes() {
        tauri::async_runtime::block_on(async {
            let mut room = Audience::gather("stream").await;
            let stream = ScreenStream::run(
                Arc::clone(&room.streamer),
                q(720, 30),
                EncoderChoice::Best,
                || {
                    Ok(Box::new(Stripes {
                        width: 1280,
                        height: 720,
                        frame: 0,
                    }) as Box<dyn Frames>)
                },
            )
            .await
            .unwrap();
            let track = room.stream_track().await;
            let (count, size) = watch(&track, 4).await;
            eprintln!("decoded {count} frames, {size:?}");
            assert_eq!(size, (1280, 720));
            assert!(count >= 60, "about 30 a second for 4 s, got {count}");
            stream.stop().await;
            room.leave().await;
        });
    }
}
