//! The voice engine as seen by the UI: who is in the channel, their volumes, deafen.
//!
//! Two modes behind the same calls:
//! * **LiveKit** (default): joining a channel opens the microphone pipeline and the
//!   room; remote microphones become mixer sources as they arrive.
//! * **Synthetic** (`OC_VOICE=synthetic`): the M0 stand-in voices, for testing volume
//!   and the speaking ring without a second person or a server.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use oc_audio::gain::percent_to_gain;
use oc_audio::meter::SILENCE_DB;
use oc_audio::mixer::{Layout, MixerControl, MixerError, SourceInput, SourceStatus};

use super::devices::{self, Alarm, Listing, MicMode, SoundPrefs, TalkKey, TALK_KEY_WORKS};
use super::hotkeys::{self, Action, Hotkey, HotkeyControl};
use super::output::{self, DeviceLatency, Output};
use super::synthetic::SyntheticVoices;
use super::SourceKey;
use crate::stream::{OwnStream, ScreenStream, SourceKind, StreamQuality, ViewStatus, Viewer};
use crate::voice::mic::{install_tap, GateControl, LocalLevel, MicPipeline, TapSlot};
use crate::voice::room::{Link, RemoteMember, RoomAccess, RoomHost, RoomParams, VoiceRoom};

/// How often the watcher looks at the system's devices (ADR-0066).
const LOOK_EVERY: Duration = Duration::from_secs(3);

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

/// Who the viewer's window shows, for its page.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamView {
    pub user_id: String,
    pub name: String,
    pub status: ViewStatus,
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

/// Receives how your own stream is going (ADR-0054).
pub type StreamListener = Arc<dyn Fn(OwnStream) + Send + Sync>;

/// What the settings window shows: what the system has, and what was chosen.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundView {
    pub outputs: Vec<String>,
    pub default_output: Option<String>,
    pub microphones: Vec<String>,
    pub default_microphone: Option<String>,
    pub output: Option<String>,
    pub microphone: Option<String>,
    pub volume: f32,
    pub mic_mode: MicMode,
    pub voice_threshold: f32,
    pub talk_key: Option<TalkKey>,
    pub mute_key: Option<Hotkey>,
    pub deafen_key: Option<Hotkey>,
    /// A key can be read on this system.
    pub talk_key_works: bool,
}

/// What the microphone hears now, for the settings' bar (ADR-0069).
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicInput {
    pub level_db: f32,
    /// The gate is open at this level.
    pub open: bool,
}

/// Receives the sound settings whenever the system's devices change.
pub type SoundListener = Arc<dyn Fn(SoundView) + Send + Sync>;

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

/// A stream going out, and which one: an old stream's end must not stop its replacement.
struct Outgoing {
    serial: u64,
    stream: ScreenStream,
}

struct Session {
    /// Tells a late server disconnect apart from the session that replaced it.
    id: u64,
    room: VoiceRoom,
    mic: MicPipeline,
    /// The screen stream going out in this session (ADR-0049).
    stream: Option<Outgoing>,
    /// Someone's stream open in the viewer's window (ADR-0053). Only in this session's
    /// room, so it ends with it.
    viewer: Option<Viewer>,
}

impl Session {
    /// What goes out and comes in stops first, the room after.
    async fn close(self) {
        if let Some(viewer) = self.viewer {
            viewer.close().await;
        }
        if let Some(outgoing) = self.stream {
            outgoing.stream.stop().await;
        }
        self.room.close().await;
        self.mic.stop();
    }
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
    stream_listener: Option<StreamListener>,
    /// Local volume of the sound of streams, percent; the viewer's page sets it.
    stream_volume: f32,
    link: LinkRelay,
    next_session_id: u64,
    next_stream: u64,
    deafened: bool,
    /// What the user chose, and where it is kept (no folder: not kept).
    sound: SoundPrefs,
    sound_dir: Option<PathBuf>,
    /// One for the engine's life: every microphone opened reads it.
    gate: Arc<GateControl>,
    hotkeys: Arc<HotkeyControl>,
    /// The settings' bar is on screen (ADR-0069). Outside a channel it needs a microphone of
    /// its own, `check`, which sends nothing; a channel's takes over while there is one.
    checking: bool,
    check: Option<MicPipeline>,
    alarm: Arc<Alarm>,
    /// One for the engine's life: the microphone keeps reading it across outputs.
    output_latency: DeviceLatency,
    output_rate: u32,
    output: Output,
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

    fn announce_stream(&self, live: bool, note: Option<String>) {
        if let Some(listener) = &self.stream_listener {
            listener(OwnStream { live, note });
        }
    }

    /// The microphone open now, if any.
    fn open_mic(&self) -> Option<&MicPipeline> {
        self.session
            .as_ref()
            .map(|session| &session.mic)
            .or(self.check.as_ref())
    }

    /// Hands the saved settings to the gate.
    fn apply_gate(&self) {
        // A file copied from a system where a key works must not leave the microphone shut.
        self.gate
            .set_key_mode(TALK_KEY_WORKS && self.sound.mic_mode == MicMode::Key);
        self.gate
            .set_talk_key(self.sound.talk_key.as_ref().map(|key| key.code));
        self.gate.set_threshold(self.sound.voice_threshold);
    }

    /// Hands the saved keys to the thread reading them.
    fn apply_hotkeys(&self) {
        self.hotkeys.set(Action::Mic, self.sound.mute_key.as_ref());
        self.hotkeys
            .set(Action::Sound, self.sound.deafen_key.as_ref());
    }
}

/// Where the viewer puts the sound of the stream it shows (ADR-0055).
#[derive(Clone)]
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "the viewer is Windows only until step 6")
)]
pub struct StreamOutlet {
    live: Weak<Mutex<Live>>,
}

#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "the viewer is Windows only until step 6")
)]
impl StreamOutlet {
    /// A mixer source for the sound, replacing a stale one, and the rate the mixer plays at.
    /// `None` if the mixer refused.
    pub fn open(&self) -> Option<(SourceInput, u32)> {
        let live = self.live.upgrade()?;
        let mut live = lock(&live);
        let gain = percent_to_gain(live.stream_volume);
        let _ = live.mixer.remove_source(&SourceKey::Stream);
        match live
            .mixer
            .add_source(SourceKey::Stream, Layout::Stereo, gain)
        {
            Ok(input) => Some((input, live.output_rate)),
            Err(e) => {
                eprintln!("[oc-stream] cannot play the sound: {e}");
                None
            }
        }
    }

    /// The sound is over.
    pub fn close(&self) {
        if let Some(live) = self.live.upgrade() {
            let _ = lock(&live).mixer.remove_source(&SourceKey::Stream);
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
        let alarm = Arc::<Alarm>::default();
        let output_latency = DeviceLatency::default();
        let opened = output::open::<SourceKey>(None, &output_latency, &alarm)
            .map_err(|e| format!("Звук недоступен: {e}"))?;
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
                stream_listener: None,
                stream_volume: 100.0,
                link: LinkRelay::default(),
                next_session_id: 0,
                next_stream: 0,
                // Set when a camp is opened (`set_self_id`).
                self_id: String::new(),
                deafened: false,
                sound: SoundPrefs::default(),
                sound_dir: None,
                gate: Arc::default(),
                hotkeys: Arc::default(),
                checking: false,
                check: None,
                alarm,
                output_latency,
                output_rate: output.sample_rate(),
                output,
            })),
            transitions: Arc::default(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Live> {
        lock(&self.live)
    }

    /// Takes up what was chosen earlier, from the app's folder. Once, while nothing plays yet.
    pub fn load_sound(&self, dir: PathBuf) {
        let sound = SoundPrefs::load(&dir);
        let elsewhere = sound.output.is_some();
        {
            let mut live = self.lock();
            let gain = percent_to_gain(sound.volume);
            if let Err(e) = live.mixer.set_master_gain(gain) {
                eprintln!("[oc-audio] volume: {e}");
            }
            live.sound = sound;
            live.sound_dir = Some(dir);
            live.apply_gate();
        }
        if elsewhere && self.needs_room() {
            if let Err(e) = self.replace_output() {
                eprintln!("[oc-audio] {e}");
            }
        }
    }

    fn save_sound(&self) {
        let (sound, dir) = {
            let live = self.lock();
            (live.sound.clone(), live.sound_dir.clone())
        };
        if let Some(dir) = dir {
            if let Err(e) = sound.save(&dir) {
                eprintln!("[oc-audio] saving the sound settings: {e}");
            }
        }
    }

    fn sound_view(&self, listing: Listing) -> SoundView {
        let live = self.lock();
        SoundView {
            outputs: listing.outputs,
            default_output: listing.default_output,
            microphones: listing.microphones,
            default_microphone: listing.default_microphone,
            output: live.sound.output.clone(),
            microphone: live.sound.microphone.clone(),
            volume: live.sound.volume,
            mic_mode: live.sound.mic_mode,
            voice_threshold: live.sound.voice_threshold,
            talk_key: live.sound.talk_key.clone(),
            mute_key: live.sound.mute_key.clone(),
            deafen_key: live.sound.deafen_key.clone(),
            talk_key_works: TALK_KEY_WORKS,
        }
    }

    /// The sound settings as they are now.
    ///
    /// # Errors
    /// If the task asking the system died.
    pub async fn sound(&self) -> Result<SoundView, EngineError> {
        Ok(self.sound_view(listing().await?))
    }

    /// Chooses the output (`None`: as in the system) and moves to it, if that is another device.
    ///
    /// # Errors
    /// The device could not be opened; the old one keeps playing and nothing is saved.
    pub async fn set_output(&self, chosen: Option<String>) -> Result<SoundView, EngineError> {
        if !self.needs_room() {
            return Err("С OC_VOICE=synthetic устройство не меняется".to_owned());
        }
        let listing = listing().await?;
        let _transition = self.transitions.lock().await;
        let (previous, moving) = {
            let mut live = self.lock();
            let previous = std::mem::replace(&mut live.sound.output, chosen);
            let moving = devices::pick(
                live.sound.output.as_deref(),
                &listing.outputs,
                listing.default_output.as_deref(),
            )
            .is_some_and(|name| name != live.output.device_name());
            (previous, moving)
        };
        if moving {
            if let Err(e) = self.replace_output() {
                self.lock().sound.output = previous;
                return Err(e);
            }
        }
        self.save_sound();
        Ok(self.sound_view(listing))
    }

    /// Chooses the microphone (`None`: as in the system). In a channel it moves there at once;
    /// otherwise the next channel opens it.
    ///
    /// # Errors
    /// The device could not be opened or published; the choice stays, and the watcher tries again.
    pub async fn set_microphone(&self, chosen: Option<String>) -> Result<SoundView, EngineError> {
        let listing = listing().await?;
        let _transition = self.transitions.lock().await;
        let moving = {
            let mut live = self.lock();
            live.sound.microphone = chosen;
            let wanted = devices::pick(
                live.sound.microphone.as_deref(),
                &listing.microphones,
                listing.default_microphone.as_deref(),
            );
            live.open_mic()
                .is_some_and(|mic| wanted.is_some_and(|name| name != mic.device_name()))
        };
        self.save_sound();
        if moving {
            if let Err(e) = self.replace_mic().await {
                self.lock().alarm.retry_microphone();
                return Err(e);
            }
        }
        Ok(self.sound_view(listing))
    }

    /// Sets the overall volume, percent. `save`: the hand is off the slider, keep it.
    ///
    /// # Errors
    /// If the mixer command queue is full.
    pub fn set_sound_volume(&self, percent: f32, save: bool) -> Result<(), EngineError> {
        {
            let mut live = self.lock();
            live.sound.volume = percent;
            live.mixer
                .set_master_gain(percent_to_gain(percent))
                .map_err(mixer_error)?;
        }
        if save {
            self.save_sound();
        }
        Ok(())
    }

    /// Chooses how the microphone goes on the air (ADR-0069).
    ///
    /// # Errors
    /// If the task asking the system died.
    pub async fn set_mic_mode(&self, mode: MicMode) -> Result<SoundView, EngineError> {
        let listing = listing().await?;
        {
            let mut live = self.lock();
            live.sound.mic_mode = mode;
            live.apply_gate();
        }
        self.save_sound();
        Ok(self.sound_view(listing))
    }

    /// Chooses the key that opens the microphone.
    ///
    /// # Errors
    /// If the task asking the system died.
    pub async fn set_talk_key(&self, key: Option<TalkKey>) -> Result<SoundView, EngineError> {
        let listing = listing().await?;
        {
            let mut live = self.lock();
            live.sound.talk_key = key;
            live.apply_gate();
        }
        self.save_sound();
        Ok(self.sound_view(listing))
    }

    /// Chooses the key for an action; none takes it away (ADR-0071).
    ///
    /// # Errors
    /// If the task asking the system died.
    pub async fn set_hotkey(
        &self,
        action: Action,
        key: Option<Hotkey>,
    ) -> Result<SoundView, EngineError> {
        let listing = listing().await?;
        {
            let mut live = self.lock();
            match action {
                Action::Mic => live.sound.mute_key = key,
                Action::Sound => live.sound.deafen_key = key,
            }
            live.apply_hotkeys();
        }
        self.save_sound();
        Ok(self.sound_view(listing))
    }

    /// Starts watching the assigned keys, where they can be read.
    pub fn watch_hotkeys(&self, on_press: impl Fn(Action) + Send + 'static) {
        if !TALK_KEY_WORKS {
            return;
        }
        let control = {
            let live = self.lock();
            live.apply_hotkeys();
            Arc::clone(&live.hotkeys)
        };
        hotkeys::watch(control, on_press);
    }

    /// Sets where the voice opens the gate, dBFS. `save`: the hand is off the slider, keep it.
    pub fn set_voice_threshold(&self, db: f32, save: bool) {
        {
            let mut live = self.lock();
            live.sound.voice_threshold = db;
            live.apply_gate();
        }
        if save {
            self.save_sound();
        }
    }

    /// The settings' bar came on screen or left it. Outside a channel this opens or closes
    /// the microphone, so it stays open no longer than someone is looking at the bar.
    ///
    /// # Errors
    /// The microphone could not be opened; the watcher tries again.
    pub async fn mic_check(&self, on: bool) -> Result<(), EngineError> {
        if !self.needs_room() {
            return Ok(());
        }
        let _transition = self.transitions.lock().await;
        self.lock().checking = on;
        if on {
            self.resume_check()
        } else {
            self.end_check();
            Ok(())
        }
    }

    /// Opens the settings' microphone if it is wanted and no channel has one. Holds `transitions`.
    fn resume_check(&self) -> Result<(), EngineError> {
        let (chosen, taps, latency, gate, alarm) = {
            let live = self.lock();
            if !live.checking || live.open_mic().is_some() {
                return Ok(());
            }
            (
                live.sound.microphone.clone(),
                Arc::clone(&live.taps),
                live.output_latency.clone(),
                Arc::clone(&live.gate),
                Arc::clone(&live.alarm),
            )
        };
        // Muted for good: frames never leave it.
        let mic = MicPipeline::start(chosen, &taps, latency, true, gate, alarm)
            .map_err(|message| format!("Микрофон: {message}"))?;
        self.lock().check = Some(mic);
        Ok(())
    }

    /// Closes the settings' microphone, if open. The tap is free when this returns.
    fn end_check(&self) {
        let check = self.lock().check.take();
        // Outside the lock: it waits for the device's thread.
        if let Some(check) = check {
            check.stop();
        }
    }

    /// What the microphone hears now, for the settings' bar.
    #[must_use]
    pub fn mic_input(&self) -> MicInput {
        let (level_db, open) = self
            .lock()
            .open_mic()
            .map_or((SILENCE_DB, false), |mic| mic.level().input());
        MicInput { level_db, open }
    }

    /// Opens the chosen output with a new mixer, and everything playing moves there. A new
    /// mixer, because a device's rate is the mixer's and the sources' (ADR-0066). The old
    /// output plays until the new one is up, so a failure costs nothing. Holds `transitions`.
    fn replace_output(&self) -> Result<(), EngineError> {
        let (chosen, latency, alarm, volume, deafened) = {
            let live = self.lock();
            (
                live.sound.output.clone(),
                live.output_latency.clone(),
                Arc::clone(&live.alarm),
                live.sound.volume,
                live.deafened,
            )
        };
        let opened = output::open::<SourceKey>(chosen.as_deref(), &latency, &alarm)
            .map_err(|e| format!("Звук: {e}"))?;
        let mut mixer = opened.mixer;
        mixer
            .set_master_gain(percent_to_gain(volume))
            .and_then(|()| mixer.set_master_muted(deafened))
            .map_err(mixer_error)?;
        let rate = opened.output.sample_rate();
        eprintln!(
            "[oc-audio] output: {} @ {rate} Hz",
            opened.output.device_name()
        );
        let old = {
            let mut live = self.lock();
            live.mixer = mixer;
            live.sources.clear();
            live.output_rate = rate;
            install_tap(&live.taps, opened.tap);
            if let Some(session) = &live.session {
                session.room.replay(rate);
                if let Some(viewer) = &session.viewer {
                    viewer.mixer_replaced();
                }
            }
            std::mem::replace(&mut live.output, opened.output)
        };
        // Outside the lock: it waits for the device's thread.
        drop(old);
        Ok(())
    }

    /// Opens the chosen microphone and puts it in the room. A new track, because a device's
    /// rate is its source's (ADR-0066). The old one has to let go of the output's tap first,
    /// and goes silent even if the new one fails. Holds `transitions`.
    async fn replace_mic(&self) -> Result<(), EngineError> {
        if self.lock().session.is_none() {
            self.end_check();
            return self.resume_check();
        }
        let (chosen, taps, latency, muted, gate, alarm) = {
            let mut live = self.lock();
            let Some(session) = live.session.as_mut() else {
                return Ok(());
            };
            session.mic.halt();
            (
                live.sound.microphone.clone(),
                Arc::clone(&live.taps),
                live.output_latency.clone(),
                live.mic_muted,
                Arc::clone(&live.gate),
                Arc::clone(&live.alarm),
            )
        };
        let mic = MicPipeline::start(chosen, &taps, latency, muted, gate, alarm)
            .map_err(|message| format!("Микрофон: {message}"))?;
        let published = {
            let mut live = self.lock();
            mic.set_muted(live.mic_muted);
            live.local_level = Some(mic.level());
            let session = live.session.as_mut().ok_or("Сначала зайдите в канал")?;
            let published = session.room.replace_mic(mic.source());
            session.mic = mic;
            published
        };
        published
            .await
            .map_err(|e| format!("Не удалось включить микрофон в комнате: {e}"))
    }

    /// Brings what is open in line with the system: the chosen device came back or left, the
    /// system's default moved, a device failed. A device that could not be opened is tried
    /// again at the next look.
    async fn reconcile(&self, listing: &Listing) -> Result<(), EngineError> {
        let _transition = self.transitions.lock().await;
        let (alarm, output_moves, mic_moves) = {
            let live = self.lock();
            let alarm = Arc::clone(&live.alarm);
            let output = devices::pick(
                live.sound.output.as_deref(),
                &listing.outputs,
                listing.default_output.as_deref(),
            );
            let mic = devices::pick(
                live.sound.microphone.as_deref(),
                &listing.microphones,
                listing.default_microphone.as_deref(),
            );
            let output_moves = match output {
                Some(name) => alarm.take_output() || name != live.output.device_name(),
                None => false,
            };
            let mic_moves = match mic {
                Some(name) if live.session.is_some() || live.checking => {
                    alarm.take_microphone()
                        || live
                            .open_mic()
                            .is_none_or(|open| name != open.device_name())
                }
                _ => false,
            };
            (alarm, output_moves, mic_moves)
        };
        if output_moves {
            self.replace_output()
                .inspect_err(|_| alarm.retry_output())?;
        }
        if mic_moves {
            let moved = self.replace_mic().await;
            moved.inspect_err(|_| alarm.retry_microphone())?;
        }
        Ok(())
    }

    /// Follows the system's devices for as long as the app lives (ADR-0066). `cpal` tells only
    /// of a device failing, so the rest is found by looking now and then.
    pub fn watch_devices(&self, listener: SoundListener) {
        if !self.needs_room() {
            return;
        }
        let engine = self.clone();
        let alarm = Arc::clone(&self.lock().alarm);
        tauri::async_runtime::spawn(async move {
            let Ok(mut seen) = on_own_thread(devices::glance).await else {
                return;
            };
            let Ok(mut known) = listing().await else {
                return;
            };
            let mut problem = None;
            loop {
                let failed = tokio::select! {
                    () = tokio::time::sleep(LOOK_EVERY) => false,
                    () = alarm.raised() => true,
                };
                let Ok(glance) = on_own_thread(devices::glance).await else {
                    continue;
                };
                if failed || glance != seen {
                    let Ok(now) = listing().await else { continue };
                    seen = glance;
                    if now != known {
                        listener(engine.sound_view(now.clone()));
                        known = now;
                    }
                }
                // Each time: a device that could not be opened is tried again.
                let result = engine.reconcile(&known).await.err();
                // Once for each new problem: it may stay for hours.
                if result != problem {
                    if let Some(e) = &result {
                        eprintln!("[oc-audio] following the devices: {e}");
                    }
                    problem = result;
                }
            }
        });
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

    pub fn set_stream_listener(&self, listener: StreamListener) {
        self.lock().stream_listener = Some(listener);
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
        // The channel's microphone needs the tap, and a device is opened by one at a time.
        self.end_check();

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
        let joined = match (synthetic, access) {
            (true, _) => self.join_synthetic(members),
            (false, Some(access)) => self.join_room(access).await,
            (false, None) => Err("Нет доступа к комнате".into()),
        };
        if joined.is_err() {
            self.check_again();
        }
        joined
    }

    /// No channel any more: the settings' bar, if on screen, needs its own microphone back.
    fn check_again(&self) {
        if let Err(e) = self.resume_check() {
            self.lock().alarm.retry_microphone();
            eprintln!("[oc-voice] {e}");
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
        let (taps, output_latency, mic_muted, playback_rate, device, gate, alarm) = {
            let live = self.lock();
            live.alarm.take_microphone();
            (
                Arc::clone(&live.taps),
                live.output_latency.clone(),
                live.mic_muted,
                live.output_rate,
                live.sound.microphone.clone(),
                Arc::clone(&live.gate),
                Arc::clone(&live.alarm),
            )
        };
        // If anything below fails or this future is dropped, dropping `mic` returns the
        // tap to `taps` (see `TapSlot`).
        let mic = MicPipeline::start(device, &taps, output_latency, mic_muted, gate, alarm)
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
                    stream: None,
                    viewer: None,
                });
                // The room measured itself while connecting, before it was ours to report.
                if let (Some(link), Some(listener)) =
                    (live.link.installed(session_id), &live.link_listener)
                {
                    listener(link);
                }
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
        let closed = self.close_session().await;
        self.check_again();
        closed
    }

    async fn close_session(&self) -> Result<(), EngineError> {
        let session = {
            let mut live = self.lock();
            live.local_level = None;
            live.session.take()
        };
        if let Some(session) = session {
            session.close().await;
        }
        let mut live = self.lock();
        let result = live.clear_voices().map_err(mixer_error);
        live.announce(Vec::new());
        live.announce_stream(false, None);
        result
    }

    /// Starts streaming a screen or window into the channel, replacing a stream already
    /// going (ADR-0049).
    ///
    /// # Errors
    /// Not in a channel, or the stream could not start; as text for the UI.
    pub async fn start_stream(
        &self,
        kind: SourceKind,
        id: u64,
        quality: StreamQuality,
        sound: bool,
    ) -> Result<(), String> {
        let _transition = self.transitions.lock().await;
        let (room, previous) = {
            let mut live = self.lock();
            let session = live.session.as_mut().ok_or("Сначала зайдите в канал")?;
            (session.room.room(), session.stream.take())
        };
        if let Some(previous) = previous {
            previous.stream.stop().await;
        }
        let mut stream = match ScreenStream::start(room, kind, id, quality, sound).await {
            Ok(stream) => stream,
            Err(e) => {
                self.lock().announce_stream(false, None);
                return Err(e);
            }
        };
        let ended = stream.ended();
        let note = stream.sound_problem();
        let serial = {
            let mut live = self.lock();
            live.next_stream += 1;
            let serial = live.next_stream;
            // Leaving takes `transitions` too, so the session is the same one.
            if let Some(session) = live.session.as_mut() {
                session.stream = Some(Outgoing { serial, stream });
            }
            live.announce_stream(true, note);
            serial
        };
        let engine = self.clone();
        tauri::async_runtime::spawn(async move {
            let _ = ended.await;
            engine.stream_ended(serial).await;
        });
        Ok(())
    }

    /// The stream's thread is over. If nobody stopped it, its source is gone or its encoder
    /// gave up: viewers must not stay on a frozen picture, and you must hear about it.
    async fn stream_ended(&self, serial: u64) {
        let _transition = self.transitions.lock().await;
        let outgoing = {
            let mut live = self.lock();
            let Some(session) = live.session.as_mut() else {
                return;
            };
            if session.stream.as_ref().is_none_or(|o| o.serial != serial) {
                return;
            }
            session.stream.take()
        };
        if let Some(outgoing) = outgoing {
            outgoing.stream.stop().await;
            self.lock().announce_stream(
                false,
                Some("Стрим остановился: окно закрыто или экран недоступен".to_owned()),
            );
        }
    }

    /// Stops the stream, if one is going.
    pub async fn stop_stream(&self) {
        let _transition = self.transitions.lock().await;
        let outgoing = self.lock().session.as_mut().and_then(|s| s.stream.take());
        if let Some(outgoing) = outgoing {
            outgoing.stream.stop().await;
        }
        self.lock().announce_stream(false, None);
    }

    /// Opens someone's stream in the viewer's window; only one at a time, so a stream
    /// already open closes first. Clicking the one already open brings its window up.
    ///
    /// # Errors
    /// Not in a channel, or no window; as text for the UI.
    pub async fn watch_stream(
        &self,
        app: &tauri::AppHandle,
        user_id: String,
        name: String,
    ) -> Result<(), String> {
        let _transition = self.transitions.lock().await;
        let (room, previous) = {
            let mut live = self.lock();
            let session = live.session.as_mut().ok_or("Сначала зайдите в канал")?;
            if let Some(viewer) = session.viewer.as_ref().filter(|v| v.user_id() == user_id) {
                viewer.raise();
                return Ok(());
            }
            (session.room.room(), session.viewer.take())
        };
        if let Some(previous) = previous {
            previous.close().await;
        }
        let window = crate::windows::open_stream(app, &name)
            .map_err(|e| format!("Не удалось открыть окно стрима: {e}"))?;
        let outlet = StreamOutlet {
            live: Arc::downgrade(&self.live),
        };
        let viewer = match Viewer::start(room, user_id, name, window.clone(), outlet).await {
            Ok(viewer) => viewer,
            Err(e) => {
                let _ = window.destroy();
                return Err(e);
            }
        };
        // Leaving takes `transitions` too, so the session is the same one.
        let mut live = self.lock();
        match live.session.as_mut() {
            Some(session) => {
                viewer.announce();
                session.viewer = Some(viewer);
            }
            None => return Err("Сначала зайдите в канал".to_owned()),
        }
        Ok(())
    }

    /// Closes the viewer's window. Whether a viewer was there to close it.
    pub async fn stop_watching(&self) -> bool {
        let _transition = self.transitions.lock().await;
        let viewer = self.lock().session.as_mut().and_then(|s| s.viewer.take());
        let Some(viewer) = viewer else { return false };
        viewer.close().await;
        true
    }

    /// Who is being watched, and the panel's numbers, for the viewer's page.
    pub fn stream_view(&self) -> Option<StreamView> {
        let live = self.lock();
        let viewer = live.session.as_ref()?.viewer.as_ref()?;
        Some(StreamView {
            user_id: viewer.user_id().to_owned(),
            name: viewer.name().to_owned(),
            status: viewer.status(),
        })
    }

    /// Where the page wants the picture, in the window's pixels.
    pub fn place_stream(&self, x: i32, y: i32, width: u32, height: u32) {
        let live = self.lock();
        if let Some(viewer) = live.session.as_ref().and_then(|s| s.viewer.as_ref()) {
            viewer.place(x, y, width, height);
        }
    }

    /// Sets the local volume of the sound of streams, now and for the next one.
    pub fn set_stream_volume(&self, percent: f32) {
        let mut live = self.lock();
        live.stream_volume = percent;
        // No sound playing yet is fine: `StreamOutlet::open` reads the value.
        let _ = live
            .mixer
            .set_gain(&SourceKey::Stream, percent_to_gain(percent));
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
        let mut live = self.lock();
        live.deafened = deafened;
        live.mixer.set_master_muted(deafened).map_err(mixer_error)
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

/// What the system has now. It takes a moment for each device.
async fn listing() -> Result<Listing, EngineError> {
    on_own_thread(devices::list).await
}

/// Asks `cpal` about devices on a thread of its own. Not a shared blocking one: `cpal` leaves
/// COM in single-threaded mode on the thread it ran on, and whatever came there next and needed
/// another mode was refused (ADR-0070).
async fn on_own_thread<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, EngineError> {
    let (done, answer) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("oc-devices".into())
        .spawn(move || {
            let _ = done.send(work());
        })
        .map_err(|e| e.to_string())?;
    answer.await.map_err(|e| e.to_string())
}

/// The engine as seen by a room. Weak: the room's event task must not keep the
/// engine (and with it the room) alive.
/// Connection reports on their way to the UI (ADR-0030). A room starts measuring while it
/// connects — before the engine has made it the current session — and then reports only
/// changes, so a report that came early is kept and sent the moment the room is installed.
/// Reports of a replaced room are dropped.
#[derive(Debug, Default)]
struct LinkRelay {
    /// The latest report of the newest room.
    latest: Option<(u64, Link)>,
}

impl LinkRelay {
    /// A report from `session`; `newest` is the last session started, `current` the
    /// installed one. Returns what to send now.
    fn report(
        &mut self,
        session: u64,
        link: Link,
        newest: u64,
        current: Option<u64>,
    ) -> Option<Link> {
        if session != newest {
            return None;
        }
        self.latest = Some((session, link));
        (current == Some(session)).then_some(link)
    }

    /// `session` became current: its report so far, if it came early.
    fn installed(&self, session: u64) -> Option<Link> {
        self.latest
            .filter(|(id, _)| *id == session)
            .map(|(_, link)| link)
    }
}

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
        let mut live = lock(&live);
        let (newest, current) = (live.next_session_id, live.session.as_ref().map(|s| s.id));
        if let (Some(link), Some(listener)) = (
            live.link.report(self.session_id, link, newest, current),
            &live.link_listener,
        ) {
            listener(link);
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
            if let Some(session) = session {
                session.close().await;
            }
            {
                let live = &mut lock(&live);
                if let Err(e) = live.clear_voices() {
                    eprintln!("[oc-voice] cleanup after disconnect: {e}");
                }
                live.announce(Vec::new());
                live.announce_stream(false, None);
                if let Some(listener) = &live.disconnect_listener {
                    listener(message);
                }
            }
            Engine {
                live,
                transitions: Arc::clone(&transitions),
            }
            .check_again();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::LinkRelay;
    use crate::voice::room::{Link, LinkQuality};

    /// `cpal` leaves COM in its own mode on the thread that asked it about devices. On the
    /// shared blocking threads that would refuse the stream's sound, which needs another.
    #[cfg(windows)]
    #[test]
    fn looking_at_devices_leaves_the_blocking_threads_as_they_were() {
        use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

        tauri::async_runtime::block_on(async {
            for _ in 0..8 {
                super::listing().await.unwrap();
                let usable = tauri::async_runtime::spawn_blocking(|| {
                    // SAFETY: paired with `CoUninitialize` on this thread when it succeeds.
                    let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                    if result.is_ok() {
                        unsafe { CoUninitialize() };
                    }
                    result.is_ok()
                })
                .await
                .unwrap();
                assert!(usable, "a blocking thread keeps the mode `cpal` chose");
            }
        });
    }

    const LINK: Link = Link {
        quality: LinkQuality::Excellent,
        rtt_ms: Some(1),
    };

    #[test]
    fn a_report_before_the_room_is_installed_is_sent_when_it_is() {
        let mut relay = LinkRelay::default();
        // Connecting session 1: measured already, not ours to show yet.
        assert_eq!(relay.report(1, LINK, 1, None), None);
        assert_eq!(
            relay.installed(1),
            Some(LINK),
            "the room never repeats an unchanged report"
        );
        assert_eq!(relay.report(1, LINK, 1, Some(1)), Some(LINK));
    }

    #[test]
    fn a_replaced_room_reports_nothing() {
        let mut relay = LinkRelay::default();
        relay.report(1, LINK, 1, Some(1));
        // Session 2 is connecting; session 1 says one last thing.
        assert_eq!(relay.report(1, LINK, 2, Some(1)), None);
        assert_eq!(relay.installed(2), None, "nothing measured for 2 yet");
    }
}
