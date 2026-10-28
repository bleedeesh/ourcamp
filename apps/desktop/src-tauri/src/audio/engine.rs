//! The voice engine as seen by the UI: who is in the channel, their volumes, deafen.
//!
//! Two modes behind the same calls:
//! * **LiveKit** (default): joining a channel opens the microphone pipeline and the
//!   room; remote microphones become mixer sources as they arrive.
//! * **Synthetic** (`OC_VOICE=synthetic`): the M0 stand-in voices, for testing volume
//!   and the speaking ring without a second person or a server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use oc_audio::gain::percent_to_gain;
use oc_audio::mixer::{Layout, MixerControl, MixerError, SourceInput, SourceStatus};

use super::output::{self, DeviceLatency, Output};
use super::synthetic::SyntheticVoices;
use super::SourceKey;
use crate::voice::mic::{LocalLevel, MicPipeline, TapSlot};
use crate::voice::room::{Link, RemoteMember, RoomAccess, RoomHost, RoomParams, VoiceRoom};

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

/// Who is in the channel with us (ADR-0011). Always the full list.
#[derive(Debug, Clone)]
pub struct Participants {
    /// The local user's id, as the room knows it.
    pub self_id: String,
    /// Everyone else.
    pub members: Vec<RemoteMember>,
}

/// Receives [`Participants`] updates (the app forwards them to the UI).
pub type ParticipantsListener = Arc<dyn Fn(Participants) + Send + Sync>;

/// Receives the reason, in Russian, when the server ends a voice session (ADR-0020).
pub type DisconnectListener = Arc<dyn Fn(String) + Send + Sync>;

/// Receives the connection's quality and ping while in a channel (ADR-0030).
pub type LinkListener = Arc<dyn Fn(Link) + Send + Sync>;

/// User-facing error text (shown in the UI toast).
pub type EngineError = String;

#[derive(Debug, Clone, Copy)]
struct Prefs {
    volume_percent: f32,
    muted: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            volume_percent: 100.0,
            muted: false,
        }
    }
}

struct Session {
    /// Tells a late server disconnect apart from the session that replaced it.
    id: u64,
    room: VoiceRoom,
    mic: MicPipeline,
}

struct Live {
    /// `Some` in synthetic mode. Declared before `_output` so producers stop first.
    synthetic: Option<SyntheticVoices>,
    mixer: MixerControl<SourceKey>,
    /// Members that currently have a mixer source.
    sources: Vec<String>,
    /// Local volume and mute of everyone the UI told us about, applied when their
    /// voice arrives — including people who join after us.
    prefs: HashMap<String, Prefs>,
    mic_muted: bool,
    /// The mixer's output tap; borrowed by the microphone pipeline while in a channel.
    taps: TapSlot,
    session: Option<Session>,
    local_level: Option<Arc<LocalLevel>>,
    listener: Option<ParticipantsListener>,
    self_id: String,
    disconnect_listener: Option<DisconnectListener>,
    link_listener: Option<LinkListener>,
    next_session_id: u64,
    output_latency: DeviceLatency,
    output_rate: u32,
    _output: Output,
}

impl Live {
    fn add_voice(&mut self, user_id: &str) -> Result<SourceInput, MixerError> {
        let prefs = self.prefs.get(user_id).copied().unwrap_or_default();
        let key = SourceKey::Voice(user_id.into());
        let input = self.mixer.add_source(
            key.clone(),
            Layout::Mono,
            percent_to_gain(prefs.volume_percent),
        )?;
        self.mixer.set_muted(&key, prefs.muted)?;
        self.sources.push(user_id.into());
        Ok(input)
    }

    fn remove_voice(&mut self, user_id: &str) -> Result<(), MixerError> {
        self.sources.retain(|id| id != user_id);
        match self.mixer.remove_source(&SourceKey::Voice(user_id.into())) {
            Ok(()) | Err(MixerError::UnknownSource) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn clear_voices(&mut self) -> Result<(), MixerError> {
        if let Some(voices) = &self.synthetic {
            voices.clear();
        }
        for user_id in std::mem::take(&mut self.sources) {
            self.remove_voice(&user_id)?;
        }
        Ok(())
    }

    fn announce(&self, members: Vec<RemoteMember>) {
        if let Some(listener) = &self.listener {
            listener(Participants {
                self_id: self.self_id.clone(),
                members,
            });
        }
    }
}

/// Shared handle to the running engine. Cheap to clone.
#[derive(Clone)]
pub struct Engine {
    live: Arc<Mutex<Live>>,
    /// Serializes join/leave: each one awaits network I/O, and two overlapping joins
    /// must not both try to take the microphone.
    transitions: Arc<tokio::sync::Mutex<()>>,
}

fn mixer_error(e: MixerError) -> EngineError {
    format!("Микшер: {e}")
}

fn lock(live: &Mutex<Live>) -> MutexGuard<'_, Live> {
    // Engine state stays consistent even if a holder panicked mid-call: every
    // mutation either completes or returns before touching shared state.
    live.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Engine {
    /// Opens the default output device.
    ///
    /// # Errors
    /// When no usable output device exists; the app keeps running without audio.
    pub fn start() -> Result<Self, EngineError> {
        let opened = output::open::<SourceKey>().map_err(|e| format!("Звук недоступен: {e}"))?;
        let output = opened.output;
        let synthetic = std::env::var("OC_VOICE")
            .is_ok_and(|v| v == "synthetic")
            .then(|| SyntheticVoices::start(output.sample_rate()));
        eprintln!(
            "[oc-audio] output: {} @ {} Hz, voice: {}",
            output.device_name(),
            output.sample_rate(),
            if synthetic.is_some() {
                "synthetic"
            } else {
                "livekit"
            },
        );
        Ok(Self {
            live: Arc::new(Mutex::new(Live {
                synthetic,
                mixer: opened.mixer,
                sources: Vec::new(),
                prefs: HashMap::new(),
                mic_muted: false,
                taps: Arc::new(Mutex::new(Some(opened.tap))),
                session: None,
                local_level: None,
                listener: None,
                disconnect_listener: None,
                link_listener: None,
                next_session_id: 0,
                // Set when a camp is opened (`set_self_id`).
                self_id: String::new(),
                output_latency: output.latency(),
                output_rate: output.sample_rate(),
                _output: output,
            })),
            transitions: Arc::default(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Live> {
        lock(&self.live)
    }

    /// Where participant updates go.
    pub fn set_participants_listener(&self, listener: ParticipantsListener) {
        self.lock().listener = Some(listener);
    }

    /// Where "the server ended your session" goes.
    pub fn set_disconnect_listener(&self, listener: DisconnectListener) {
        self.lock().disconnect_listener = Some(listener);
    }

    pub fn set_link_listener(&self, listener: LinkListener) {
        self.lock().link_listener = Some(listener);
    }

    /// Whether joining needs a room ([`RoomAccess`]) — false for synthetic voices.
    #[must_use]
    pub fn needs_room(&self) -> bool {
        self.lock().synthetic.is_none()
    }

    /// Who the local user is in the open camp (levels and participant updates use it).
    pub fn set_self_id(&self, user_id: &str) {
        user_id.clone_into(&mut self.lock().self_id);
    }

    /// Leaves the current channel (if any) and joins the room `access` points to (`None`
    /// in synthetic mode). `members` carries the saved volumes of people the UI expects.
    ///
    /// # Errors
    /// No microphone, no room access, or the mixer refused a source.
    pub async fn join_channel(
        &self,
        access: Option<RoomAccess>,
        members: &[MemberSpec],
    ) -> Result<(), EngineError> {
        let _transition = self.transitions.lock().await;
        self.close_session().await?;

        let synthetic = {
            let mut live = self.lock();
            for spec in members {
                live.prefs.insert(
                    spec.user_id.clone(),
                    Prefs {
                        volume_percent: spec.volume_percent,
                        muted: spec.muted,
                    },
                );
            }
            live.synthetic.is_some()
        };
        match (synthetic, access) {
            (true, _) => self.join_synthetic(members),
            (false, Some(access)) => self.join_room(access).await,
            (false, None) => Err("Нет доступа к комнате".into()),
        }
    }

    fn join_synthetic(&self, members: &[MemberSpec]) -> Result<(), EngineError> {
        let mut live = self.lock();
        for spec in members {
            let input = live.add_voice(&spec.user_id).map_err(mixer_error)?;
            // A muted member still has a source (they may unmute), it is just silent.
            if !spec.remote_mic_muted {
                if let Some(voices) = &live.synthetic {
                    voices.add(&spec.user_id, input);
                }
            }
        }
        live.announce(
            members
                .iter()
                .map(|m| RemoteMember {
                    user_id: m.user_id.clone(),
                    name: m.user_id.clone(),
                    mic_muted: m.remote_mic_muted,
                })
                .collect(),
        );
        Ok(())
    }

    async fn join_room(&self, access: RoomAccess) -> Result<(), EngineError> {
        let (taps, output_latency, mic_muted, playback_rate) = {
            let live = self.lock();
            (
                Arc::clone(&live.taps),
                live.output_latency.clone(),
                live.mic_muted,
                live.output_rate,
            )
        };
        // If anything below fails or this future is dropped, dropping `mic` returns the
        // tap to `taps` (see `TapSlot`).
        let mic = MicPipeline::start(&taps, output_latency, mic_muted)
            .map_err(|message| format!("Микрофон: {message}"))?;
        let session_id = {
            let mut live = self.lock();
            live.next_session_id += 1;
            live.next_session_id
        };
        let host: Arc<dyn RoomHost> = Arc::new(Host {
            live: Arc::downgrade(&self.live),
            transitions: Arc::clone(&self.transitions),
            session_id,
        });
        let params = RoomParams {
            url: access.url,
            token: access.token,
            playback_rate,
        };
        match VoiceRoom::connect(params, mic.source(), mic_muted, host).await {
            Ok(room) => {
                let mut live = self.lock();
                live.local_level = Some(mic.level());
                live.session = Some(Session {
                    id: session_id,
                    room,
                    mic,
                });
                Ok(())
            }
            Err(e) => {
                mic.stop();
                Err(e)
            }
        }
    }

    /// Leaves the channel.
    ///
    /// # Errors
    /// If the mixer command queue is full.
    pub async fn leave_channel(&self) -> Result<(), EngineError> {
        let _transition = self.transitions.lock().await;
        self.close_session().await
    }

    async fn close_session(&self) -> Result<(), EngineError> {
        let session = {
            let mut live = self.lock();
            live.local_level = None;
            live.session.take()
        };
        if let Some(Session { room, mic, .. }) = session {
            room.close().await;
            mic.stop();
        }
        let mut live = self.lock();
        let result = live.clear_voices().map_err(mixer_error);
        live.announce(Vec::new());
        result
    }

    /// Sets a member's local volume, now and for whenever they (re)appear.
    ///
    /// # Errors
    /// Never in practice: unknown members only have their preference stored.
    pub fn set_volume(&self, user_id: &str, percent: f32) -> Result<(), EngineError> {
        let mut live = self.lock();
        live.prefs.entry(user_id.into()).or_default().volume_percent = percent;
        match live
            .mixer
            .set_gain(&SourceKey::Voice(user_id.into()), percent_to_gain(percent))
        {
            Ok(()) | Err(MixerError::UnknownSource) => Ok(()),
            Err(e) => Err(mixer_error(e)),
        }
    }

    /// Locally mutes or unmutes a member, now and for whenever they (re)appear.
    ///
    /// # Errors
    /// Never in practice: unknown members only have their preference stored.
    pub fn set_local_mute(&self, user_id: &str, muted: bool) -> Result<(), EngineError> {
        let mut live = self.lock();
        live.prefs.entry(user_id.into()).or_default().muted = muted;
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

    /// Mutes the microphone; the room shows us as muted.
    pub fn set_mic_muted(&self, muted: bool) {
        let mut live = self.lock();
        live.mic_muted = muted;
        if let Some(session) = &live.session {
            session.mic.set_muted(muted);
            session.room.set_mic_muted(muted);
        }
    }

    /// Levels of everyone in the channel, the local user included.
    #[must_use]
    pub fn levels(&self) -> Vec<MemberLevel> {
        let live = self.lock();
        let remote = live.sources.iter().filter_map(|id| {
            let SourceStatus {
                level_db, speaking, ..
            } = live.mixer.status(&SourceKey::Voice(id.clone())).ok()?;
            Some(MemberLevel {
                user_id: id.clone(),
                level_db,
                speaking,
            })
        });
        let local = live.local_level.as_ref().map(|level| {
            let (level_db, speaking) = level.get();
            MemberLevel {
                user_id: live.self_id.clone(),
                level_db,
                speaking,
            }
        });
        remote.chain(local).collect()
    }
}

/// The engine as seen by a room. Weak: the room's event task must not keep the
/// engine (and with it the room) alive.
struct Host {
    live: Weak<Mutex<Live>>,
    transitions: Arc<tokio::sync::Mutex<()>>,
    session_id: u64,
}

impl RoomHost for Host {
    fn voice_started(&self, user_id: &str) -> Option<SourceInput> {
        let live = self.live.upgrade()?;
        let mut live = lock(&live);
        // A stale source (reconnect) is replaced rather than duplicated.
        let _ = live.remove_voice(user_id);
        match live.add_voice(user_id) {
            Ok(input) => Some(input),
            Err(e) => {
                eprintln!("[oc-voice] cannot play {user_id}: {e}");
                None
            }
        }
    }

    fn voice_stopped(&self, user_id: &str) {
        if let Some(live) = self.live.upgrade() {
            if let Err(e) = lock(&live).remove_voice(user_id) {
                eprintln!("[oc-voice] cannot stop {user_id}: {e}");
            }
        }
    }

    fn members_changed(&self, members: Vec<RemoteMember>) {
        if let Some(live) = self.live.upgrade() {
            lock(&live).announce(members);
        }
    }

    fn link_changed(&self, link: Link) {
        let Some(live) = self.live.upgrade() else {
            return;
        };
        let live = lock(&live);
        // A room being replaced may still report once; only the current one counts.
        if live
            .session
            .as_ref()
            .is_some_and(|s| s.id == self.session_id)
        {
            if let Some(listener) = &live.link_listener {
                listener(link);
            }
        }
    }

    fn disconnected(&self, message: String) {
        let (live, transitions, id) = (
            self.live.clone(),
            Arc::clone(&self.transitions),
            self.session_id,
        );
        // Not inline: we are on the room's event task, which closing the room awaits.
        tauri::async_runtime::spawn(async move {
            let Some(live) = live.upgrade() else { return };
            let _transition = transitions.lock().await;
            let session = {
                let mut live = lock(&live);
                if live.session.as_ref().is_none_or(|s| s.id != id) {
                    return; // already left, or a newer session replaced it
                }
                live.local_level = None;
                live.session.take()
            };
            if let Some(Session { room, mic, .. }) = session {
                room.close().await;
                mic.stop();
            }
            let live = &mut lock(&live);
            if let Err(e) = live.clear_voices() {
                eprintln!("[oc-voice] cleanup after disconnect: {e}");
            }
            live.announce(Vec::new());
            if let Some(listener) = &live.disconnect_listener {
                listener(message);
            }
        });
    }
}
