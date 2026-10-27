//! Native audio: devices (output, microphone), the voice engine and the synthetic
//! test voices.

pub mod engine;
pub mod input;
pub mod output;
pub mod synthetic;

/// Identifies one mixer source. Stream audio gets its own variant (and therefore its
/// own local volume, independent of the member's voice) when streams land in M6.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceKey {
    /// Microphone track of a member.
    Voice(String),
}
