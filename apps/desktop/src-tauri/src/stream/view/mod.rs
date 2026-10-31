//! Watching someone's stream (ADR-0053): while a [`Viewer`] lives, the room is subscribed
//! to their screen and the frames go straight to the picture in the viewer's window. Only
//! a few numbers reach the page.

mod render;
mod surface;

use std::collections::HashMap;
use std::num::NonZeroIsize;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use futures_util::StreamExt;
use livekit::prelude::*;
use livekit::webrtc::audio_stream::native::{NativeAudioStream, NativeAudioStreamOptions};
use livekit::webrtc::prelude::I420Buffer;
use livekit::webrtc::video_frame::VideoBuffer;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
};
use tauri::{Emitter, WebviewWindow};

use super::ViewStatus;
use crate::audio::engine::StreamOutlet;
use crate::voice::room::{play, AbortOnDrop, REMOTE_QUEUE_FRAMES};
use render::Painter;
use surface::{Input, VideoSurface};

/// A member's attribute saying whose stream they watch. Everyone reads it to count viewers.
const WATCHING: &str = "oc.watching";
/// The numbers of the panel for the viewer's page. Must match `STREAM_VIEW_EVENT` in
/// `src/engine/tauri-stream-api.ts`.
const VIEW_EVENT: &str = "stream://view";
/// What was done to the picture, for the page. Must match `STREAM_INPUT_EVENT` there.
const INPUT_EVENT: &str = "stream://input";
/// The viewer exists and can be asked: the page may have opened before it did. Must match
/// `STREAM_READY_EVENT` there.
const READY_EVENT: &str = "stream://ready";

/// What goes to the render thread: only the newest, so a late one is never drawn.
#[derive(Default)]
struct Mailbox {
    picture: Option<I420Buffer>,
    size: Option<(u32, u32)>,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
}

impl Shared {
    fn with(&self, change: impl FnOnce(&mut Mailbox)) {
        change(&mut self.mailbox.lock().unwrap_or_else(PoisonError::into_inner));
        self.wake.notify_one();
    }
}

/// Someone's stream open in the viewer's window.
pub struct Viewer {
    user_id: String,
    name: String,
    window: WebviewWindow,
    surface: VideoSurface,
    shared: Arc<Shared>,
    status: Arc<Mutex<ViewStatus>>,
    render: JoinHandle<()>,
    stop: tokio::sync::oneshot::Sender<()>,
    follow: tauri::async_runtime::JoinHandle<()>,
}

impl Viewer {
    /// Starts watching `user_id` in `window`.
    ///
    /// # Errors
    /// If the picture has nowhere to go; as text for the UI.
    pub async fn start(
        room: Arc<Room>,
        user_id: String,
        name: String,
        window: WebviewWindow,
        outlet: StreamOutlet,
    ) -> Result<Self, String> {
        let surface = make_surface(&window).await?;
        let shared = Arc::new(Shared::default());
        let (ready, painted) = tokio::sync::oneshot::channel();
        let render = std::thread::Builder::new()
            .name("oc-stream-view".into())
            .spawn({
                let shared = Arc::clone(&shared);
                move || render_thread(surface, &shared, ready)
            })
            .map_err(|e| e.to_string())?;
        painted
            .await
            .map_err(|_| "Не удалось показать картинку".to_owned())??;

        let status = Arc::new(Mutex::new(ViewStatus::default()));
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let follow_task = tauri::async_runtime::spawn(follow(
            Follow {
                room,
                identity: ParticipantIdentity::from(user_id.clone()),
                surface,
                shared: Arc::clone(&shared),
                status: Arc::clone(&status),
                window: window.clone(),
                frames: None,
                shown: false,
                count: 0,
                outlet,
                sound: None,
            },
            stopped,
        ));
        Ok(Self {
            user_id,
            name,
            window,
            surface,
            shared,
            status,
            render,
            stop,
            follow: follow_task,
        })
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Tells the page it can ask for its context now.
    pub fn announce(&self) {
        let _ = self.window.emit_to(self.window.label(), READY_EVENT, ());
    }

    /// The panel's numbers now, for a page that has just opened.
    pub fn status(&self) -> ViewStatus {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Brings the window forward, for someone who clicks LIVE again.
    pub fn raise(&self) {
        let _ = self.window.unminimize();
        let _ = self.window.show();
        let _ = self.window.set_focus();
    }

    /// The picture's place in the window, in its pixels, as the page laid it out.
    pub fn place(&self, x: i32, y: i32, width: u32, height: u32) {
        let (w, h) = (
            i32::try_from(width).unwrap_or(i32::MAX),
            i32::try_from(height).unwrap_or(i32::MAX),
        );
        self.surface.place(x, y, w, h);
        self.shared.with(|m| m.size = Some((width, height)));
    }

    /// Stops watching and closes the window. The surface goes before the window that holds
    /// it, and the render thread may need the window's thread to finish a frame: this must
    /// not run on that thread.
    pub async fn close(self) {
        let _ = self.stop.send(());
        let _ = self.follow.await;
        self.shared.with(|m| m.stop = true);
        let render = self.render;
        let _ = tauri::async_runtime::spawn_blocking(move || render.join()).await;
        close_window(&self.window).await;
    }
}

/// Destroys the window and waits until it is gone: `destroy` only asks, and the label stays
/// taken until the window's thread gets to it, so the next stream could not open.
async fn close_window(window: &WebviewWindow) {
    let (gone, wait) = tokio::sync::oneshot::channel();
    let gone = Mutex::new(Some(gone));
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            if let Some(gone) = gone.lock().unwrap_or_else(PoisonError::into_inner).take() {
                let _ = gone.send(());
            }
        }
    });
    if window.destroy().is_ok() {
        // Bounded: if the event is ever missed, the worst is one failed opening.
        let _ = tokio::time::timeout(Duration::from_secs(5), wait).await;
    }
}

/// Makes the child window on the thread of its parent, and tells the page about input.
async fn make_surface(window: &WebviewWindow) -> Result<VideoSurface, String> {
    let parent = window.hwnd().map_err(|e| e.to_string())?.0 as isize;
    let page = window.clone();
    let (made, surface) = tokio::sync::oneshot::channel();
    window
        .run_on_main_thread(move || {
            let result = VideoSurface::create(parent, move |input| {
                let name = match input {
                    Input::DoubleClick => "doubleClick",
                    Input::Escape => "escape",
                };
                let _ = page.emit_to(page.label(), INPUT_EVENT, name);
            });
            let _ = made.send(result);
        })
        .map_err(|e| e.to_string())?;
    surface
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Не удалось создать окно картинки: {e}"))
}

struct Screen {
    painter: Painter,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

/// Sets `wgpu` up for the child window.
fn make_screen(target: VideoSurface) -> Result<Screen, String> {
    tauri::async_runtime::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let hwnd = NonZeroIsize::new(target.raw()).ok_or("Нет окна для картинки")?;
        // SAFETY: the child window lives until its parent is destroyed, and `Viewer::close`
        // joins this thread, which owns the surface, before destroying the parent.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(RawDisplayHandle::Windows(WindowsDisplayHandle::new())),
                raw_window_handle: RawWindowHandle::Win32(Win32WindowHandle::new(hwnd)),
            })
        }
        .map_err(|e| e.to_string())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..wgpu::RequestAdapterOptions::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|e| e.to_string())?;
        let capabilities = surface.get_capabilities(&adapter);
        // Plain 8 bits, not sRGB: the shader already gives the values to show, a second
        // conversion would wash the picture out.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
            .ok_or("Видеокарта не умеет показывать картинку")?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: 1,
            height: 1,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 1,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: Vec::new(),
        };
        Ok(Screen {
            painter: Painter::new(device, queue, format),
            surface,
            config,
        })
    })
}

impl Screen {
    fn resize(&mut self, size: (u32, u32)) {
        // A window can be squeezed to nothing; a surface cannot.
        self.config.width = size.0.max(1);
        self.config.height = size.1.max(1);
        self.surface.configure(self.painter.device(), &self.config);
    }

    fn draw(&self) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(self.painter.device(), &self.config);
                return;
            }
            // The window is covered, or the moment is unlucky: the next picture tries again.
            _ => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.painter
            .draw(&view, (self.config.width, self.config.height));
        self.painter.queue().present(frame);
    }
}

/// Draws whatever is newest until told to stop. A thread of its own: presenting waits for
/// the screen, and nothing else should wait with it.
fn render_thread(
    target: VideoSurface,
    shared: &Shared,
    ready: tokio::sync::oneshot::Sender<Result<(), String>>,
) {
    let mut screen = match make_screen(target) {
        Ok(screen) => {
            let _ = ready.send(Ok(()));
            screen
        }
        Err(e) => {
            let _ = ready.send(Err(format!("Не удалось показать картинку: {e}")));
            return;
        }
    };
    loop {
        let (picture, size) = {
            let mut mailbox = shared
                .wake
                .wait_while(
                    shared
                        .mailbox
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner),
                    |m| !m.stop && m.picture.is_none() && m.size.is_none(),
                )
                .unwrap_or_else(PoisonError::into_inner);
            if mailbox.stop {
                return;
            }
            (mailbox.picture.take(), mailbox.size.take())
        };
        if let Some(size) = size {
            screen.resize(size);
        }
        if let Some(picture) = picture {
            screen.painter.upload(&picture);
        }
        screen.draw();
    }
}

struct Follow {
    room: Arc<Room>,
    identity: ParticipantIdentity,
    surface: VideoSurface,
    shared: Arc<Shared>,
    status: Arc<Mutex<ViewStatus>>,
    window: WebviewWindow,
    frames: Option<NativeVideoStream>,
    /// The child window is showing.
    shown: bool,
    /// Pictures since the last tick.
    count: u32,
    outlet: StreamOutlet,
    /// The sound playing, for as long as it stays in here.
    sound: Option<AbortOnDrop>,
}

impl Follow {
    fn publication(&self, source: TrackSource) -> Option<RemoteTrackPublication> {
        self.room
            .remote_participants()
            .get(&self.identity)?
            .track_publications()
            .into_values()
            .find(|p| p.source() == source)
    }

    fn screen_publication(&self) -> Option<RemoteTrackPublication> {
        self.publication(TrackSource::Screenshare)
    }

    fn sound_publication(&self) -> Option<RemoteTrackPublication> {
        self.publication(TrackSource::ScreenshareAudio)
    }

    fn is_sound(
        &self,
        participant: &RemoteParticipant,
        publication: &RemoteTrackPublication,
    ) -> bool {
        participant.identity() == self.identity
            && publication.source() == TrackSource::ScreenshareAudio
    }

    fn play_sound(&mut self, track: &RemoteAudioTrack) {
        self.sound = None;
        let Some(input) = self.outlet.open() else {
            return;
        };
        let stream = NativeAudioStream::with_options(
            track.rtc_track(),
            i32::try_from(self.outlet.rate).expect("sample rate fits in i32"),
            2,
            NativeAudioStreamOptions {
                queue_size_frames: Some(REMOTE_QUEUE_FRAMES),
            },
        );
        self.sound = Some(AbortOnDrop(tauri::async_runtime::spawn(play(
            stream, input,
        ))));
        self.report(|s| s.sound = true);
    }

    fn sound_ended(&mut self) {
        if self.sound.take().is_some() {
            self.outlet.close();
            self.report(|s| s.sound = false);
        }
    }

    /// Everyone with the attribute for this stream, and you.
    fn viewers(&self) -> u32 {
        let wanted: &str = self.identity.as_str();
        let others = self
            .room
            .remote_participants()
            .into_values()
            .filter(|p| p.attributes().get(WATCHING).is_some_and(|w| w == wanted))
            .count();
        u32::try_from(others).unwrap_or(u32::MAX).saturating_add(1)
    }

    /// Only a change goes to the page.
    fn report(&self, change: impl FnOnce(&mut ViewStatus)) {
        let now = {
            let mut status = self.status.lock().unwrap_or_else(PoisonError::into_inner);
            let before = *status;
            change(&mut status);
            (*status != before).then_some(*status)
        };
        if let Some(now) = now {
            let _ = self.window.emit_to(self.window.label(), VIEW_EVENT, now);
        }
    }

    /// The stream is gone, for now: its window waits in case it comes back.
    fn ended(&mut self) {
        self.frames = None;
        self.shown = false;
        self.surface.show(false);
        self.report(|s| {
            s.live = false;
            s.height = 0;
            s.fps = 0;
        });
    }

    fn is_screen(
        &self,
        participant: &RemoteParticipant,
        publication: &RemoteTrackPublication,
    ) -> bool {
        participant.identity() == self.identity && publication.source() == TrackSource::Screenshare
    }

    fn recount(&self) {
        self.report(|s| s.viewers = self.viewers());
    }

    fn picture(&mut self, picture: I420Buffer) {
        let height = picture.height();
        self.count += 1;
        self.shared.with(|m| m.picture = Some(picture));
        if !self.shown {
            self.surface.show(true);
            self.shown = true;
        }
        self.report(|s| {
            s.live = true;
            s.height = height;
        });
    }

    fn event(&mut self, event: RoomEvent) {
        match event {
            RoomEvent::TrackPublished {
                publication,
                participant,
            } if self.is_screen(&participant, &publication)
                || self.is_sound(&participant, &publication) =>
            {
                publication.set_subscribed(true);
            }
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(track),
                publication,
                participant,
            } if self.is_sound(&participant, &publication) => self.play_sound(&track),
            RoomEvent::TrackUnsubscribed {
                publication,
                participant,
                ..
            }
            | RoomEvent::TrackUnpublished {
                publication,
                participant,
            } if self.is_sound(&participant, &publication) => self.sound_ended(),
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Video(track),
                publication,
                participant,
            } if self.is_screen(&participant, &publication) => {
                self.frames = Some(NativeVideoStream::new(track.rtc_track()));
            }
            RoomEvent::TrackUnsubscribed {
                publication,
                participant,
                ..
            }
            | RoomEvent::TrackUnpublished {
                publication,
                participant,
            } if self.is_screen(&participant, &publication) => self.ended(),
            RoomEvent::ParticipantDisconnected(participant) => {
                if participant.identity() == self.identity {
                    self.ended();
                    self.sound_ended();
                }
                self.recount();
            }
            RoomEvent::ParticipantAttributesChanged { .. } | RoomEvent::ParticipantConnected(_) => {
                self.recount();
            }
            _ => {}
        }
    }
}

async fn next_frame(
    frames: &mut Option<NativeVideoStream>,
) -> Option<livekit::webrtc::video_frame::BoxVideoFrame> {
    match frames {
        Some(frames) => frames.next().await,
        None => std::future::pending().await,
    }
}

/// Says in the room whose stream we watch; an empty value when we stop.
async fn say_watching(room: &Room, whose: String) {
    let attributes = HashMap::from([(WATCHING.to_owned(), whose)]);
    if let Err(e) = room.local_participant().set_attributes(attributes).await {
        eprintln!("[ourcamp] stream viewers: {e}");
    }
}

/// Keeps the subscription and carries pictures for as long as the viewer lives.
async fn follow(mut this: Follow, mut stopped: tokio::sync::oneshot::Receiver<()>) {
    // Before looking at what is there: nothing can fall between the two.
    let mut events = this.room.subscribe();
    if let Some(publication) = this.screen_publication() {
        publication.set_subscribed(true);
        if let Some(RemoteTrack::Video(track)) = publication.track() {
            this.frames = Some(NativeVideoStream::new(track.rtc_track()));
        }
    }
    if let Some(publication) = this.sound_publication() {
        publication.set_subscribed(true);
        if let Some(RemoteTrack::Audio(track)) = publication.track() {
            this.play_sound(&track);
        }
    }
    say_watching(&this.room, this.identity.to_string()).await;
    this.recount();

    let mut second = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = &mut stopped => break,
            _ = second.tick() => {
                let fps = std::mem::take(&mut this.count);
                this.report(|s| s.fps = fps);
            }
            frame = next_frame(&mut this.frames) => match frame {
                Some(frame) => this.picture(frame.buffer.to_i420()),
                None => this.ended(),
            },
            event = events.recv() => match event {
                Some(event) => this.event(event),
                None => break,
            },
        }
    }

    if let Some(publication) = this.screen_publication() {
        publication.set_subscribed(false);
    }
    if let Some(publication) = this.sound_publication() {
        publication.set_subscribed(false);
    }
    this.sound_ended();
    say_watching(&this.room, String::new()).await;
}
