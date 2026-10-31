//! Screen streams (M6, ADR-0049). A stream publishes into the voice room already joined:
//! on Windows, the SDK's capture → H.264 through Media Foundation → the SDK's pre-encoded
//! path. Other systems come with step 6.

use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod h264;
#[cfg(windows)]
mod mf;
#[cfg(windows)]
mod pipeline;

#[cfg(windows)]
mod view;

#[cfg(windows)]
pub use pipeline::{list_sources, ScreenStream};
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
    /// 720 or 1080: the height the picture is brought down to.
    pub height: u32,
    /// 30 or 60.
    pub fps: u32,
}

#[cfg(not(windows))]
pub use other::{list_sources, ScreenStream, Viewer};

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
}

/// Until step 6 of M6: everything but Windows says so and streams nothing.
#[cfg(not(windows))]
mod other {
    use std::future::Future;
    use std::sync::Arc;

    use livekit::Room;

    use tauri::WebviewWindow;

    use super::{SourceKind, StreamQuality, StreamSource, ViewStatus};

    const LATER: &str = "Стримы на этой системе появятся позже (M6, шаг 6)";

    pub struct ScreenStream;

    #[must_use]
    pub fn list_sources() -> Vec<StreamSource> {
        Vec::new()
    }

    // Awaited like the `async fn`s on Windows; here there is nothing to wait for.
    impl ScreenStream {
        pub fn start(
            _room: Arc<Room>,
            _kind: SourceKind,
            _id: u64,
            _quality: StreamQuality,
        ) -> impl Future<Output = Result<Self, String>> {
            std::future::ready(Err(LATER.to_owned()))
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
