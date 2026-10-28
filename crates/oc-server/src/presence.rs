//! Who sits at which campfire (ADR-0021): the server asks LiveKit over loopback once a
//! second and publishes a [`CampState`] snapshot whenever it changes. `/api/v1/events`
//! streams the snapshots to clients.
//!
//! Polling instead of LiveKit webhooks (ADR-0004): webhooks would have to reach this
//! server over TLS LiveKit does not trust (self-signed), and from inside Docker in
//! development; a 1 s poll of a local API costs nothing at camp scale.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::Body;
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HOST};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use livekit_token::{AccessToken, VideoGrants};
use oc_proto::{CampState, ChannelPresence, PresentMember};
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio::sync::watch;

use crate::store::{Store, StoreResult};
use crate::LiveKit;

/// How often LiveKit is asked. Joins and leaves show up within this.
const POLL: Duration = Duration::from_secs(1);
/// One LiveKit call must answer within this, or the round is skipped.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// One participant as LiveKit's Twirp JSON describes it (only what we read; LiveKit
/// answers in `snake_case`, see `docs/decisions/0021-*`).
#[derive(Debug, Deserialize)]
struct Participant {
    identity: String,
    #[serde(default)]
    tracks: Vec<Track>,
}

#[derive(Debug, Deserialize)]
struct Track {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    muted: bool,
}

impl Participant {
    /// No microphone published, or it is muted — like `RemoteMember::mic_muted` in the app.
    fn mic_muted(&self) -> bool {
        !self
            .tracks
            .iter()
            .any(|t| t.kind == "AUDIO" && t.source == "MICROPHONE" && !t.muted)
    }
}

#[derive(Debug, Deserialize)]
struct Rooms {
    #[serde(default)]
    rooms: Vec<Room>,
}

#[derive(Debug, Deserialize)]
struct Room {
    name: String,
}

#[derive(Debug, Deserialize)]
struct Participants {
    #[serde(default)]
    participants: Vec<Participant>,
}

/// LiveKit's `RoomService` on loopback.
struct LiveKitApi {
    authority: String,
    config: LiveKit,
}

impl LiveKitApi {
    /// An admin token. LiveKit wants the room named for room-level calls.
    fn token(&self, room: &str) -> Result<String, String> {
        AccessToken::with_api_key(&self.config.api_key, &self.config.api_secret)
            .with_identity("oc-server")
            .with_ttl(Duration::from_secs(60))
            .with_grants(VideoGrants {
                room_list: true,
                room_admin: true,
                room: room.to_owned(),
                ..VideoGrants::default()
            })
            .to_jwt()
            .map_err(|e| e.to_string())
    }

    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        room: &str,
        body: String,
    ) -> Result<T, String> {
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("/twirp/livekit.RoomService/{method}"))
            .header(HOST, &self.authority)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", self.token(room)?))
            .body(Body::from(body))
            .map_err(|e| e.to_string())?;
        let exchange = async {
            let tcp = TcpStream::connect(&self.authority)
                .await
                .map_err(|e| e.to_string())?;
            let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tcp))
                .await
                .map_err(|e| e.to_string())?;
            tokio::spawn(connection);
            let response = sender
                .send_request(request)
                .await
                .map_err(|e| e.to_string())?;
            let status = response.status();
            let bytes: Bytes = response
                .into_body()
                .collect()
                .await
                .map_err(|e| e.to_string())?
                .to_bytes();
            if !status.is_success() {
                return Err(format!("{method}: HTTP {status}"));
            }
            serde_json::from_slice(&bytes).map_err(|e| format!("{method}: {e}"))
        };
        tokio::time::timeout(CALL_TIMEOUT, exchange)
            .await
            .map_err(|_| format!("{method}: timed out"))?
    }

    /// Room name → its participants.
    async fn presence(&self) -> Result<HashMap<String, Vec<Participant>>, String> {
        let rooms: Rooms = self.call("ListRooms", "", "{}".into()).await?;
        let mut out = HashMap::new();
        for room in rooms.rooms {
            let body = serde_json::json!({ "room": room.name }).to_string();
            let list: Participants = self.call("ListParticipants", &room.name, body).await?;
            out.insert(room.name, list.participants);
        }
        Ok(out)
    }
}

/// The snapshot to start with: members from the database, nobody at any campfire yet.
/// Published before LiveKit is first asked, so members show up even while it is slow.
pub fn initial(store: &Store) -> StoreResult<CampState> {
    snapshot(store, &HashMap::new())
}

/// Builds the snapshot clients see: members from the database, presence from LiveKit
/// restricted to this camp's channels and members, in channel order.
fn snapshot(store: &Store, rooms: &HashMap<String, Vec<Participant>>) -> StoreResult<CampState> {
    let members = store.members()?;
    let known: std::collections::HashSet<&str> =
        members.iter().map(|m| m.user_id.as_str()).collect();
    let channels = store.camp()?.channels;
    let presence = channels
        .iter()
        .filter_map(|channel| {
            let people: BTreeMap<&str, bool> = rooms
                .get(&channel.id)?
                .iter()
                .filter(|p| known.contains(p.identity.as_str()))
                .map(|p| (p.identity.as_str(), p.mic_muted()))
                .collect();
            (!people.is_empty()).then(|| ChannelPresence {
                channel_id: channel.id.clone(),
                members: people
                    .into_iter()
                    .map(|(user_id, mic_muted)| PresentMember {
                        user_id: user_id.to_owned(),
                        mic_muted,
                    })
                    .collect(),
            })
        })
        .collect();
    Ok(CampState { members, presence })
}

/// Polls until `shutdown` turns true, publishing changes to `out`.
pub async fn run(
    store: Arc<Mutex<Store>>,
    livekit: LiveKit,
    authority: String,
    out: watch::Sender<Arc<CampState>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let api = LiveKitApi {
        authority,
        config: livekit,
    };
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut livekit_down = false;
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = shutdown.changed() => return,
        }
        // A round can take up to `CALL_TIMEOUT` per call; stopping must not wait for it.
        let polled = tokio::select! {
            polled = api.presence() => polled,
            _ = shutdown.changed() => return,
        };
        let rooms = match polled {
            Ok(rooms) => {
                if livekit_down {
                    eprintln!("[oc-server] LiveKit reachable again");
                    livekit_down = false;
                }
                rooms
            }
            Err(e) => {
                // Logged once per outage, not every second; campfires show as empty.
                if !livekit_down {
                    eprintln!("[oc-server] presence unavailable: {e}");
                    livekit_down = true;
                }
                HashMap::new()
            }
        };
        let store = Arc::clone(&store);
        let built = tokio::task::spawn_blocking(move || {
            let store = store.lock().unwrap_or_else(PoisonError::into_inner);
            snapshot(&store, &rooms)
        })
        .await;
        match built {
            Ok(Ok(state)) => {
                // `send_if_modified`: subscribers wake only on a real change.
                out.send_if_modified(|current| {
                    let changed = **current != state;
                    if changed {
                        *current = Arc::new(state);
                    }
                    changed
                });
            }
            Ok(Err(e)) => eprintln!("[oc-server] presence snapshot: {e}"),
            Err(_) => eprintln!("[oc-server] presence snapshot task panicked"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Device;

    fn participant(json: &str) -> Participant {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn mic_state_comes_from_the_microphone_track_as_livekit_reports_it() {
        // Trimmed from a real `ListParticipants` answer of livekit-server 1.13.
        let muted = participant(
            r#"{"sid":"PA_1","identity":"u1","state":"ACTIVE","tracks":[{"sid":"TR_1","type":"AUDIO",
            "name":"microphone","muted":true,"source":"MICROPHONE","mime_type":"audio/red"}],"name":"Проба"}"#,
        );
        assert!(muted.mic_muted());
        let talking = participant(
            r#"{"identity":"u2","tracks":[{"type":"AUDIO","source":"MICROPHONE","muted":false}]}"#,
        );
        assert!(!talking.mic_muted());
        let screen_only = participant(
            r#"{"identity":"u3","tracks":[{"type":"AUDIO","source":"SCREEN_SHARE_AUDIO","muted":false}]}"#,
        );
        assert!(screen_only.mic_muted(), "stream audio is not a microphone");
        assert!(
            participant(r#"{"identity":"u4"}"#).mic_muted(),
            "no tracks at all"
        );
    }

    #[test]
    fn snapshot_keeps_channel_order_and_only_this_camps_people() {
        let mut store = Store::open_in_memory("x").unwrap();
        let join = store.join_token().unwrap().as_str().to_owned();
        for (id, name) in [("bob", "Боб"), ("ann", "Аня")] {
            let device = Device {
                user_id: id,
                public_key: id.as_bytes(),
                name,
            };
            store.login(device, Some(&join)).unwrap().unwrap();
        }
        let rooms = HashMap::from([
            (
                "c1".to_owned(),
                vec![
                    participant(r#"{"identity":"bob"}"#),
                    participant(r#"{"identity":"ann"}"#),
                ],
            ),
            (
                "c0".to_owned(),
                vec![participant(r#"{"identity":"stranger"}"#)],
            ),
            (
                "other-camp-room".to_owned(),
                vec![participant(r#"{"identity":"bob"}"#)],
            ),
        ]);
        let state = snapshot(&store, &rooms).unwrap();
        assert_eq!(state.members.len(), 2);
        assert_eq!(
            state.presence.len(),
            1,
            "c0 had only a stranger; unknown rooms ignored"
        );
        assert_eq!(state.presence[0].channel_id, "c1");
        let ids: Vec<_> = state.presence[0]
            .members
            .iter()
            .map(|m| m.user_id.as_str())
            .collect();
        assert_eq!(ids, ["ann", "bob"], "sorted for a stable snapshot");
    }
}
