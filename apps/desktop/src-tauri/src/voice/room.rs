//! One LiveKit room = one voice channel ("костёр"), ADR-0004.
//!
//! Publishes the local microphone with the ADR-0008 settings, turns every remote
//! microphone track into a mixer source and reports who is in the room. The room knows
//! nothing about the mixer or the UI: it talks to the engine through [`RoomHost`].
//!
//! Subscriptions are explicit (ADR-0049): every microphone at once, a stream only for
//! whoever asks for it, so that a stream costs nothing to those not watching.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::*;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::RtcAudioSource;
use livekit::webrtc::audio_stream::native::{NativeAudioStream, NativeAudioStreamOptions};
use livekit::webrtc::stats::RtcStats;
use oc_audio::cue::Cue;
use oc_audio::mixer::SourceInput;
use oc_audio::pcm::i16_to_f32;
use tauri::async_runtime::JoinHandle;

use super::cues::RoomCues;
use crate::stream::FULL_TRACK;

/// Opus target for voice: 64 kbit/s VBR, mono (ADR-0008).
const VOICE_BITRATE: u64 = 64_000;
/// Decoded frames (10 ms each) buffered between libwebrtc and our copy task. `NetEq` is
/// the real jitter buffer; this only absorbs task scheduling hiccups.
pub(crate) const REMOTE_QUEUE_FRAMES: usize = 20;
/// How often the panel's connection quality and ping are refreshed (ADR-0030).
const LINK_INTERVAL: Duration = Duration::from_secs(2);

/// A remote member as the UI needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMember {
    /// Account id (the LiveKit identity).
    pub user_id: String,
    /// Display name from the token.
    pub name: String,
    /// No microphone published, or it is muted.
    pub mic_muted: bool,
}

/// How well the connection carries voice, as LiveKit rates it (packet loss, jitter,
/// round trip) — the same scale its own clients show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkQuality {
    Excellent,
    Good,
    Poor,
    Lost,
}

impl From<ConnectionQuality> for LinkQuality {
    fn from(q: ConnectionQuality) -> Self {
        match q {
            ConnectionQuality::Excellent => Self::Excellent,
            ConnectionQuality::Good => Self::Good,
            ConnectionQuality::Poor => Self::Poor,
            ConnectionQuality::Lost => Self::Lost,
        }
    }
}

/// Our connection to the room, for the panel (ADR-0030).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub quality: LinkQuality,
    /// Round trip to the voice server, ms; `None` until the first measurement.
    pub rtt_ms: Option<u32>,
}

/// What the room needs from the engine.
pub trait RoomHost: Send + Sync + 'static {
    /// A member's microphone started arriving: register a mixer source for it.
    /// `None` if the mixer refused (the error is the host's to report).
    fn voice_started(&self, user_id: &str) -> Option<SourceInput>;
    /// A member's microphone is gone (unpublished, left, or we left).
    fn voice_stopped(&self, user_id: &str);
    /// The set of remote members or their mic state changed. Always the full list.
    fn members_changed(&self, members: Vec<RemoteMember>);
    /// The server ended our session (not our own `close`): the room is gone, the host
    /// should tidy up and tell the user `message`.
    fn disconnected(&self, message: String);
    /// Quality or ping changed (checked every [`LINK_INTERVAL`]).
    fn link_changed(&self, link: Link);
    /// Something happened in the room that has a sound (ADR-0075).
    fn cue(&self, _cue: Cue) {}
}

/// Where and how to join one voice room, as the camp hands it out.
#[derive(Debug, Clone)]
pub struct RoomAccess {
    /// `wss://<camp>` (ADR-0019).
    pub url: String,
    /// LiveKit token for one channel.
    pub token: String,
}

/// Connection parameters.
pub struct RoomParams {
    /// `ws://` or `wss://` address of the LiveKit server.
    pub url: String,
    /// Access token for this user and room.
    pub token: String,
    /// Rate decoded voices are delivered at (the mixer's).
    pub playback_rate: u32,
}

/// The microphone track as it is now, and whether it should be muted: a new track takes that
/// over, whenever the two happen together (ADR-0066).
struct MicState {
    track: LocalAudioTrack,
    muted: bool,
}

fn lock_mic(mic: &Mutex<MicState>) -> MutexGuard<'_, MicState> {
    // Two plain fields, each set in one step: a panic cannot leave them half-done.
    mic.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the room is told to do while it runs, in turn with its events.
enum Control {
    /// There is a new mixer: play every voice into it, at this rate.
    Replay { rate: u32 },
    /// There is a new microphone: put it in the room instead of the old one.
    Microphone {
        source: NativeAudioSource,
        done: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
}

/// A connected room with the microphone published.
pub struct VoiceRoom {
    room: Arc<Room>,
    mic: Arc<Mutex<MicState>>,
    control: tokio::sync::mpsc::UnboundedSender<Control>,
    events: JoinHandle<()>,
    link: AbortOnDrop,
    /// The runtime the room was connected on. Some synchronous SDK calls (`mute`)
    /// `tokio::spawn` internally and panic outside a runtime — e.g. on the UI thread,
    /// where Tauri runs synchronous commands. They enter this handle first.
    runtime: tokio::runtime::Handle,
}

impl VoiceRoom {
    /// Connects, publishes `mic` and starts handling room events.
    ///
    /// # Errors
    /// Connection or publish failure, as text for the UI.
    pub async fn connect(
        params: RoomParams,
        mic: NativeAudioSource,
        mic_muted: bool,
        host: Arc<dyn RoomHost>,
    ) -> Result<Self, String> {
        // `#[non_exhaustive]` forbids a struct expression outside the SDK: only a change.
        let mut options = RoomOptions::default();
        options.auto_subscribe = false;
        let (room, events) = Room::connect(&params.url, &params.token, options)
            .await
            .map_err(|e| format!("Не удалось подключиться к {}: {e}", params.url))?;
        let room = Arc::new(room);
        // Microphones already in the room were announced before we subscribed to events;
        // the later ones arrive as `TrackPublished` (`handle_events`).
        for participant in room.remote_participants().into_values() {
            for publication in participant.track_publications().into_values() {
                listen_if_microphone(&publication);
            }
        }

        let track = LocalAudioTrack::create_audio_track("microphone", RtcAudioSource::Native(mic));
        if mic_muted {
            track.mute();
        }
        // A silenced member's token comes without publishing (ADR-0028): they listen
        // only, and the microphone goes up when the right comes back (`handle_events`).
        if may_publish(&room) {
            if let Err(e) = publish_mic(&room, &track).await {
                let _ = room.close().await;
                return Err(format!("Не удалось включить микрофон в комнате: {e}"));
            }
        }

        // Participants already in the room were announced before we subscribed to
        // events; report them once up front.
        host.members_changed(members(&room));
        let link = AbortOnDrop(tauri::async_runtime::spawn(watch_link(
            Arc::clone(&room),
            Arc::clone(&host),
        )));
        let mic = Arc::new(Mutex::new(MicState {
            track,
            muted: mic_muted,
        }));
        let (control, controls) = tokio::sync::mpsc::unbounded_channel();
        let events = tauri::async_runtime::spawn(handle_events(
            Arc::clone(&room),
            events,
            controls,
            host,
            params.playback_rate,
            Arc::clone(&mic),
        ));
        Ok(Self {
            room,
            mic,
            control,
            events,
            link,
            runtime: tokio::runtime::Handle::current(),
        })
    }

    /// The room, for what else goes into it (a stream, ADR-0049).
    pub fn room(&self) -> Arc<Room> {
        Arc::clone(&self.room)
    }

    /// Mutes the published microphone; others see it as muted. Callable from any
    /// thread.
    pub fn set_mic_muted(&self, muted: bool) {
        let _runtime = self.runtime.enter();
        let mut mic = lock_mic(&self.mic);
        mic.muted = muted;
        if muted {
            mic.track.mute();
        } else {
            mic.track.unmute();
        }
    }

    /// The mixer was replaced: every voice starts over in the new one, at its rate. The
    /// engine has already forgotten the old sources, so no `voice_stopped` comes.
    pub fn replay(&self, rate: u32) {
        let _ = self.control.send(Control::Replay { rate });
    }

    /// Puts another microphone in the room. Others lose this person for as long as that takes.
    ///
    /// # Errors
    /// If it could not be published, as text for the UI.
    pub fn replace_mic(
        &self,
        source: NativeAudioSource,
    ) -> impl Future<Output = Result<(), String>> {
        let (done, answer) = tokio::sync::oneshot::channel();
        let sent = self
            .control
            .send(Control::Microphone { source, done })
            .is_ok();
        async move {
            let closed = || "Комната уже закрыта".to_owned();
            if !sent {
                return Err(closed());
            }
            answer.await.map_err(|_| closed())?
        }
    }

    /// Leaves the room and stops every remote voice task. The engine removes the
    /// mixer sources itself, so no [`RoomHost`] callbacks are made.
    pub async fn close(self) {
        drop(self.link);
        if let Err(e) = self.room.close().await {
            eprintln!("[oc-voice] leaving room: {e}");
        }
        // Aborting the handler drops its voice tasks (see `AbortOnDrop`), so nothing
        // keeps writing into sources the engine is about to remove.
        self.events.abort();
        let _ = self.events.await;
    }
}

/// Reports our connection while the room lives: LiveKit's quality rating and the round
/// trip of the media path. Only changes go to the host.
async fn watch_link(room: Arc<Room>, host: Arc<dyn RoomHost>) {
    let mut last = None;
    let mut tick = tokio::time::interval(LINK_INTERVAL);
    loop {
        tick.tick().await;
        let rtt_ms = room
            .get_stats()
            .await
            .ok()
            .and_then(|s| rtt_ms(&s.subscriber_stats, &s.publisher_stats));
        let link = Link {
            quality: room.local_participant().connection_quality().into(),
            rtt_ms,
        };
        if last != Some(link) {
            last = Some(link);
            host.link_changed(link);
        }
    }
}

/// Ping: the current round trip of the ICE candidate pair in use (STUN consent checks
/// keep it fresh even in silence). Receiving is always on, so its transport goes first.
/// libwebrtc counts it in whole milliseconds, so a server on this machine reads 0: what
/// tells "measured" apart is that the pair has had answers.
fn rtt_ms(subscriber: &[RtcStats], publisher: &[RtcStats]) -> Option<u32> {
    subscriber.iter().chain(publisher).find_map(|s| match s {
        RtcStats::CandidatePair(p)
            if p.candidate_pair.nominated && p.candidate_pair.responses_received > 0 =>
        {
            // Seconds → ms; a round trip of 49 days does not fit, nor happen.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Some((p.candidate_pair.current_round_trip_time * 1000.0).round() as u32)
        }
        _ => None,
    })
}

/// Whether this participant may publish now: the room token's grant, changed by the
/// camp when it silences or lifts (ADR-0028). No permission info means the default: yes.
fn may_publish(room: &Room) -> bool {
    room.local_participant()
        .permission()
        .is_none_or(|p| p.can_publish)
}

/// Publishes the microphone with the voice settings of ADR-0008.
async fn publish_mic(room: &Room, track: &LocalAudioTrack) -> Result<(), String> {
    let options = TrackPublishOptions {
        source: TrackSource::Microphone,
        audio_encoding: Some(AudioEncoding {
            max_bitrate: VOICE_BITRATE,
        }),
        // DTX clips the start of phrases; RED + Opus in-band FEC (on by default in
        // libwebrtc) hide packet loss.
        dtx: false,
        red: true,
        ..TrackPublishOptions::default()
    };
    room.local_participant()
        .publish_track(LocalTrack::Audio(track.clone()), options)
        .await
        .map(drop)
        .map_err(|e| e.to_string())
}

/// Asks for a remote track if it is a voice. Needs a runtime: the SDK spawns the request.
fn listen_if_microphone(publication: &RemoteTrackPublication) {
    if publication.source() == TrackSource::Microphone {
        publication.set_subscribed(true);
    }
}

/// Aborts a task when dropped. Tokio's `JoinHandle` only detaches on drop, which
/// would leave a voice task running after its room is gone.
pub(crate) struct AbortOnDrop(pub(crate) JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn members(room: &Room) -> Vec<RemoteMember> {
    let mut list: Vec<RemoteMember> = room
        .remote_participants()
        .into_values()
        .map(|p| {
            let mic_live = p.track_publications().values().any(|publication| {
                publication.source() == TrackSource::Microphone && !publication.is_muted()
            });
            RemoteMember {
                user_id: p.identity().to_string(),
                name: p.name(),
                mic_muted: !mic_live,
            }
        })
        .collect();
    // Stable order keeps the UI from reshuffling on every update.
    list.sort_by(|a, b| a.user_id.cmp(&b.user_id));
    list
}

/// Takes the old microphone out of the room and puts `source` in. In this order: one person
/// has one microphone for the others, at every moment.
async fn swap_mic(
    room: &Room,
    mic: &Mutex<MicState>,
    source: NativeAudioSource,
) -> Result<(), String> {
    let local = room.local_participant();
    let published: Vec<TrackSid> = local
        .track_publications()
        .values()
        .filter(|p| p.source() == TrackSource::Microphone)
        .map(LocalTrackPublication::sid)
        .collect();
    for sid in published {
        local
            .unpublish_track(&sid)
            .await
            .map_err(|e| e.to_string())?;
    }
    let track = LocalAudioTrack::create_audio_track("microphone", RtcAudioSource::Native(source));
    {
        let mut mic = lock_mic(mic);
        if mic.muted {
            track.mute();
        }
        mic.track = track.clone();
    }
    // A silenced member stays off the air: the right coming back publishes the new track.
    if may_publish(room) {
        publish_mic(room, &track).await?;
    }
    Ok(())
}

/// A member's microphone playing, and which publication that is: the end of an older one can
/// come after this one has begun, and must not stop it (ADR-0066).
struct Voice {
    publication: TrackSid,
    _task: AbortOnDrop,
}

/// Plays one member's microphone, in the mixer the host gives.
fn play_voice(
    host: &Arc<dyn RoomHost>,
    voices: &mut HashMap<String, Voice>,
    user_id: String,
    publication: &RemoteTrackPublication,
    track: &RemoteAudioTrack,
    rate: u32,
) {
    if let Some(input) = host.voice_started(&user_id) {
        let stream = NativeAudioStream::with_options(
            track.rtc_track(),
            i32::try_from(rate).expect("sample rate fits in i32"),
            1,
            NativeAudioStreamOptions {
                queue_size_frames: Some(REMOTE_QUEUE_FRAMES),
            },
        );
        let task = tauri::async_runtime::spawn(play(stream, input));
        voices.insert(
            user_id,
            Voice {
                publication: publication.sid(),
                _task: AbortOnDrop(task),
            },
        );
    }
}

/// Puts the microphone up or takes it down, as the right to publish now says.
async fn follow_permission(
    room: &Room,
    local: &LocalParticipant,
    mic: &Mutex<MicState>,
    may_publish: bool,
) {
    let published: Vec<TrackSid> = local
        .track_publications()
        .values()
        .filter(|p| p.source() == TrackSource::Microphone)
        .map(LocalTrackPublication::sid)
        .collect();
    if may_publish {
        if published.is_empty() {
            let track = lock_mic(mic).track.clone();
            if let Err(e) = publish_mic(room, &track).await {
                eprintln!("[oc-voice] microphone back after a lift: {e}");
            }
        }
    } else {
        for sid in published {
            // The track itself keeps running: only the sender goes.
            if let Err(e) = local.unpublish_track(&sid).await {
                eprintln!("[oc-voice] microphone down on silence: {e}");
            }
        }
    }
}

/// Starts every voice over, in the mixer the host has now.
fn replay_voices(
    room: &Room,
    host: &Arc<dyn RoomHost>,
    voices: &mut HashMap<String, Voice>,
    rate: u32,
) {
    voices.clear();
    for participant in room.remote_participants().into_values() {
        for publication in participant.track_publications().into_values() {
            if publication.source() != TrackSource::Microphone {
                continue;
            }
            if let Some(RemoteTrack::Audio(track)) = publication.track() {
                play_voice(
                    host,
                    voices,
                    participant.identity().to_string(),
                    &publication,
                    &track,
                    rate,
                );
            }
        }
    }
}

/// Who is in the room besides us.
fn identities(room: &Room) -> Vec<String> {
    room.remote_participants()
        .into_keys()
        .map(|identity| identity.to_string())
        .collect()
}

/// Passes what deserves a sound to the host (ADR-0075).
fn follow_cues(room: &Room, event: &RoomEvent, cues: &mut RoomCues, host: &dyn RoomHost) {
    let cue = match event {
        // The name, not the kind: a stream's light copy and its sound are the same stream.
        RoomEvent::TrackPublished { publication, .. } if publication.name() == FULL_TRACK => {
            cues.stream_started()
        }
        RoomEvent::ParticipantConnected(participant) => {
            cues.joined(participant.identity().to_string())
        }
        RoomEvent::ParticipantDisconnected(participant) => {
            cues.left(participant.identity().to_string(), Instant::now());
            None
        }
        RoomEvent::Reconnecting => {
            cues.reconnecting();
            None
        }
        RoomEvent::Reconnected => {
            cues.reconnected(identities(room));
            None
        }
        _ => None,
    };
    if let Some(cue) = cue {
        host.cue(cue);
    }
}

/// Waits for the moment, or forever when there is none.
async fn until(moment: Option<Instant>) {
    match moment {
        Some(moment) => tokio::time::sleep_until(moment.into()).await,
        None => std::future::pending().await,
    }
}

async fn handle_events(
    room: Arc<Room>,
    mut events: tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
    mut controls: tokio::sync::mpsc::UnboundedReceiver<Control>,
    host: Arc<dyn RoomHost>,
    mut playback_rate: u32,
    mic: Arc<Mutex<MicState>>,
) {
    let mut voices: HashMap<String, Voice> = HashMap::new();
    let mut last_members = members(&room);
    let mut cues = RoomCues::new(identities(&room));
    let stop_voice = |voices: &mut HashMap<String, Voice>, user_id: &str| {
        if voices.remove(user_id).is_some() {
            host.voice_stopped(user_id);
        }
    };

    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            () = until(cues.due()) => {
                if let Some(cue) = cues.departures() {
                    host.cue(cue);
                }
                continue;
            }
            Some(control) = controls.recv() => {
                match control {
                    Control::Replay { rate } => {
                        playback_rate = rate;
                        replay_voices(&room, &host, &mut voices, rate);
                    }
                    Control::Microphone { source, done } => {
                        let _ = done.send(swap_mic(&room, &mic, source).await);
                    }
                }
                continue;
            }
        };
        let Some(event) = event else { break };
        follow_cues(&room, &event, &mut cues, host.as_ref());
        match event {
            RoomEvent::TrackPublished { publication, .. } => listen_if_microphone(&publication),
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(track),
                publication,
                participant,
            } if publication.source() == TrackSource::Microphone => {
                let user_id = participant.identity().to_string();
                stop_voice(&mut voices, &user_id);
                play_voice(
                    &host,
                    &mut voices,
                    user_id,
                    &publication,
                    &track,
                    playback_rate,
                );
            }
            RoomEvent::TrackUnsubscribed {
                publication,
                participant,
                ..
            } if publication.source() == TrackSource::Microphone => {
                let user_id = participant.identity().to_string();
                if voices
                    .get(&user_id)
                    .is_some_and(|v| v.publication == publication.sid())
                {
                    stop_voice(&mut voices, &user_id);
                }
            }
            RoomEvent::ParticipantDisconnected(participant) => {
                stop_voice(&mut voices, &participant.identity().to_string());
            }
            // Silenced or lifted (ADR-0028). Taking the right away, LiveKit drops the
            // microphone on its side but the SDK keeps the publication: we take it down
            // here too, so that giving the right back can publish it anew.
            RoomEvent::ParticipantPermissionChanged {
                participant: Participant::Local(local),
                permission,
            } => {
                let may = permission.is_none_or(|p| p.can_publish);
                follow_permission(&room, &local, &mic, may).await;
            }
            RoomEvent::Disconnected { reason } => {
                eprintln!("[oc-voice] disconnected: {reason:?}");
                if reason != livekit::DisconnectReason::ClientInitiated {
                    host.disconnected(disconnect_message(reason).to_owned());
                }
                break;
            }
            _ => {}
        }
        // Cheap (a handful of participants) and covers every event that can change the
        // list: joins, leaves, publishes, mutes.
        let now = members(&room);
        if now != last_members {
            host.members_changed(now.clone());
            last_members = now;
        }
    }

    // Server-side disconnect (not our `close`): release every voice.
    let user_ids: Vec<String> = voices.keys().cloned().collect();
    for user_id in user_ids {
        stop_voice(&mut voices, &user_id);
    }
    host.members_changed(Vec::new());
}

/// Why the server ended the session, for the user.
fn disconnect_message(reason: livekit::DisconnectReason) -> &'static str {
    match reason {
        livekit::DisconnectReason::DuplicateIdentity => {
            "Вы зашли на этот сервер из другого окна или с другого устройства — здесь голос отключён"
        }
        livekit::DisconnectReason::ParticipantRemoved => "Вас убрали из канала",
        // The camp removes a campfire by closing its room (ADR-0026).
        livekit::DisconnectReason::RoomDeleted => "Канал удалён",
        livekit::DisconnectReason::RoomClosed => "Канал закрыт на сервере",
        livekit::DisconnectReason::ServerShutdown => "Сервер голоса остановлен",
        _ => "Связь с каналом потеряна — зайдите в него снова",
    }
}

/// Copies one remote voice, or a stream's sound, into its mixer source until the track ends.
pub(crate) async fn play(mut stream: NativeAudioStream, mut input: SourceInput) {
    let mut buf: Vec<f32> = Vec::new();
    while let Some(frame) = stream.next().await {
        // Frames are 10 ms; the buffer grows once and is reused.
        buf.resize(frame.data.len(), 0.0);
        i16_to_f32(&frame.data, &mut buf);
        input.write(&buf);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use livekit::webrtc::audio_source::AudioSourceOptions;
    use oc_camp_client::transport::{install, PlainLoopback};
    use oc_camp_client::{CampClient, DeviceKey};

    struct NoHost;

    impl RoomHost for NoHost {
        fn voice_started(&self, _: &str) -> Option<SourceInput> {
            None
        }
        fn voice_stopped(&self, _: &str) {}
        fn members_changed(&self, _: Vec<RemoteMember>) {}
        fn disconnected(&self, _: String) {}
        fn link_changed(&self, _: Link) {}
    }

    fn pair(nominated: bool, rtt_s: f64) -> RtcStats {
        answered(nominated, rtt_s, 3)
    }

    fn answered(nominated: bool, rtt_s: f64, responses_received: u64) -> RtcStats {
        RtcStats::CandidatePair(livekit::webrtc::stats::CandidatePairStats {
            candidate_pair: livekit::webrtc::stats::dictionaries::CandidatePairStats {
                nominated,
                current_round_trip_time: rtt_s,
                responses_received,
                ..Default::default()
            },
            ..Default::default()
        })
    }

    /// What the SDK does on every disconnect by the server: removes a sender from a closed
    /// connection and gets an error from C++. Its text used to be read after being freed, and
    /// a debug build then aborted the process now and then (ADR-0050). One call in a few hits
    /// it, so many calls; needs no server.
    #[test]
    fn an_error_from_webrtc_is_read_while_alive() {
        use livekit::webrtc::audio_source::native::NativeAudioSource;
        use livekit::webrtc::audio_source::AudioSourceOptions;
        use livekit::webrtc::media_stream_track::MediaStreamTrack;
        use livekit::webrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
        use livekit::webrtc::peer_connection_factory::{PeerConnectionFactory, RtcConfiguration};

        let factory = PeerConnectionFactory::default();
        let connection = factory
            .create_peer_connection(RtcConfiguration::default())
            .unwrap();
        let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
        let track = factory.create_audio_track("mic", source);
        let sender = connection
            .add_track(MediaStreamTrack::Audio(track), &["stream"])
            .unwrap();
        connection.close();
        for _ in 0..1000 {
            let error = connection.remove_track(sender.clone()).unwrap_err();
            assert!(error.message.contains("closed"), "{}", error.message);
        }
    }

    #[test]
    fn ping_is_the_round_trip_of_the_pair_in_use() {
        assert_eq!(
            rtt_ms(&[pair(false, 0.3), pair(true, 0.0234)], &[]),
            Some(23)
        );
        assert_eq!(
            rtt_ms(&[], &[pair(true, 0.0415)]),
            Some(42),
            "publisher only"
        );
        assert_eq!(
            rtt_ms(&[pair(true, 0.012)], &[pair(true, 0.09)]),
            Some(12),
            "receiving goes first"
        );
        assert_eq!(
            rtt_ms(&[answered(true, 0.0, 0)], &[]),
            None,
            "no answers yet: not measured"
        );
        assert_eq!(
            rtt_ms(&[answered(true, 0.0, 7)], &[]),
            Some(0),
            "a server on this machine: under a millisecond, libwebrtc says 0"
        );
        assert_eq!(rtt_ms(&[], &[]), None);
    }

    /// A camp server in this process (LiveKit from `pnpm livekit`), a fresh device logged
    /// in, and the room parameters for channel `c0` — the real path, pinned TLS included.
    pub(crate) async fn camp_room(
        name: &str,
    ) -> (oc_server::Running, std::path::PathBuf, RoomParams) {
        install(PlainLoopback::Deny);
        let data_dir = std::env::temp_dir().join(format!("oc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data_dir);
        let camp = oc_server::start(oc_server::Config {
            data_dir: data_dir.clone(),
            bind: (std::net::Ipv4Addr::LOCALHOST, 0).into(),
            public_host: oc_invite::Host::Ipv4(std::net::Ipv4Addr::LOCALHOST),
            public_port: None,
            camp_name: name.into(),
            router: None,
            certificate: None,
            livekit: oc_server::LiveKit {
                url: "ws://127.0.0.1:7880".into(),
                api_key: "devkey".into(),
                api_secret: "secret".into(),
            }
            .into(),
        })
        .await
        .unwrap();
        let client = CampClient::new(&camp.links.join);
        let session = client
            .login(
                &DeviceKey::generate(),
                name,
                Some(camp.links.join.token.as_str()),
            )
            .await
            .unwrap();
        let token = client.room_token(&session, "c0").await.unwrap().token;
        let params = RoomParams {
            url: client.voice_url(),
            token,
            playback_rate: 48_000,
        };
        (camp, data_dir, params)
    }

    async fn connect(params: RoomParams) -> VoiceRoom {
        let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
        VoiceRoom::connect(params, source, false, Arc::new(NoHost))
            .await
            .expect("voice through the camp (is `pnpm livekit` running?)")
    }

    /// Regression: Tauri runs synchronous commands on the UI thread, which has no tokio
    /// runtime; `LocalAudioTrack::mute` spawned onto "no reactor" and aborted the app.
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn mic_mute_works_from_a_thread_without_a_runtime() {
        let (camp, dir, params) = tauri::async_runtime::block_on(camp_room("mute"));
        let room = tauri::async_runtime::block_on(connect(params));
        let room = std::thread::spawn(move || {
            room.set_mic_muted(true);
            room.set_mic_muted(false);
            room
        })
        .join()
        .expect("muting outside a runtime must not panic");
        tauri::async_runtime::block_on(async {
            room.close().await;
            camp.stop().await;
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    /// ADR-0021 against the real LiveKit: someone sits down at the Lobby through the camp,
    /// and a member who has not joined voice sees it on the camp's event stream (admin
    /// token scope, Twirp JSON and filtering all real).
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn presence_is_seen_by_members_outside_voice() {
        tauri::async_runtime::block_on(async {
            let (camp, dir, params) = camp_room("presence").await;
            let room = connect(params).await;

            let watcher = CampClient::new(&camp.links.join);
            let session = watcher
                .login(
                    &DeviceKey::generate(),
                    "Ангелина",
                    Some(camp.links.join.token.as_str()),
                )
                .await
                .unwrap();
            let mut events = watcher.events(&session).await.unwrap();
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            let state = loop {
                let event = tokio::time::timeout_at(deadline, events.next())
                    .await
                    .expect("presence within 10 s")
                    .expect("stream open")
                    .expect("valid event");
                let oc_proto::CampEvent::State(state) = event else {
                    continue;
                };
                // The seat can show up before the microphone does: the camp polls a moment
                // of LiveKit's state, and on a busy machine the track comes a little later.
                let unmuted =
                    |p: &oc_proto::ChannelPresence| p.members.iter().any(|m| !m.mic_muted);
                if state.presence.iter().any(unmuted) {
                    break state;
                }
            };
            assert_eq!(state.presence[0].channel_id, "c0");
            let sitter = &state.presence[0].members[0].user_id;
            let profile = state.members.iter().find(|m| &m.user_id == sitter).unwrap();
            assert_eq!(profile.name, "presence", "the person who joined voice");

            // A polite leave (what the app does on exit too) frees the seat at once;
            // without it LiveKit keeps the seat until it times the connection out.
            room.close().await;
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                let event = tokio::time::timeout_at(deadline, events.next())
                    .await
                    .expect("the seat is freed within 3 s")
                    .expect("stream open")
                    .expect("valid event");
                let oc_proto::CampEvent::State(state) = event else {
                    continue;
                };
                if state.presence.is_empty() {
                    break;
                }
            }
            camp.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// ADR-0028 against the real LiveKit: silencing takes the microphone down at once
    /// (`UpdateParticipant`), lifting brings it back without rejoining — the room
    /// publishes it again on `ParticipantPermissionChanged`.
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn silencing_takes_the_mic_down_and_lifting_brings_it_back() {
        tauri::async_runtime::block_on(async {
            let (camp, dir, params) = camp_room("silence").await;
            let room = connect(params).await;
            let mut states = camp.camp_state();
            // Whether the one person in `c0` publishes an unmuted microphone, once seen.
            let speaking = |want: bool| {
                move |s: &Arc<oc_proto::CampState>| {
                    s.presence
                        .first()
                        .and_then(|p| p.members.first())
                        .is_some_and(|m| m.mic_muted != want)
                }
            };
            let deadline = std::time::Duration::from_secs(10);
            let me = tokio::time::timeout(deadline, states.wait_for(speaking(true)))
                .await
                .expect("speaking within 10 s")
                .unwrap()
                .presence[0]
                .members[0]
                .user_id
                .clone();

            let manager = camp.manager();
            manager.set_silenced(&me, true).await.unwrap();
            tokio::time::timeout(deadline, states.wait_for(speaking(false)))
                .await
                .expect("the microphone is taken down")
                .unwrap();

            manager.set_silenced(&me, false).await.unwrap();
            tokio::time::timeout(deadline, states.wait_for(speaking(true)))
                .await
                .expect("the microphone is back")
                .unwrap();

            room.close().await;
            camp.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// Records whose voices arrive.
    #[derive(Default)]
    struct Ears(std::sync::Mutex<Vec<String>>);

    impl Ears {
        async fn wait_for(&self, user_id: &str) {
            tokio::time::timeout(Duration::from_secs(10), async {
                while !self.0.lock().unwrap().iter().any(|id| id == user_id) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the microphone arrives by itself");
        }
    }

    impl RoomHost for Ears {
        fn voice_started(&self, user_id: &str) -> Option<SourceInput> {
            self.0.lock().unwrap().push(user_id.to_owned());
            None
        }
        fn voice_stopped(&self, _: &str) {}
        fn members_changed(&self, _: Vec<RemoteMember>) {}
        fn disconnected(&self, _: String) {}
        fn link_changed(&self, _: Link) {}
    }

    /// One more member of this camp, logged in.
    async fn another_member(
        camp: &oc_server::Running,
        name: &str,
    ) -> (CampClient, oc_camp_client::Session) {
        let client = CampClient::new(&camp.links.join);
        let session = client
            .login(
                &DeviceKey::generate(),
                name,
                Some(camp.links.join.token.as_str()),
            )
            .await
            .unwrap();
        (client, session)
    }

    /// Their way into `c0`, with the rights they have at this moment.
    async fn way_in(client: &CampClient, session: &oc_camp_client::Session) -> RoomParams {
        RoomParams {
            url: client.voice_url(),
            token: client.room_token(session, "c0").await.unwrap().token,
            playback_rate: 48_000,
        }
    }

    async fn listener(params: RoomParams, ears: &Arc<Ears>) -> VoiceRoom {
        let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
        let host: Arc<dyn RoomHost> = ears.clone();
        VoiceRoom::connect(params, source, false, host)
            .await
            .expect("voice through the camp (is `pnpm livekit` running?)")
    }

    impl Ears {
        fn heard(&self, user_id: &str) -> usize {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter(|id| *id == user_id)
                .count()
        }

        async fn wait_for_times(&self, user_id: &str, times: usize) {
            tokio::time::timeout(Duration::from_secs(10), async {
                while self.heard(user_id) < times {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("{times} voices, heard {}", self.heard(user_id)));
        }
    }

    /// ADR-0066 against the real LiveKit: a new mixer plays every voice again without anyone
    /// doing anything in the room.
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn a_new_mixer_plays_every_voice_again() {
        tauri::async_runtime::block_on(async {
            let (camp, dir, params) = camp_room("replay").await;
            let speaker = connect(params).await;
            let speaker_id = speaker.room().local_participant().identity().to_string();
            let ears = Arc::new(Ears::default());
            let (client, session) = another_member(&camp, "Слушатель").await;
            let hearing = listener(way_in(&client, &session).await, &ears).await;
            ears.wait_for_times(&speaker_id, 1).await;

            hearing.replay(44_100);
            ears.wait_for_times(&speaker_id, 2).await;

            hearing.close().await;
            speaker.close().await;
            camp.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// Plays voices into a mixer as the engine does, and counts how many are on.
    struct Player {
        mixer: std::sync::Mutex<oc_audio::mixer::MixerControl<u32>>,
        _core: std::sync::Mutex<oc_audio::mixer::MixerCore>,
        started: std::sync::atomic::AtomicU32,
        stopped: std::sync::atomic::AtomicU32,
    }

    impl Player {
        fn new() -> Self {
            let (mixer, core) =
                oc_audio::mixer::mixer(oc_audio::mixer::MixerConfig::default_48k_stereo());
            Self {
                mixer: mixer.into(),
                _core: core.into(),
                started: 0.into(),
                stopped: 0.into(),
            }
        }

        fn started(&self) -> u32 {
            self.started.load(std::sync::atomic::Ordering::Relaxed)
        }

        fn playing(&self) -> u32 {
            self.started() - self.stopped.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    impl RoomHost for Player {
        fn voice_started(&self, _: &str) -> Option<SourceInput> {
            let n = self
                .started
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.mixer
                .lock()
                .unwrap()
                .add_source(n, oc_audio::mixer::Layout::Mono, 1.0)
                .ok()
        }
        fn voice_stopped(&self, _: &str) {
            self.stopped
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        fn members_changed(&self, _: Vec<RemoteMember>) {}
        fn disconnected(&self, _: String) {}
        fn link_changed(&self, _: Link) {}
    }

    /// ADR-0066 against the real LiveKit: another microphone, at another rate, reaches the
    /// others by itself and goes on playing when the old one's end comes, later than its own
    /// start. Muting holds across the change.
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn another_microphone_reaches_the_others_and_stays_muted() {
        tauri::async_runtime::block_on(async {
            let (camp, dir, params) = camp_room("swap").await;
            let speaker = connect(params).await;
            let local = speaker.room().local_participant();
            let player = Arc::new(Player::new());
            let (client, session) = another_member(&camp, "Слушатель").await;
            let silent = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
            let host: Arc<dyn RoomHost> = player.clone();
            let hearing = VoiceRoom::connect(way_in(&client, &session).await, silent, false, host)
                .await
                .unwrap();
            let voices = |times: u32| {
                let player = Arc::clone(&player);
                async move {
                    for _ in 0..500 {
                        if player.started() >= times {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    panic!("{times} voices, heard {}", player.started());
                }
            };
            voices(1).await;

            speaker.set_mic_muted(true);
            let source = NativeAudioSource::new(AudioSourceOptions::default(), 16_000, 1, 0);
            speaker.replace_mic(source).await.unwrap();
            voices(2).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
            assert_eq!(player.playing(), 1, "the new voice goes on");

            let mics: Vec<_> = local
                .track_publications()
                .into_values()
                .filter(|p| p.source() == TrackSource::Microphone)
                .collect();
            assert_eq!(mics.len(), 1, "the old one is gone");
            assert!(mics[0].is_muted(), "muted before the change, muted after");
            speaker.set_mic_muted(false);
            assert!(!mics[0].is_muted(), "and the button works on the new one");

            hearing.close().await;
            speaker.close().await;
            camp.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// The stream of `user_id` as this room sees it, once announced.
    fn screen_of(room: &VoiceRoom, user_id: &str) -> Option<RemoteTrackPublication> {
        let identity = ParticipantIdentity::from(user_id.to_owned());
        room.room()
            .remote_participants()
            .get(&identity)?
            .track_publications()
            .into_values()
            .find(|p| p.source() == crate::stream::STREAM_SOURCE)
    }

    /// A publisher of black frames, for as long as the task lives.
    fn black_screen() -> (LocalVideoTrack, AbortOnDrop) {
        use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
        use livekit::webrtc::video_source::native::NativeVideoSource;
        use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};

        let (width, height) = (320, 180);
        let source = NativeVideoSource::new(VideoResolution { width, height }, true);
        let frames = AbortOnDrop(tauri::async_runtime::spawn({
            let source = source.clone();
            async move {
                let buffer = I420Buffer::new_black(width, height);
                let mut frame = VideoFrame::new(VideoRotation::VideoRotation0, buffer);
                let mut tick = tokio::time::interval(Duration::from_millis(50));
                loop {
                    tick.tick().await;
                    frame.timestamp_us += 50_000;
                    source.capture_frame(&frame);
                }
            }
        }));
        let track = LocalVideoTrack::create_video_track("screen", RtcVideoSource::Native(source));
        (track, frames)
    }

    /// ADR-0049 against the real LiveKit: microphones come by themselves to whoever was
    /// here first and to whoever came later; a stream reaches only the one who asked; the
    /// camp's snapshot tells everyone who is on air.
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn microphones_come_by_themselves_and_a_stream_only_on_request() {
        use livekit::webrtc::video_stream::native::NativeVideoStream;

        tauri::async_runtime::block_on(async {
            let deadline = Duration::from_secs(10);
            let (camp, dir, params) = camp_room("on-demand").await;
            let early_ears = Arc::new(Ears::default());
            let early = listener(params, &early_ears).await;

            let (client, session) = another_member(&camp, "Стример").await;
            let access = way_in(&client, &session).await;
            let (streamer, _) = Room::connect(&access.url, &access.token, RoomOptions::default())
                .await
                .unwrap();
            let streamer_id = streamer.local_participant().identity().to_string();
            let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
            let mic =
                LocalAudioTrack::create_audio_track("microphone", RtcAudioSource::Native(source));
            publish_mic(&streamer, &mic).await.unwrap();
            let (screen, frames) = black_screen();
            let published = streamer
                .local_participant()
                .publish_track(
                    LocalTrack::Video(screen),
                    TrackPublishOptions {
                        source: crate::stream::STREAM_SOURCE,
                        ..TrackPublishOptions::default()
                    },
                )
                .await
                .unwrap();

            let mut states = camp.camp_state();
            let on_air = |want: bool, id: String| {
                move |s: &Arc<oc_proto::CampState>| {
                    s.presence
                        .iter()
                        .flat_map(|c| &c.members)
                        .any(|m| m.user_id == id && m.streaming == want)
                }
            };
            let state =
                tokio::time::timeout(deadline, states.wait_for(on_air(true, streamer_id.clone())))
                    .await
                    .expect("the camp sees the stream")
                    .unwrap()
                    .clone();
            let others_on_air = state
                .presence
                .iter()
                .flat_map(|c| &c.members)
                .filter(|m| m.user_id != streamer_id && m.streaming)
                .count();
            assert_eq!(others_on_air, 0, "only the one who shows their screen");

            // Comes after the stream is up: both tracks are already in the room.
            let (client, session) = another_member(&camp, "Опоздавший").await;
            let late_ears = Arc::new(Ears::default());
            let late = listener(way_in(&client, &session).await, &late_ears).await;

            early_ears.wait_for(&streamer_id).await;
            late_ears.wait_for(&streamer_id).await;
            // Long enough for a stream sent without asking to have arrived alongside.
            tokio::time::sleep(Duration::from_secs(1)).await;
            for room in [&early, &late] {
                let stream = screen_of(room, &streamer_id).expect("the stream is announced");
                assert!(!stream.is_desired(), "nobody asked for it");
                assert!(stream.track().is_none(), "nothing is delivered");
            }

            let asked = screen_of(&late, &streamer_id).unwrap();
            asked.set_subscribed(true);
            let track = tokio::time::timeout(deadline, async {
                loop {
                    if let Some(track) = asked.track() {
                        return track;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the stream comes when asked for");
            let RemoteTrack::Video(video) = track else {
                panic!("a screen is a video track");
            };
            let mut stream = NativeVideoStream::new(video.rtc_track());
            tokio::time::timeout(Duration::from_secs(15), stream.next())
                .await
                .expect("frames of the stream")
                .expect("the stream is open");
            let untouched = screen_of(&early, &streamer_id).unwrap();
            assert!(untouched.track().is_none(), "the other one did not ask");

            drop(frames);
            streamer
                .local_participant()
                .unpublish_track(&published.sid())
                .await
                .unwrap();
            tokio::time::timeout(deadline, states.wait_for(on_air(false, streamer_id)))
                .await
                .expect("the camp sees the stream end")
                .unwrap();

            early.close().await;
            late.close().await;
            streamer.close().await.unwrap();
            camp.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// Someone watching does not lose the picture while others come and go: every change
    /// of the room renegotiates the connection that carries it.
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn a_stream_keeps_going_while_people_come_and_go() {
        use livekit::webrtc::video_stream::native::NativeVideoStream;

        tauri::async_runtime::block_on(async {
            let (camp, dir, params) = camp_room("churn").await;
            let ears = Arc::new(Ears::default());
            let viewer = listener(params, &ears).await;

            let (client, session) = another_member(&camp, "Стример").await;
            let access = way_in(&client, &session).await;
            let (streamer, _) = Room::connect(&access.url, &access.token, RoomOptions::default())
                .await
                .unwrap();
            let streamer_id = streamer.local_participant().identity().to_string();
            let (screen, frames) = black_screen();
            streamer
                .local_participant()
                .publish_track(
                    LocalTrack::Video(screen),
                    TrackPublishOptions {
                        source: crate::stream::STREAM_SOURCE,
                        ..TrackPublishOptions::default()
                    },
                )
                .await
                .unwrap();

            let deadline = Duration::from_secs(10);
            let asked = tokio::time::timeout(deadline, async {
                loop {
                    if let Some(publication) = screen_of(&viewer, &streamer_id) {
                        return publication;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the stream is announced");
            asked.set_subscribed(true);
            let track = tokio::time::timeout(deadline, async {
                loop {
                    if let Some(track) = asked.track() {
                        return track;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the stream comes when asked for");
            let RemoteTrack::Video(video) = track else {
                panic!("a screen is a video track");
            };

            // The longest wait between two pictures, while the room changes.
            let mut decoded = NativeVideoStream::new(video.rtc_track());
            tokio::time::timeout(Duration::from_secs(15), decoded.next())
                .await
                .expect("the first picture")
                .expect("the stream is open");
            let watch = tauri::async_runtime::spawn(async move {
                let mut longest = Duration::ZERO;
                let mut last = tokio::time::Instant::now();
                let mut count = 0_u32;
                let end = last + Duration::from_secs(12);
                while let Ok(Some(_)) = tokio::time::timeout_at(end, decoded.next()).await {
                    count += 1;
                    longest = longest.max(last.elapsed());
                    last = tokio::time::Instant::now();
                }
                (
                    count,
                    longest.max(last.elapsed().min(Duration::from_secs(12))),
                )
            });
            for round in 0..6 {
                let (client, session) = another_member(&camp, &format!("Гость {round}")).await;
                let guest = connect(way_in(&client, &session).await).await;
                tokio::time::sleep(Duration::from_millis(700)).await;
                guest.close().await;
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            let (count, longest) = watch.await.unwrap();
            eprintln!("{count} pictures, the longest wait {longest:?}");
            // One black picture over and over: libwebrtc sends a static screen about 3 a second.
            assert!(count > 20, "pictures for all of 12 s, got {count}");
            assert!(
                longest < Duration::from_secs(2),
                "the picture stopped for {longest:?}"
            );

            drop(frames);
            viewer.close().await;
            streamer.close().await.unwrap();
            camp.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// A silenced member publishes nothing and yet hears: subscribing does not depend on
    /// having a track of one's own (ADR-0028, 0049).
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn a_silenced_member_still_hears_the_channel() {
        tauri::async_runtime::block_on(async {
            let (camp, dir, params) = camp_room("silent-ears").await;
            let speaker = connect(params).await;
            let speaker_id = speaker.room().local_participant().identity().to_string();

            let (client, session) = another_member(&camp, "Без голоса").await;
            camp.manager()
                .set_silenced(&session.user_id, true)
                .await
                .unwrap();
            let ears = Arc::new(Ears::default());
            let silent = listener(way_in(&client, &session).await, &ears).await;
            assert!(!may_publish(&silent.room()), "the token came without it");
            ears.wait_for(&speaker_id).await;

            silent.close().await;
            speaker.close().await;
            camp.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// The count of viewers rests on everyone saying whose stream they watch (ADR-0053):
    /// the camp's token allows it, others see it, and being silenced does not take it away.
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn a_member_says_whose_stream_they_watch() {
        tauri::async_runtime::block_on(async {
            let (camp, dir, params) = camp_room("watching").await;
            let observer = connect(params).await;
            let (client, session) = another_member(&camp, "Зритель").await;
            let member = connect(way_in(&client, &session).await).await;

            let says = |value: &'static str| {
                let room = observer.room();
                let identity = ParticipantIdentity::from(session.user_id.clone());
                async move {
                    tokio::time::timeout(Duration::from_secs(10), async {
                        loop {
                            let seen = room.remote_participants().get(&identity).is_some_and(|p| {
                                p.attributes().get("oc.watching").map(String::as_str) == Some(value)
                            });
                            if seen {
                                return;
                            }
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    })
                    .await
                    .expect("others see the attribute");
                }
            };
            let set = |value: &str| {
                let attributes = HashMap::from([("oc.watching".to_owned(), value.to_owned())]);
                let room = member.room();
                async move {
                    room.local_participant()
                        .set_attributes(attributes)
                        .await
                        .expect("the token allows it");
                }
            };
            set("a").await;
            says("a").await;

            camp.manager()
                .set_silenced(&session.user_id, true)
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            set("b").await;
            says("b").await;

            member.close().await;
            observer.close().await;
            camp.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// Records server-side disconnects.
    struct Watcher(std::sync::Mutex<Vec<String>>);

    impl RoomHost for Watcher {
        fn voice_started(&self, _: &str) -> Option<SourceInput> {
            None
        }
        fn voice_stopped(&self, _: &str) {}
        fn members_changed(&self, _: Vec<RemoteMember>) {}
        fn disconnected(&self, message: String) {
            self.0.lock().unwrap().push(message);
        }
        fn link_changed(&self, _: Link) {}
    }

    /// Records the sounds a room asks for.
    #[derive(Default)]
    struct CueEars(std::sync::Mutex<Vec<Cue>>);

    impl RoomHost for CueEars {
        fn voice_started(&self, _: &str) -> Option<SourceInput> {
            None
        }
        fn voice_stopped(&self, _: &str) {}
        fn members_changed(&self, _: Vec<RemoteMember>) {}
        fn disconnected(&self, _: String) {}
        fn link_changed(&self, _: Link) {}
        fn cue(&self, cue: Cue) {
            self.0.lock().unwrap().push(cue);
        }
    }

    impl CueEars {
        async fn wait_for(&self, want: &[Cue]) {
            let seen = tokio::time::timeout(Duration::from_secs(10), async {
                while self.0.lock().unwrap().as_slice() != want {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            });
            let heard = seen.await;
            assert!(
                heard.is_ok(),
                "wanted {want:?}, heard {:?}",
                self.0.lock().unwrap()
            );
        }
    }

    /// ADR-0075 against the real LiveKit: someone coming, their microphone (no sound), their
    /// stream, and their leaving, each once. Our own closing is silent.
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn a_room_asks_for_its_sounds_once_each() {
        tauri::async_runtime::block_on(async {
            let (camp, dir, params) = camp_room("cues").await;
            let ears = Arc::new(CueEars::default());
            let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
            let host: Arc<dyn RoomHost> = ears.clone();
            let room = VoiceRoom::connect(params, source, false, host)
                .await
                .unwrap();

            let (client, session) = another_member(&camp, "Гость").await;
            let access = way_in(&client, &session).await;
            let (guest, _) = Room::connect(&access.url, &access.token, RoomOptions::default())
                .await
                .unwrap();
            ears.wait_for(&[Cue::Joined]).await;

            let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
            let mic =
                LocalAudioTrack::create_audio_track("microphone", RtcAudioSource::Native(source));
            publish_mic(&guest, &mic).await.unwrap();
            let (screen, frames) = black_screen();
            guest
                .local_participant()
                .publish_track(
                    LocalTrack::Video(screen),
                    TrackPublishOptions {
                        source: crate::stream::STREAM_SOURCE,
                        ..TrackPublishOptions::default()
                    },
                )
                .await
                .unwrap();
            ears.wait_for(&[Cue::Joined, Cue::StreamStarted]).await;

            drop(frames);
            guest.close().await.unwrap();
            let all = [Cue::Joined, Cue::StreamStarted, Cue::Left];
            ears.wait_for(&all).await;

            // Nothing late, and nothing from our own closing.
            tokio::time::sleep(Duration::from_millis(400)).await;
            room.close().await;
            tokio::time::sleep(Duration::from_millis(400)).await;
            assert_eq!(ears.0.lock().unwrap().as_slice(), all);
            camp.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// Records connection reports.
    struct LinkWatcher(std::sync::Mutex<Vec<Link>>);

    impl RoomHost for LinkWatcher {
        fn voice_started(&self, _: &str) -> Option<SourceInput> {
            None
        }
        fn voice_stopped(&self, _: &str) {}
        fn members_changed(&self, _: Vec<RemoteMember>) {}
        fn disconnected(&self, _: String) {}
        fn link_changed(&self, link: Link) {
            self.0.lock().unwrap().push(link);
        }
    }

    /// ADR-0030 against the real LiveKit: the panel gets a quality and, once ICE has
    /// measured it, a ping.
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn the_link_reports_quality_and_ping() {
        tauri::async_runtime::block_on(async {
            let (camp, dir, params) = camp_room("link").await;
            let watcher = Arc::new(LinkWatcher(std::sync::Mutex::default()));
            let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
            let room = VoiceRoom::connect(params, source, false, watcher.clone())
                .await
                .expect("voice through the camp");
            let mut ping = None;
            for _ in 0..80 {
                ping = watcher.0.lock().unwrap().iter().find_map(|l| l.rtt_ms);
                if ping.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let seen = watcher.0.lock().unwrap().clone();
            assert!(
                ping.is_some_and(|ms| ms < 200),
                "a loopback ping within 8 s: {seen:?}"
            );
            assert_ne!(seen.last().map(|l| l.quality), Some(LinkQuality::Lost));
            room.close().await;
            camp.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// The same person joining twice (a second window on the same profile): LiveKit drops
    /// the first session, and the user must be told why instead of seeing a frozen UI.
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn a_duplicate_login_is_reported_to_the_first_session() {
        tauri::async_runtime::block_on(async {
            let (camp, dir, params) = camp_room("dup").await;
            let second = RoomParams {
                url: params.url.clone(),
                token: params.token.clone(),
                playback_rate: 48_000,
            };
            let watcher = Arc::new(Watcher(std::sync::Mutex::default()));
            let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
            let first = VoiceRoom::connect(params, source, false, watcher.clone())
                .await
                .expect("first session");
            let again = connect(second).await;

            let mut seen = Vec::new();
            for _ in 0..50 {
                seen = watcher.0.lock().unwrap().clone();
                if !seen.is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            assert_eq!(seen.len(), 1, "exactly one disconnect, got {seen:?}");
            assert!(seen[0].contains("другого окна"), "{}", seen[0]);

            again.close().await;
            first.close().await;
            camp.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    /// The whole M3 path: camp server (in-process) → login with a device key → room token →
    /// `wss://` to the camp port, pinned, proxied by `/rtc` to the real LiveKit. The SDK's own
    /// transport has no TLS at all, so connecting proves it uses ours (ADR-0019).
    #[test]
    #[ignore = "needs a local LiveKit: `pnpm livekit`"]
    fn voice_goes_through_the_camp_server_over_pinned_tls() {
        tauri::async_runtime::block_on(async {
            let (camp, dir, params) = camp_room("e2e").await;
            assert!(params.url.starts_with("wss://127.0.0.1:"), "{}", params.url);
            let room = connect(params).await;
            assert_eq!(room.room.name(), "c0");
            room.close().await;
            camp.stop().await;
            let _ = std::fs::remove_dir_all(dir);
        });
    }
}
