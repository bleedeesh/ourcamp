//! Real voice (M2): the microphone pipeline and the LiveKit room.
//!
//! Nothing here touches the mixer or Tauri state directly; [`crate::audio::engine`]
//! wires the pieces together.

pub mod mic;
pub mod room;
pub mod token;
