//! LiveKit's `RoomService` over loopback (Twirp, JSON): who sits in which room for
//! presence (ADR-0021), closing the room of a removed campfire and moderation (ADR-0026).

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use axum::body::Body;
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use livekit_token::{AccessToken, VideoGrants};
use serde::Deserialize;
use tokio::net::TcpStream;

use crate::LiveKit;

/// One LiveKit call must answer within this.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// One participant as LiveKit's Twirp JSON describes it (only what we read; LiveKit
/// answers in `snake_case`, see `docs/decisions/0021-*`).
#[derive(Debug, Deserialize)]
pub struct Participant {
    pub identity: String,
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
    pub fn mic_muted(&self) -> bool {
        !self
            .tracks
            .iter()
            .any(|t| t.kind == "AUDIO" && t.source == "MICROPHONE" && !t.muted)
    }

    /// Showing their screen (ADR-0049). The sound of a stream alone does not count.
    pub fn streaming(&self) -> bool {
        self.tracks
            .iter()
            .any(|t| t.kind == "VIDEO" && t.source == "SCREEN_SHARE" && !t.muted)
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

/// Why a call failed.
#[derive(Debug)]
pub enum RoomError {
    /// LiveKit answered with this status.
    Status(StatusCode),
    /// No answer: down, timed out, or not HTTP.
    Unreachable(String),
}

impl fmt::Display for RoomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status) => write!(f, "LiveKit answered HTTP {status}"),
            Self::Unreachable(why) => write!(f, "LiveKit unreachable: {why}"),
        }
    }
}

/// LiveKit's `RoomService` on loopback.
pub struct RoomService {
    authority: String,
    config: LiveKit,
}

impl RoomService {
    pub fn new(authority: String, config: LiveKit) -> Self {
        Self { authority, config }
    }

    /// An admin token. LiveKit wants the room named for room-level calls.
    fn token(&self, room: &str) -> Result<String, RoomError> {
        AccessToken::with_api_key(&self.config.api_key, &self.config.api_secret)
            .with_identity("oc-server")
            .with_ttl(Duration::from_secs(60))
            .with_grants(VideoGrants {
                room_list: true,
                room_admin: true,
                room_create: true,
                room: room.to_owned(),
                ..VideoGrants::default()
            })
            .to_jwt()
            .map_err(|e| RoomError::Unreachable(e.to_string()))
    }

    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        room: &str,
        body: String,
    ) -> Result<T, RoomError> {
        let unreachable = |e: &dyn fmt::Display| RoomError::Unreachable(format!("{method}: {e}"));
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("/twirp/livekit.RoomService/{method}"))
            .header(HOST, &self.authority)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", self.token(room)?))
            .body(Body::from(body))
            .map_err(|e| unreachable(&e))?;
        let exchange = async {
            let tcp = TcpStream::connect(&self.authority)
                .await
                .map_err(|e| unreachable(&e))?;
            let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tcp))
                .await
                .map_err(|e| unreachable(&e))?;
            tokio::spawn(connection);
            let response = sender
                .send_request(request)
                .await
                .map_err(|e| unreachable(&e))?;
            let status = response.status();
            let bytes: Bytes = response
                .into_body()
                .collect()
                .await
                .map_err(|e| unreachable(&e))?
                .to_bytes();
            if !status.is_success() {
                return Err(RoomError::Status(status));
            }
            serde_json::from_slice(&bytes).map_err(|e| unreachable(&e))
        };
        tokio::time::timeout(CALL_TIMEOUT, exchange)
            .await
            .map_err(|_| unreachable(&"timed out"))?
    }

    /// Room name → its participants.
    pub async fn presence(&self) -> Result<HashMap<String, Vec<Participant>>, RoomError> {
        let rooms: Rooms = self.call("ListRooms", "", "{}".into()).await?;
        let mut out = HashMap::new();
        for room in rooms.rooms {
            let body = serde_json::json!({ "room": room.name }).to_string();
            let list: Participants = self.call("ListParticipants", &room.name, body).await?;
            out.insert(room.name, list.participants);
        }
        Ok(out)
    }

    /// Takes `identity` out of `room` (they see "removed"). Not being there is no error.
    pub async fn remove_participant(&self, room: &str, identity: &str) -> Result<(), RoomError> {
        let body = serde_json::json!({ "room": room, "identity": identity }).to_string();
        match self
            .call::<serde_json::Value>("RemoveParticipant", room, body)
            .await
        {
            Ok(_) | Err(RoomError::Status(StatusCode::NOT_FOUND)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Allows or forbids `identity` to publish in `room`. Forbidding makes LiveKit take
    /// their published tracks down at once (ADR-0028).
    pub async fn set_can_publish(
        &self,
        room: &str,
        identity: &str,
        can_publish: bool,
    ) -> Result<(), RoomError> {
        let body = serde_json::json!({
            "room": room,
            "identity": identity,
            "permission": {
                "can_subscribe": true,
                "can_publish": can_publish,
                "can_publish_data": true,
                "can_update_metadata": true,
            },
        })
        .to_string();
        match self
            .call::<serde_json::Value>("UpdateParticipant", room, body)
            .await
        {
            Ok(_) | Err(RoomError::Status(StatusCode::NOT_FOUND)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Closes a room: everyone in it is disconnected (they see "room deleted"). A room
    /// that does not exist — nobody ever sat there — is no error.
    pub async fn delete_room(&self, room: &str) -> Result<(), RoomError> {
        let body = serde_json::json!({ "room": room }).to_string();
        match self
            .call::<serde_json::Value>("DeleteRoom", room, body)
            .await
        {
            Ok(_) | Err(RoomError::Status(StatusCode::NOT_FOUND)) => Ok(()),
            Err(e) => Err(e),
        }
    }
}
