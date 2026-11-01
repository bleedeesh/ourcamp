//! Screen streams (M6, ADR-0049). A stream publishes into the voice room already joined:
//! on Windows, capture (a window directly, ADR-0060; a screen through the SDK) → H.264 through Media Foundation → the SDK's pre-encoded
//! path. Other systems come with step 6.

use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod h264;
#[cfg(windows)]
mod mf;
#[cfg(windows)]
mod pipeline;

#[cfg(windows)]
mod sound;
#[cfg(windows)]
mod view;
#[cfg(windows)]
mod wgc;

#[cfg(windows)]
pub use pipeline::{list_sources, ScreenStream};
#[cfg(windows)]
pub use sound::available as sound_available;
#[cfg(windows)]
pub use view::Viewer;

/// What can be streamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Screen,
    Window,
}

/// A screen or a window to offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSource {
    pub kind: SourceKind,
    pub id: u64,
    /// A window's title; empty for a screen on most systems.
    pub title: String,
}

/// Quality the streamer chose (ADR-0049).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "read by the Windows pipeline; others come with step 6"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct StreamQuality {
    /// 720, 1080 or 1440: the height the picture is brought down to.
    pub height: u32,
    /// 30 or 60.
    pub fps: u32,
}

/// The most bits per second for a quality: the same picture quality in every row. Less goes
/// out when the picture needs less or `WebRTC` finds less room on the link (ADR-0063).
#[must_use]
pub const fn bitrate(quality: StreamQuality) -> u32 {
    let high_rate = quality.fps >= 60;
    match (quality.height, high_rate) {
        (1440.., true) => 40_000_000,
        (1440.., false) | (1080.., true) => 20_000_000,
        (1080.., false) => 12_000_000,
        (_, true) => 10_000_000,
        (_, false) => 6_000_000,
    }
}

/// The stream's two video tracks, by name (ADR-0065). LiveKit stops a video track that does not
/// fit a viewer's line only if it is not `SCREEN_SHARE`, so they are published as a camera.
#[cfg(windows)]
pub(crate) const FULL_TRACK: &str = "screen";
#[cfg(windows)]
pub(crate) const LIGHT_TRACK: &str = "screen-light";
#[cfg_attr(
    not(windows),
    allow(
        dead_code,
        reason = "for the tests of the room; streams come with step 6"
    )
)]
pub(crate) const STREAM_SOURCE: livekit::track::TrackSource = livekit::track::TrackSource::Camera;
/// A member's attribute: whose light layer they need. The streamer encodes it only then.
#[cfg(windows)]
pub(crate) const ASK_LIGHT: &str = "oc.light";

/// What a viewer gets when their line cannot carry the stream (ADR-0065).
#[cfg(windows)]
pub(crate) const fn light_quality(quality: StreamQuality) -> StreamQuality {
    StreamQuality {
        height: if quality.height > 720 { 720 } else { 480 },
        fps: 30,
    }
}

/// The most bits per second of the light layer.
#[cfg(windows)]
pub(crate) const fn light_bitrate(quality: StreamQuality) -> u32 {
    if quality.height > 720 {
        3_000_000
    } else {
        1_500_000
    }
}

/// A quality to offer, with what it costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityOption {
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
}

/// What the window "Начать стрим" offers: the table has one home, here.
#[must_use]
pub fn quality_options() -> Vec<QualityOption> {
    [720, 1080, 1440]
        .into_iter()
        .flat_map(|height| {
            [30, 60].map(|fps| QualityOption {
                height,
                fps,
                bitrate: bitrate(StreamQuality { height, fps }),
            })
        })
        .collect()
}

/// How your own stream is going, for the button in the main window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnStream {
    pub live: bool,
    /// What you should know and did not ask: why it stopped, or that it goes without sound.
    pub note: Option<String>,
}

#[cfg(not(windows))]
pub use other::{list_sources, sound_available, ScreenStream, Viewer};

/// What the panel of the viewer's window shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewStatus {
    /// Pictures are arriving.
    pub live: bool,
    /// Height of the picture.
    pub height: u32,
    /// Pictures in the last second.
    pub fps: u32,
    /// How many are watching, you included.
    pub viewers: u32,
    /// The stream comes with sound.
    pub sound: bool,
    /// Your line cannot carry the stream, so a lighter picture comes (ADR-0065).
    pub reduced: bool,
}

/// Until step 6 of M6: everything but Windows says so and streams nothing.
#[cfg(not(windows))]
mod other {
    use std::future::Future;
    use std::sync::Arc;

    use livekit::Room;
    use tauri::WebviewWindow;
    use tokio::sync::oneshot;

    use super::{SourceKind, StreamQuality, StreamSource, ViewStatus};
    use crate::audio::engine::StreamOutlet;

    const LATER: &str = "Стримы на этой системе появятся позже (M6, шаг 6)";

    pub struct ScreenStream;

    #[must_use]
    pub fn list_sources() -> Vec<StreamSource> {
        Vec::new()
    }

    #[must_use]
    pub fn sound_available() -> bool {
        false
    }

    // Awaited like the `async fn`s on Windows; here there is nothing to wait for.
    impl ScreenStream {
        pub fn start(
            _room: Arc<Room>,
            _kind: SourceKind,
            _id: u64,
            _quality: StreamQuality,
            _sound: bool,
        ) -> impl Future<Output = Result<Self, String>> {
            std::future::ready(Err(LATER.to_owned()))
        }

        pub fn ended(&mut self) -> oneshot::Receiver<()> {
            let Self = self;
            oneshot::channel().1
        }

        pub fn sound_problem(&self) -> Option<String> {
            let Self = self;
            None
        }

        #[allow(clippy::unused_async, reason = "the same signature as on Windows")]
        pub async fn stop(self) {}
    }

    pub struct Viewer;

    // No viewer is ever made here: `start` says so, and the rest is what Windows has.
    impl Viewer {
        pub fn start(
            _room: Arc<Room>,
            _user_id: String,
            _name: String,
            _window: WebviewWindow,
            _outlet: StreamOutlet,
        ) -> impl Future<Output = Result<Self, String>> {
            std::future::ready(Err(LATER.to_owned()))
        }

        pub fn user_id(&self) -> &'static str {
            let Self = self;
            ""
        }

        pub fn name(&self) -> &'static str {
            let Self = self;
            ""
        }

        pub fn status(&self) -> ViewStatus {
            let Self = self;
            ViewStatus::default()
        }

        pub fn raise(&self) {
            let Self = self;
        }

        pub fn announce(&self) {
            let Self = self;
        }

        pub fn place(&self, _x: i32, _y: i32, _width: u32, _height: u32) {
            let Self = self;
        }

        #[allow(clippy::unused_async, reason = "the same signature as on Windows")]
        pub async fn close(self) {
            let Self = self;
        }
    }
}
