//! Native audio: devices (output, microphone), the voice engine and the synthetic
//! test voices.

pub mod engine;
pub mod input;
pub mod output;
pub mod synthetic;

/// Identifies one mixer source. The sound of a stream has its own, and therefore its own
/// local volume, independent of the member's voice (ADR-0055).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceKey {
    /// Microphone track of a member.
    Voice(String),
    /// The sound of the stream being watched; one at a time, like the viewer's window.
    Stream,
}
