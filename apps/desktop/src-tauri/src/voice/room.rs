//! One LiveKit room = one voice channel ("костёр"), ADR-0004.
//!
//! Publishes the local microphone with the ADR-0008 settings, turns every remote
//! microphone track into a mixer source and reports who is in the room. The room knows
//! nothing about the mixer or the UI: it talks to the engine through [`RoomHost`].

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::StreamExt;
use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::*;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::RtcAudioSource;
use livekit::webrtc::audio_stream::native::{NativeAudioStream, NativeAudioStreamOptions};
use oc_audio::mixer::SourceInput;
use oc_audio::pcm::i16_to_f32;
use tauri::async_runtime::JoinHandle;

/// Opus target for voice: 64 kbit/s VBR, mono (ADR-0008).
const VOICE_BITRATE: u64 = 64_000;
/// Decoded frames (10 ms each) buffered between libwebrtc and our copy task. `NetEq` is
/// the real jitter buffer; this only absorbs task scheduling hiccups.
const REMOTE_QUEUE_FRAMES: usize = 20;

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

/// What the room needs from the engine.
pub trait RoomHost: Send + Sync + 'static {
    /// A member's microphone started arriving: register a mixer source for it.
    /// `None` if the mixer refused (the error is the host's to report).
    fn voice_started(&self, user_id: &str) -> Option<SourceInput>;
    /// A member's microphone is gone (unpublished, left, or we left).
    fn voice_stopped(&self, user_id: &str);
    /// The set of remote members or their mic state changed. Always the full list.
    fn members_changed(&self, members: Vec<RemoteMember>);
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

/// A connected room with the microphone published.
pub struct VoiceRoom {
    room: Arc<Room>,
    mic: LocalAudioTrack,
    events: JoinHandle<()>,
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
        let (room, events) = Room::connect(&params.url, &params.token, RoomOptions::default())
            .await
            .map_err(|e| format!("Не удалось подключиться к {}: {e}", params.url))?;
        let room = Arc::new(room);

        let track = LocalAudioTrack::create_audio_track("microphone", RtcAudioSource::Native(mic));
        if mic_muted {
            track.mute();
        }
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
        if let Err(e) = room
            .local_participant()
            .publish_track(LocalTrack::Audio(track.clone()), options)
            .await
        {
            let _ = room.close().await;
            return Err(format!("Не удалось включить микрофон в комнате: {e}"));
        }

        // Participants already in the room were announced before we subscribed to
        // events; report them once up front.
        host.members_changed(members(&room));
        let events = tauri::async_runtime::spawn(handle_events(
            Arc::clone(&room),
            events,
            host,
            params.playback_rate,
        ));
        Ok(Self {
            room,
            mic: track,
            events,
        })
    }

    /// Mutes the published microphone; others see it as muted.
    pub fn set_mic_muted(&self, muted: bool) {
        if muted {
            self.mic.mute();
        } else {
            self.mic.unmute();
        }
    }

    /// Leaves the room and stops every remote voice task. The engine removes the
    /// mixer sources itself, so no [`RoomHost`] callbacks are made.
    pub async fn close(self) {
        if let Err(e) = self.room.close().await {
            eprintln!("[oc-voice] leaving room: {e}");
        }
        // Aborting the handler drops its voice tasks (see `AbortOnDrop`), so nothing
        // keeps writing into sources the engine is about to remove.
        self.events.abort();
        let _ = self.events.await;
    }
}

/// Aborts a task when dropped. Tokio's `JoinHandle` only detaches on drop, which
/// would leave a voice task running after its room is gone.
struct AbortOnDrop(JoinHandle<()>);

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

async fn handle_events(
    room: Arc<Room>,
    mut events: tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
    host: Arc<dyn RoomHost>,
    playback_rate: u32,
) {
    let mut voices: HashMap<String, AbortOnDrop> = HashMap::new();
    let mut last_members = members(&room);
    let stop_voice = |voices: &mut HashMap<String, AbortOnDrop>, user_id: &str| {
        if voices.remove(user_id).is_some() {
            host.voice_stopped(user_id);
        }
    };

    while let Some(event) = events.recv().await {
        match event {
            RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(track),
                publication,
                participant,
            } if publication.source() == TrackSource::Microphone => {
                let user_id = participant.identity().to_string();
                stop_voice(&mut voices, &user_id);
                if let Some(input) = host.voice_started(&user_id) {
                    let stream = NativeAudioStream::with_options(
                        track.rtc_track(),
                        i32::try_from(playback_rate).expect("sample rate fits in i32"),
                        1,
                        NativeAudioStreamOptions {
                            queue_size_frames: Some(REMOTE_QUEUE_FRAMES),
                        },
                    );
                    let task = tauri::async_runtime::spawn(play(stream, input));
                    voices.insert(user_id, AbortOnDrop(task));
                }
            }
            RoomEvent::TrackUnsubscribed {
                publication,
                participant,
                ..
            } if publication.source() == TrackSource::Microphone => {
                stop_voice(&mut voices, &participant.identity().to_string());
            }
            RoomEvent::ParticipantDisconnected(participant) => {
                stop_voice(&mut voices, &participant.identity().to_string());
            }
            RoomEvent::Disconnected { reason } => {
                eprintln!("[oc-voice] disconnected: {reason:?}");
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

/// Copies one remote voice into its mixer source until the track ends.
async fn play(mut stream: NativeAudioStream, mut input: SourceInput) {
    let mut buf: Vec<f32> = Vec::new();
    while let Some(frame) = stream.next().await {
        // Frames are 10 ms; the buffer grows once and is reused.
        buf.resize(frame.data.len(), 0.0);
        i16_to_f32(&frame.data, &mut buf);
        input.write(&buf);
    }
}
