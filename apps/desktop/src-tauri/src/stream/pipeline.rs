//! From a screen to the room, on the stream's own thread: frames as I420 at the chosen
//! size, NV12 for the encoder, encoded frames to the SDK. The encoder is tied to that
//! thread (COM); only the capture callback and the published source cross it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoderBackend, VideoEncoding};
use livekit::track::{LocalTrack, LocalVideoTrack, TrackSource};
use livekit::webrtc::desktop_capturer::{
    CaptureSource, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions, DesktopFrame,
};
use livekit::webrtc::native::yuv_helper;
use livekit::webrtc::video_frame::{
    EncodedFrameType, EncodedVideoCodec, EncodedVideoFrame, I420Buffer, VideoBuffer,
};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use livekit::Room;
use tokio::sync::oneshot;

use super::mf::{EncoderChoice, EncoderSettings, H264Encoder};
use super::{SourceKind, StreamQuality, StreamSource};

/// The first frame must come within this, or the source is not there to capture.
const FIRST_FRAME: Duration = Duration::from_secs(5);
/// `WebRTC` may ask for little on a bad link; below this the picture is mush anyway.
const MIN_BITRATE: u32 = 300_000;

/// Bits per second for a quality (ADR-0049).
const fn bitrate(quality: StreamQuality) -> u32 {
    match (quality.height >= 1080, quality.fps >= 60) {
        (true, true) => 6_000_000,
        (true, false) | (false, true) => 4_000_000,
        (false, false) => 2_500_000,
    }
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
    /// The next frame at the chosen size; `None` when none is ready yet.
    fn next(&mut self) -> Option<I420Buffer>;
}

/// A frame from the capturer, at the chosen size.
fn to_i420(frame: &DesktopFrame, quality: StreamQuality) -> Option<I420Buffer> {
    let (width, height) = (frame.width(), frame.height());
    if width < 2 || height < 2 {
        return None;
    }
    let mut buffer = I420Buffer::new(width as u32, height as u32);
    let (stride_y, stride_u, stride_v) = buffer.strides();
    let (y, u, v) = buffer.data_mut();
    // What libyuv calls ARGB is BGRA in memory: what the capturer gives.
    yuv_helper::argb_to_i420(
        frame.data(),
        frame.stride(),
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

/// The screen or window, through the SDK's capturer.
struct Desktop {
    capturer: DesktopCapturer,
    frames: mpsc::Receiver<I420Buffer>,
}

impl Desktop {
    fn open(kind: SourceKind, id: u64, quality: StreamQuality) -> Result<Self, String> {
        let mut capturer = capturer(kind).ok_or("Захват экрана на этом компьютере недоступен")?;
        let source: CaptureSource = capturer
            .get_source_list()
            .into_iter()
            .find(|s| s.id() == id)
            .ok_or("Этого экрана или окна больше нет")?;
        let (sender, frames) = mpsc::channel();
        capturer.start_capture(Some(source), move |result| {
            // A failed frame is skipped: the next tick asks again, and the thread
            // stops by itself when frames stay away at the start.
            if let Some(buffer) = result.ok().and_then(|f| to_i420(&f, quality)) {
                let _ = sender.send(buffer);
            }
        });
        Ok(Self { capturer, frames })
    }
}

impl Frames for Desktop {
    fn next(&mut self) -> Option<I420Buffer> {
        self.capturer.capture_frame();
        self.frames.try_recv().ok()
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

/// The screens and windows that can be streamed.
#[must_use]
pub fn list_sources() -> Vec<StreamSource> {
    [SourceKind::Screen, SourceKind::Window]
        .into_iter()
        .flat_map(|kind| {
            capturer(kind)
                .map(|c| c.get_source_list())
                .unwrap_or_default()
                .into_iter()
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
    encoder: String,
}

/// A running stream in the room.
pub struct ScreenStream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    room: Arc<Room>,
    track_sid: livekit::id::TrackSid,
}

impl ScreenStream {
    /// Starts streaming a screen or window into `room`.
    ///
    /// # Errors
    /// The source is gone, no encoder works, or the room refused the track (forbidden to
    /// speak, ADR-0028); as text for the UI.
    pub async fn start(
        room: Arc<Room>,
        kind: SourceKind,
        id: u64,
        quality: StreamQuality,
    ) -> Result<Self, String> {
        Self::run(room, quality, EncoderChoice::Best, move || {
            Ok(Box::new(Desktop::open(kind, id, quality)?) as Box<dyn Frames>)
        })
        .await
    }

    async fn run(
        room: Arc<Room>,
        quality: StreamQuality,
        choice: EncoderChoice,
        open: impl FnOnce() -> Result<Box<dyn Frames>, String> + Send + 'static,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let go = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("oc-stream".into())
            .spawn({
                let (stop, go) = (Arc::clone(&stop), Arc::clone(&go));
                move || stream_thread(open, quality, choice, &stop, &go, ready_tx)
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
        let track =
            LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(ready.source));
        let options = TrackPublishOptions {
            source: TrackSource::Screenshare,
            video_codec: VideoCodec::H264,
            // A pre-encoded source: `WebRTC` only packetizes what we send.
            video_encoder: VideoEncoderBackend::PreEncoded,
            simulcast: false,
            video_encoding: Some(VideoEncoding {
                max_bitrate: u64::from(bitrate(quality)),
                max_framerate: f64::from(quality.fps),
            }),
            ..TrackPublishOptions::default()
        };
        let published = room
            .local_participant()
            .publish_track(LocalTrack::Video(track), options)
            .await;
        let publication = match published {
            Ok(publication) => publication,
            Err(e) => {
                halt(thread, &stop);
                return Err(format!("Не удалось начать стрим: {e}"));
            }
        };
        go.store(true, Ordering::Relaxed);
        Ok(Self {
            stop,
            thread: Some(thread),
            room,
            track_sid: publication.sid(),
        })
    }

    /// Stops the stream and takes it out of the room.
    pub async fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = tauri::async_runtime::spawn_blocking(move || thread.join()).await;
        }
        if let Err(e) = self
            .room
            .local_participant()
            .unpublish_track(&self.track_sid)
            .await
        {
            eprintln!("[oc-stream] unpublish: {e}");
        }
    }
}

/// The encoder and what it feeds. Remade when a window changes size.
struct Encoding {
    encoder: H264Encoder,
    width: u32,
    height: u32,
    nv12: Vec<u8>,
}

impl Encoding {
    fn new(
        width: u32,
        height: u32,
        quality: StreamQuality,
        choice: EncoderChoice,
    ) -> Result<Self, String> {
        let encoder = H264Encoder::new(
            EncoderSettings {
                width,
                height,
                fps: quality.fps,
                bitrate: bitrate(quality),
            },
            choice,
        )?;
        Ok(Self {
            encoder,
            width,
            height,
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
}

fn stream_thread(
    open: impl FnOnce() -> Result<Box<dyn Frames>, String>,
    quality: StreamQuality,
    choice: EncoderChoice,
    stop: &AtomicBool,
    go: &AtomicBool,
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
        if let Some(frame) = frames.next() {
            break frame;
        }
        if stop.load(Ordering::Relaxed) || started.elapsed() > FIRST_FRAME {
            let _ = ready.send(Err("Этот экран или окно не отдаёт кадры".to_owned()));
            return;
        }
        std::thread::sleep(tick);
    };
    let (width, height) = (first.width(), first.height());
    let encoding = match Encoding::new(width, height, quality, choice) {
        Ok(encoding) => encoding,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let source = NativeVideoSource::new_encoded(VideoResolution { width, height });
    let name = encoding.encoder.name().to_owned();
    if ready
        .send(Ok(Ready {
            source: source.clone(),
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

    pump(
        frames.as_mut(),
        Some(first),
        encoding,
        &source,
        quality,
        choice,
        stop,
    );
}

/// Sends frames until told to stop, at the chosen pace.
fn pump(
    frames: &mut dyn Frames,
    mut current: Option<I420Buffer>,
    mut encoding: Encoding,
    source: &NativeVideoSource,
    quality: StreamQuality,
    choice: EncoderChoice,
    stop: &AtomicBool,
) {
    let tick = Duration::from_secs_f64(1.0 / f64::from(quality.fps));
    let mut key = true;
    let mut next = Instant::now();
    let started = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if let Some(frame) = current.take().or_else(|| frames.next()) {
            if (frame.width(), frame.height()) != (encoding.width, encoding.height) {
                // A window changed size: a new encoder, starting with a key frame.
                match Encoding::new(frame.width(), frame.height(), quality, choice) {
                    Ok(fresh) => encoding = fresh,
                    Err(e) => {
                        eprintln!("[oc-stream] {e}");
                        return;
                    }
                }
                key = true;
            }
            key |= source.take_keyframe_request();
            if let Some(rate) = source.take_rate_control_request() {
                let target = (rate.target_bitrate_bps.min(u64::from(bitrate(quality))) as u32)
                    .max(MIN_BITRATE);
                encoding.encoder.set_bitrate(target);
            }
            encoding.fill(&frame);
            let timestamp = started.elapsed().as_micros() as i64;
            match encoding.encoder.encode(&encoding.nv12, timestamp, key) {
                Ok(encoded) => {
                    key = false;
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
                                width: encoding.width,
                                height: encoding.height,
                            },
                            frame_metadata: None,
                        });
                    }
                }
                Err(e) => {
                    eprintln!("[oc-stream] {e}");
                    return;
                }
            }
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
        assert_eq!(bitrate(q(1080, 60)), 6_000_000);
        assert_eq!(bitrate(q(1080, 30)), 4_000_000);
        assert_eq!(bitrate(q(720, 60)), 4_000_000);
        assert_eq!(bitrate(q(720, 30)), 2_500_000);
    }

    /// Moving stripes at a fixed size.
    struct Stripes {
        width: u32,
        height: u32,
        frame: u32,
    }

    impl Frames for Stripes {
        fn next(&mut self) -> Option<I420Buffer> {
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

    /// A streamer and a viewer in one room of a fresh camp.
    struct Audience {
        camp: oc_server::Running,
        dir: std::path::PathBuf,
        streamer: Arc<Room>,
        viewer: Room,
        events: tokio::sync::mpsc::UnboundedReceiver<livekit::RoomEvent>,
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
            }
        }

        /// The stream's track as the viewer gets it. The streamer's, by identity: every
        /// camp in these tests has its room `c0` on one LiveKit, where someone from another
        /// test, or from a run just ended, may still count as present with a dead track.
        async fn stream_track(&mut self) -> livekit::track::RemoteVideoTrack {
            use livekit::track::RemoteTrack;
            let streamer = self.streamer.local_participant().identity();
            tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    if let Some(livekit::RoomEvent::TrackSubscribed {
                        track: RemoteTrack::Video(track),
                        publication,
                        participant,
                    }) = self.events.recv().await
                    {
                        if participant.identity() == streamer {
                            assert_eq!(publication.source(), TrackSource::Screenshare);
                            return track;
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
            let stream = ScreenStream::start(
                Arc::clone(&room.streamer),
                screen.kind,
                screen.id,
                q(1080, 60),
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
            assert_eq!(size.1, 1080);
            assert!(count > 40 * seconds as u32, "near 60 a second, got {count}");
            stream.stop().await;
            room.leave().await;
        });
    }

    /// The whole road, but for the screen: encoder, pre-encoded track, a real LiveKit through
    /// the camp's pinned port, and another member decoding what arrives. The size and the
    /// count come out of the decoder, so a stream nobody can decode fails here.
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
