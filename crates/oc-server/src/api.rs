//! HTTP API of the camp (`/api/v1`, ADR-0018). Clients reach it over the pinned TLS port.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::rejection::JsonRejection;
use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post};
use axum::{Json, Router};
use livekit_token::{AccessToken, VideoGrants};
use oc_invite::{Fingerprint, InviteKind};
use oc_proto::{
    login_message, user_id, verify, CampEvent, CampInfo, CampState, Challenge, CurrentInvite,
    ErrorBody, ErrorCode, InviteInfo, InviteKindDto, LoginRequest, LoginResponse, RoomToken,
    API_VERSION, PUBLIC_KEY_LEN,
};

use tokio::sync::{broadcast, watch};

use crate::manage::Directive;
use crate::store::{canonical_token, Device, LoginError, Store, StoreResult};
use crate::LiveKit;

/// A nonce must be signed and sent back within this time.
const NONCE_TTL: Duration = Duration::from_secs(60);
/// Outstanding nonces kept at most; beyond that challenges are refused until some expire
/// (a flood of challenge requests cannot grow memory without bound).
const MAX_NONCES: usize = 4096;
/// Session lifetime (ADR-0018).
const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60;
/// Room token lifetime; LiveKit refreshes it for connected clients.
const ROOM_TOKEN_TTL: Duration = Duration::from_secs(60 * 60);
/// Longest display name, in characters.
const MAX_NAME_CHARS: usize = 32;

/// State shared by request handlers.
#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<Store>>,
    nonces: Arc<Mutex<HashMap<String, Instant>>>,
    fingerprint: Fingerprint,
    livekit: Arc<LiveKit>,
    /// `host:port` of LiveKit, validated at startup (`ws://` only: it is on loopback).
    livekit_authority: Arc<str>,
    /// Latest camp snapshot (ADR-0021), streamed by `/api/v1/events`.
    events: watch::Receiver<Arc<CampState>>,
    /// Turns true when the server stops; open event streams close on it.
    shutdown: watch::Receiver<bool>,
    /// Events for one member (`moved`, `kicked`; ADR-0028), sent by the manager.
    directives: broadcast::Sender<Directive>,
}

impl AppState {
    /// Everything the handlers need.
    #[must_use]
    pub fn new(
        store: Store,
        fingerprint: Fingerprint,
        livekit: LiveKit,
        livekit_authority: &str,
        events: watch::Receiver<Arc<CampState>>,
        shutdown: watch::Receiver<bool>,
        directives: broadcast::Sender<Directive>,
    ) -> Self {
        Self {
            livekit_authority: Arc::from(livekit_authority),
            events,
            shutdown,
            directives,
            store: Arc::new(Mutex::new(store)),
            nonces: Arc::default(),
            fingerprint,
            livekit: Arc::new(livekit),
        }
    }

    /// The store, for the presence poller.
    pub fn store_handle(&self) -> Arc<Mutex<Store>> {
        Arc::clone(&self.store)
    }

    /// Runs a store call on the blocking pool: SQLite I/O must not stall the async
    /// workers that serve every other connection.
    async fn with_store<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Store) -> StoreResult<R> + Send + 'static,
    ) -> Result<R, ApiError> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || {
            // A panic mid-query cannot leave SQLite inconsistent (transactions roll back).
            let mut store = store.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut store)
        })
        .await
        .map_err(|_| ApiError(ErrorCode::Internal))?
        .map_err(|e| {
            eprintln!("[oc-server] {e}");
            ApiError(ErrorCode::Internal)
        })
    }

    /// Where `/rtc` requests are forwarded.
    pub fn livekit_authority(&self) -> &str {
        &self.livekit_authority
    }

    fn issue_nonce(&self) -> Option<String> {
        let mut nonces = self.nonces.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        nonces.retain(|_, issued| now.duration_since(*issued) < NONCE_TTL);
        if nonces.len() >= MAX_NONCES {
            return None;
        }
        let nonce = oc_proto::encode_bytes(&random::<32>());
        nonces.insert(nonce.clone(), now);
        Some(nonce)
    }

    /// Removes the nonce whether or not it is still valid: each one is tried once.
    fn take_nonce(&self, nonce: &str) -> bool {
        let mut nonces = self.nonces.lock().unwrap_or_else(PoisonError::into_inner);
        nonces
            .remove(nonce)
            .is_some_and(|issued| issued.elapsed() < NONCE_TTL)
    }
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    // The OS CSPRNG failing is unrecoverable for a server that issues secrets.
    getrandom::fill(&mut bytes).expect("operating system random number generator");
    bytes
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .to_vec()
}

/// An error as the client sees it: a status and an [`ErrorCode`].
#[derive(Debug)]
struct ApiError(ErrorCode);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            ErrorCode::BadRequest => StatusCode::BAD_REQUEST,
            ErrorCode::BadSignature | ErrorCode::NonceInvalid | ErrorCode::Unauthorized => {
                StatusCode::UNAUTHORIZED
            }
            ErrorCode::InviteRequired | ErrorCode::InviteInvalid | ErrorCode::Forbidden => {
                StatusCode::FORBIDDEN
            }
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(ErrorBody { error: self.0 })).into_response()
    }
}

/// Malformed JSON gets the same error shape as everything else.
impl From<JsonRejection> for ApiError {
    fn from(_: JsonRejection) -> Self {
        Self(ErrorCode::BadRequest)
    }
}

/// The API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/camp", get(camp))
        .route("/api/v1/invites/current", get(current_invite))
        .route("/api/v1/invites/{token}", get(invite))
        .route("/api/v1/auth/challenge", post(challenge))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/members/me", delete(leave))
        .route("/api/v1/channels/{id}/token", post(room_token))
        .route("/api/v1/events", get(events))
        .route("/rtc", any(crate::rtc::proxy))
        .route("/rtc/{*rest}", any(crate::rtc::proxy))
        .with_state(state)
}

async fn camp(State(state): State<AppState>) -> Result<Json<CampInfo>, ApiError> {
    let camp = state.with_store(|s| s.camp()).await?;
    Ok(Json(CampInfo {
        api_version: API_VERSION,
        name: camp.name,
        channels: camp.channels,
    }))
}

async fn invite(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<InviteInfo>, ApiError> {
    // Lenient like the link parser (case, look-alikes), then an exact lookup.
    let token = canonical_token(&token).ok_or(ApiError(ErrorCode::NotFound))?;
    let status = state
        .with_store(move |s| s.invite_status(token.as_str()))
        .await?
        .ok_or(ApiError(ErrorCode::NotFound))?;
    Ok(Json(InviteInfo {
        kind: match status.kind {
            InviteKind::Join => InviteKindDto::Join,
            InviteKind::ClaimAdmin => InviteKindDto::Claim,
        },
        usable: status.usable,
    }))
}

async fn challenge(State(state): State<AppState>) -> Result<Json<Challenge>, ApiError> {
    let nonce = state.issue_nonce().ok_or(ApiError(ErrorCode::Internal))?;
    Ok(Json(Challenge { nonce }))
}

async fn login(
    State(state): State<AppState>,
    body: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<Json<LoginResponse>, ApiError> {
    let Json(req) = body?;
    let bad = || ApiError(ErrorCode::BadRequest);
    let public_key: [u8; PUBLIC_KEY_LEN] = oc_proto::decode_bytes(&req.public_key)
        .and_then(|k| k.try_into().ok())
        .ok_or_else(bad)?;
    let signature = oc_proto::decode_bytes(&req.signature).ok_or_else(bad)?;
    let name = req.name.trim().to_owned();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(bad());
    }

    // Nonce first: whatever happens next, this nonce cannot be tried again.
    if !state.take_nonce(&req.nonce) {
        return Err(ApiError(ErrorCode::NonceInvalid));
    }
    if !verify(
        &public_key,
        &login_message(&state.fingerprint, &req.nonce),
        &signature,
    ) {
        return Err(ApiError(ErrorCode::BadSignature));
    }

    let id = user_id(&public_key);
    // A token that does not even parse can still be passed through: for a new device
    // it is simply not found (`invite_invalid`), for a known one it is irrelevant.
    let invite = req
        .invite
        .map(|t| canonical_token(&t).map_or(t, |c| c.as_str().to_owned()));
    let session = random::<32>();
    let session_hash = sha256(&session);
    let (user_id, role) = {
        let id = id.clone();
        state
            .with_store(move |s| {
                let device = Device {
                    user_id: &id,
                    public_key: &public_key,
                    name: &name,
                };
                let role = match s.login(device, invite.as_deref())? {
                    Ok(role) => role,
                    Err(e) => return Ok(Err(e)),
                };
                s.create_session(&session_hash, &id, SESSION_TTL_SECS)?;
                Ok(Ok((id, role)))
            })
            .await?
            .map_err(|e| {
                ApiError(match e {
                    LoginError::InviteRequired => ErrorCode::InviteRequired,
                    LoginError::InviteInvalid => ErrorCode::InviteInvalid,
                })
            })?
    };
    Ok(Json(LoginResponse {
        session: oc_proto::encode_bytes(&session),
        user_id,
        role,
    }))
}

/// SHA-256 of the bearer session token, or `unauthorized`.
fn session_hash(headers: &HeaderMap) -> Result<Vec<u8>, ApiError> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(oc_proto::decode_bytes)
        .map(|session| sha256(&session))
        .ok_or(ApiError(ErrorCode::Unauthorized))
}

async fn current_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<CurrentInvite>, ApiError> {
    let hash = session_hash(&headers)?;
    let (member, token) = state
        .with_store(move |s| Ok((s.session_member(&hash)?, s.join_token()?)))
        .await?;
    member.ok_or(ApiError(ErrorCode::Unauthorized))?;
    Ok(Json(CurrentInvite {
        token: token.as_str().to_owned(),
    }))
}

/// `DELETE /api/v1/members/me`: the member leaves the camp. Their sessions end with it;
/// coming back takes a new invite. The next presence snapshot no longer lists them.
async fn leave(State(state): State<AppState>, headers: HeaderMap) -> Result<StatusCode, ApiError> {
    let hash = session_hash(&headers)?;
    let left = state
        .with_store(move |s| match s.session_member(&hash)? {
            Some(member) => s.remove_member(&member.user_id),
            None => Ok(false),
        })
        .await?;
    if left {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(ErrorCode::Unauthorized))
    }
}

async fn room_token(
    State(state): State<AppState>,
    Path(channel): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RoomToken>, ApiError> {
    let unauthorized = || ApiError(ErrorCode::Unauthorized);
    let hash = session_hash(&headers)?;
    let (member, exists) = {
        let channel = channel.clone();
        state
            .with_store(move |s| Ok((s.session_member(&hash)?, s.campfire_exists(&channel)?)))
            .await?
    };
    let member = member.ok_or_else(unauthorized)?;
    if !exists {
        return Err(ApiError(ErrorCode::NotFound));
    }

    let token = AccessToken::with_api_key(&state.livekit.api_key, &state.livekit.api_secret)
        .with_identity(&member.user_id)
        .with_name(&member.name)
        .with_ttl(ROOM_TOKEN_TTL)
        .with_grants(VideoGrants {
            room_join: true,
            room: channel,
            // A silenced member joins listening only; LiveKit enforces it (ADR-0028).
            can_publish: Some(!member.silenced),
            can_subscribe: Some(true),
            ..VideoGrants::default()
        })
        .to_jwt()
        .map_err(|e| {
            eprintln!("[oc-server] room token: {e}");
            ApiError(ErrorCode::Internal)
        })?;
    Ok(Json(RoomToken { token }))
}

/// Keeps idle event connections alive through NATs and proxies.
const EVENTS_PING: Duration = Duration::from_secs(30);

/// `GET /api/v1/events`: a WebSocket of [`CampEvent`]s for members (ADR-0021). The session
/// is checked before the upgrade, so strangers get a plain 401.
async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let hash = session_hash(&headers)?;
    let member = state
        .with_store(move |s| s.session_member(&hash))
        .await?
        .ok_or(ApiError(ErrorCode::Unauthorized))?;
    let stream = EventStream {
        user_id: member.user_id,
        events: state.events.clone(),
        directives: state.directives.subscribe(),
        shutdown: state.shutdown.clone(),
    };
    Ok(upgrade.on_upgrade(move |socket| stream_events(socket, stream)))
}

/// One member's `/api/v1/events` stream.
struct EventStream {
    /// Whose stream it is: directives for anyone else are skipped.
    user_id: String,
    events: watch::Receiver<Arc<CampState>>,
    directives: broadcast::Receiver<Directive>,
    shutdown: watch::Receiver<bool>,
}

async fn stream_events(mut socket: WebSocket, stream: EventStream) {
    let EventStream {
        user_id,
        mut events,
        mut directives,
        mut shutdown,
    } = stream;
    // `wait_for` hands back a read guard (`watch::Ref`, not `Send`); drop it right away
    // so it is not held across the `await` that sends the close frame.
    let stopping = async move {
        let _ = shutdown.wait_for(|&stopping| stopping).await;
    };
    tokio::pin!(stopping);
    let send =
        |event: &CampEvent| serde_json::to_string(event).map(|json| Message::Text(json.into()));
    let first = send(&CampEvent::State((**events.borrow_and_update()).clone()));
    let Ok(first) = first else { return };
    if socket.send(first).await.is_err() {
        return;
    }
    let mut ping = tokio::time::interval(EVENTS_PING);
    ping.tick().await; // the first tick is immediate
    loop {
        tokio::select! {
            changed = events.changed() => {
                if changed.is_err() {
                    break; // the poller is gone: the server is stopping
                }
                let state = (**events.borrow_and_update()).clone();
                let Ok(message) = send(&CampEvent::State(state)) else { return };
                if socket.send(message).await.is_err() {
                    return;
                }
            }
            directive = directives.recv() => match directive {
                Ok(directive) if directive.user_id == user_id => {
                    let Ok(message) = send(&directive.event) else { return };
                    if socket.send(message).await.is_err() {
                        return;
                    }
                    if directive.event == CampEvent::Kicked {
                        // No longer a member: say goodbye and end the stream.
                        let close = CloseFrame {
                            code: close_code::POLICY,
                            reason: "removed from the camp".into(),
                        };
                        let _ = socket.send(Message::Close(Some(close))).await;
                        return;
                    }
                }
                // Someone else's, or missed while slow: a missed `kicked` still ends in
                // a refused reconnect, a missed `moved` is only a missed move.
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                // Clients send nothing but control frames; any close or error ends it.
                Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
            () = &mut stopping => break,
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
        }
    }
    // Stopping says so at once instead of leaving clients to notice a dead connection:
    // they show the camp as offline right away.
    let close = CloseFrame {
        code: close_code::AWAY,
        reason: "camp server stopping".into(),
    };
    let _ = socket.send(Message::Close(Some(close))).await;
}
