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
pub use pipeline::{list_sources, ScreenStream};

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
pub use other::{list_sources, ScreenStream};

/// Until step 6 of M6: everything but Windows says so and streams nothing.
#[cfg(not(windows))]
mod other {
    use std::sync::Arc;

    use livekit::Room;

    use super::{SourceKind, StreamQuality, StreamSource};

    const LATER: &str = "Стримы на этой системе появятся позже (M6, шаг 6)";

    pub struct ScreenStream;

    #[must_use]
    pub fn list_sources() -> Vec<StreamSource> {
        Vec::new()
    }

    impl ScreenStream {
        #[allow(clippy::unused_async, reason = "the same signature as on Windows")]
        pub async fn start(
            _room: Arc<Room>,
            _kind: SourceKind,
            _id: u64,
            _quality: StreamQuality,
        ) -> Result<Self, String> {
            Err(LATER.to_owned())
        }

        #[allow(clippy::unused_async, reason = "the same signature as on Windows")]
        pub async fn stop(self) {}
    }
}
