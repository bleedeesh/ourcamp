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

/// A connected room with the microphone published.
pub struct VoiceRoom {
    room: Arc<Room>,
    mic: LocalAudioTrack,
    events: JoinHandle<()>,
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
            runtime: tokio::runtime::Handle::current(),
        })
    }

    /// Mutes the published microphone; others see it as muted. Callable from any
    /// thread.
    pub fn set_mic_muted(&self, muted: bool) {
        let _runtime = self.runtime.enter();
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

#[cfg(test)]
mod tests {
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
    }

    /// A camp server in this process (LiveKit from `pnpm livekit`), a fresh device logged
    /// in, and the room parameters for channel `c0` — the real path, pinned TLS included.
    async fn camp_room(name: &str) -> (oc_server::Running, std::path::PathBuf, RoomParams) {
        install(PlainLoopback::Deny);
        let data_dir = std::env::temp_dir().join(format!("oc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data_dir);
        let camp = oc_server::start(oc_server::Config {
            data_dir: data_dir.clone(),
            bind: (std::net::Ipv4Addr::LOCALHOST, 0).into(),
            public_host: oc_invite::Host::Ipv4(std::net::Ipv4Addr::LOCALHOST),
            public_port: None,
            camp_name: name.into(),
            livekit: oc_server::LiveKit {
                url: "ws://127.0.0.1:7880".into(),
                api_key: "devkey".into(),
                api_secret: "secret".into(),
            },
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
