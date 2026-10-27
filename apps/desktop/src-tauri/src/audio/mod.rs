//! Native audio: device output, the voice engine and its temporary synthetic sources.

pub mod engine;
pub mod output;
pub mod synthetic;

/// Identifies one mixer source. Stream audio gets its own variant (and therefore its
/// own local volume, independent of the member's voice) when streams land in M6.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceKey {
    /// Microphone track of a member.
    Voice(String),
}
