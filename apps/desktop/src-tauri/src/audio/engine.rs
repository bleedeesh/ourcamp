//! The voice engine as seen by the UI: who is in the channel, their volumes, deafen.
//!
//! Today remote members are played by [`SyntheticVoices`]; in M2 the same calls will
//! subscribe to their LiveKit tracks instead. The UI contract does not change.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use oc_audio::gain::percent_to_gain;
use oc_audio::mixer::{Layout, MixerControl, MixerError, SourceStatus};

use super::output::{self, Output};
use super::synthetic::SyntheticVoices;
use super::SourceKey;

/// A remote member to play when joining a channel.
#[derive(Debug, Clone)]
pub struct MemberSpec {
    /// Account id.
    pub user_id: String,
    /// Saved local volume, 0–200 %.
    pub volume_percent: f32,
    /// Saved local mute.
    pub muted: bool,
    /// The member muted their own microphone, so no audio arrives from them.
    pub remote_mic_muted: bool,
}

/// Level of one member, for the speaking ring.
#[derive(Debug, Clone)]
pub struct MemberLevel {
    /// Account id.
    pub user_id: String,
    /// Pre-volume level in dBFS.
    pub level_db: f32,
    /// Speech detector state.
    pub speaking: bool,
}

struct Live {
    // Declared before `output` so synthetic producers stop before the device does.
    voices: SyntheticVoices,
    mixer: MixerControl<SourceKey>,
    members: Vec<String>,
    #[allow(dead_code, reason = "read by the microphone capture pipeline in M2")]
    mic_muted: bool,
    _output: Output,
}

/// Shared handle to the running engine. Cheap to clone.
#[derive(Clone)]
pub struct Engine {
    live: Arc<Mutex<Live>>,
}

/// User-facing error text (shown in the UI toast).
pub type EngineError = String;

fn mixer_error(e: MixerError) -> EngineError {
    format!("Микшер: {e}")
}

impl Engine {
    /// Opens the default output device.
    ///
    /// # Errors
    /// When no usable output device exists; the app keeps running without audio.
    pub fn start() -> Result<Self, EngineError> {
        let (mixer, output) =
            output::open::<SourceKey>().map_err(|e| format!("Звук недоступен: {e}"))?;
        let voices = SyntheticVoices::start(output.sample_rate());
        eprintln!(
            "[oc-audio] output: {} @ {} Hz",
            output.device_name(),
            output.sample_rate()
        );
        Ok(Self {
            live: Arc::new(Mutex::new(Live {
                voices,
                mixer,
                members: Vec::new(),
                mic_muted: false,
                _output: output,
            })),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Live> {
        // Engine state stays consistent even if a holder panicked mid-call: every
        // mutation below either completes or returns before touching shared state.
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the set of remote members being played.
    ///
    /// # Errors
    /// If the mixer rejects a source (queue full, too many sources).
    pub fn join_channel(&self, members: &[MemberSpec]) -> Result<(), EngineError> {
        let mut live = self.lock();
        Self::clear(&mut live)?;
        for spec in members {
            let key = SourceKey::Voice(spec.user_id.clone());
            let input = live
                .mixer
                .add_source(
                    key.clone(),
                    Layout::Mono,
                    percent_to_gain(spec.volume_percent),
                )
                .map_err(mixer_error)?;
            live.mixer
                .set_muted(&key, spec.muted)
                .map_err(mixer_error)?;
            // A muted member still has a source (they may unmute), it is just silent.
            if !spec.remote_mic_muted {
                live.voices.add(&spec.user_id, input);
            }
            live.members.push(spec.user_id.clone());
        }
        Ok(())
    }

    /// Stops playing everyone.
    ///
    /// # Errors
    /// If the mixer command queue is full.
    pub fn leave_channel(&self) -> Result<(), EngineError> {
        Self::clear(&mut self.lock())
    }

    fn clear(live: &mut Live) -> Result<(), EngineError> {
        live.voices.clear();
        for user_id in std::mem::take(&mut live.members) {
            match live.mixer.remove_source(&SourceKey::Voice(user_id)) {
                Ok(()) | Err(MixerError::UnknownSource) => {}
                Err(e) => return Err(mixer_error(e)),
            }
        }
        Ok(())
    }

    /// Sets a member's local volume. Members not in the channel are ignored (the UI
    /// stores the value and passes it on the next join).
    ///
    /// # Errors
    /// Never in practice; kept fallible for the M2 implementation.
    pub fn set_volume(&self, user_id: &str, percent: f32) -> Result<(), EngineError> {
        let live = self.lock();
        match live
            .mixer
            .set_gain(&SourceKey::Voice(user_id.into()), percent_to_gain(percent))
        {
            Ok(()) | Err(MixerError::UnknownSource) => Ok(()),
            Err(e) => Err(mixer_error(e)),
        }
    }

    /// Locally mutes or unmutes a member.
    ///
    /// # Errors
    /// Never in practice; kept fallible for the M2 implementation.
    pub fn set_local_mute(&self, user_id: &str, muted: bool) -> Result<(), EngineError> {
        let live = self.lock();
        match live
            .mixer
            .set_muted(&SourceKey::Voice(user_id.into()), muted)
        {
            Ok(()) | Err(MixerError::UnknownSource) => Ok(()),
            Err(e) => Err(mixer_error(e)),
        }
    }

    /// Silences all output ("deafen").
    ///
    /// # Errors
    /// If the mixer command queue is full.
    pub fn set_deafened(&self, deafened: bool) -> Result<(), EngineError> {
        self.lock()
            .mixer
            .set_master_muted(deafened)
            .map_err(mixer_error)
    }

    /// Records the microphone state; capture is wired to it in M2.
    pub fn set_mic_muted(&self, muted: bool) {
        self.lock().mic_muted = muted;
    }

    /// Levels of all members in the channel.
    #[must_use]
    pub fn levels(&self) -> Vec<MemberLevel> {
        let live = self.lock();
        live.members
            .iter()
            .filter_map(|id| {
                let SourceStatus {
                    level_db, speaking, ..
                } = live.mixer.status(&SourceKey::Voice(id.clone())).ok()?;
                Some(MemberLevel {
                    user_id: id.clone(),
                    level_db,
                    speaking,
                })
            })
            .collect()
    }
}
