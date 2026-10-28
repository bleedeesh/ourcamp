//! # oc-camp-client
//!
//! What a device needs to talk to a camp (ADR-0018), without any UI:
//!
//! * [`DeviceKey`] — the Ed25519 key that *is* the user until accounts exist (ADR-0015);
//! * [`CampClient`] — HTTPS to the camp, trusting only the certificate pinned in the
//!   invite link (`oc-tls`), and the API calls on top.
//!
//! ```no_run
//! # async fn demo(link: &str) -> Result<(), oc_camp_client::ClientError> {
//! use oc_camp_client::{CampClient, DeviceKey};
//!
//! let invite: oc_invite::Invite = link.parse().unwrap();
//! let key = DeviceKey::load_or_create("device.key".as_ref())?;
//! let camp = CampClient::new(&invite);
//! let session = camp.login(&key, "Дима", Some(invite.token.as_str())).await?;
//! oc_camp_client::transport::install(oc_camp_client::transport::PlainLoopback::Deny); // once
//! let room = camp.room_token(&session, "c0").await?;
//! // livekit::Room::connect(&camp.voice_url(), &room.token, Default::default())
//! # Ok(()) }
//! ```

mod device;
mod events;
mod http;
pub mod transport;

use std::fmt;
use std::io;

use oc_invite::{Fingerprint, Host, Invite, InviteKind, Token};
use oc_proto::{
    CampInfo, Challenge, CurrentInvite, ErrorCode, InviteInfo, LoginRequest, LoginResponse, Role,
    RoomToken, API_VERSION,
};

pub use device::DeviceKey;
pub use events::CampEvents;

/// Why a camp request failed.
#[derive(Debug)]
pub enum ClientError {
    /// Network or TLS failure — including a certificate that does not match the link.
    Io(io::Error),
    /// The camp answered with an error.
    Api(ErrorCode),
    /// The camp answered something this client does not understand.
    Protocol(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "camp unreachable: {e}"),
            Self::Api(code) => write!(f, "camp refused: {code:?}"),
            Self::Protocol(m) => write!(f, "unexpected answer from the camp: {m}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// A logged-in device. The session token is a secret: it is not printed by `Debug`.
#[derive(Clone)]
pub struct Session {
    token: String,
    /// The device's id in this camp.
    pub user_id: String,
    /// Role in this camp.
    pub role: Role,
}

impl Session {
    /// A session saved earlier (the app keeps it between runs).
    #[must_use]
    pub fn restore(token: String, user_id: String, role: Role) -> Self {
        Self {
            token,
            user_id,
            role,
        }
    }

    /// The bearer token, for saving. A secret: store it like a password.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("user_id", &self.user_id)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

/// One camp, reached over pinned TLS.
pub struct CampClient {
    http: http::Client,
    host: Host,
    port: u16,
    fingerprint: Fingerprint,
}

impl CampClient {
    /// The camp an invite link points to.
    #[must_use]
    pub fn new(invite: &Invite) -> Self {
        Self::at(&invite.host, invite.port, invite.fingerprint)
    }

    /// A camp by address and certificate fingerprint.
    #[must_use]
    pub fn at(host: &Host, port: u16, fingerprint: Fingerprint) -> Self {
        Self {
            http: http::Client::new(format!("{host}:{port}"), fingerprint),
            host: host.clone(),
            port,
            fingerprint,
        }
    }

    /// Name and channels. Fails with [`ClientError::Protocol`] for a server speaking a
    /// different API version.
    ///
    /// # Errors
    /// See [`ClientError`].
    pub async fn camp(&self) -> Result<CampInfo, ClientError> {
        let info: CampInfo = self.http.get("/api/v1/camp").await?;
        if info.api_version != API_VERSION {
            return Err(ClientError::Protocol(format!(
                "camp speaks API v{}, this client v{API_VERSION}",
                info.api_version
            )));
        }
        Ok(info)
    }

    /// Whether an invite token is still good.
    ///
    /// # Errors
    /// [`ClientError::Api`] with [`ErrorCode::NotFound`] for unknown tokens.
    pub async fn invite(&self, token: &str) -> Result<InviteInfo, ClientError> {
        self.http.get(&format!("/api/v1/invites/{token}")).await
    }

    /// Proves possession of `key` and enters the camp. `invite` is required the first
    /// time this device comes to this camp.
    ///
    /// # Errors
    /// See [`ClientError`]; refusals arrive as [`ClientError::Api`].
    pub async fn login(
        &self,
        key: &DeviceKey,
        name: &str,
        invite: Option<&str>,
    ) -> Result<Session, ClientError> {
        let Challenge { nonce } = self.http.post("/api/v1/auth/challenge", None, &()).await?;
        let signature = key.sign(&oc_proto::login_message(&self.fingerprint, &nonce));
        let request = LoginRequest {
            public_key: oc_proto::encode_bytes(&key.public_key()),
            name: name.to_owned(),
            nonce,
            signature: oc_proto::encode_bytes(&signature),
            invite: invite.map(str::to_owned),
        };
        let response: LoginResponse = self.http.post("/api/v1/auth/login", None, &request).await?;
        if response.user_id != key.user_id() {
            return Err(ClientError::Protocol(
                "camp assigned someone else's id".into(),
            ));
        }
        Ok(Session {
            token: response.session,
            user_id: response.user_id,
            role: response.role,
        })
    }

    /// Where this camp's voice server is: the camp itself (`/rtc` on the same port,
    /// ADR-0019). Also pins the camp's certificate for the LiveKit transport, so the SDK
    /// will connect there and nowhere else (see [`transport`]).
    #[must_use]
    pub fn voice_url(&self) -> String {
        transport::pin(&self.http.authority, self.fingerprint);
        format!("wss://{}", self.http.authority)
    }

    /// Live camp state (members, who is at which campfire) until dropped (ADR-0021).
    ///
    /// # Errors
    /// [`ErrorCode::Unauthorized`] for an expired session; network/TLS errors.
    pub async fn events(&self, session: &Session) -> Result<CampEvents, ClientError> {
        CampEvents::connect(&self.http.authority, self.fingerprint, &session.token).await
    }

    /// The camp's current friends link, for "copy invite".
    ///
    /// # Errors
    /// [`ErrorCode::Unauthorized`] for an expired session.
    pub async fn current_invite(&self, session: &Session) -> Result<Invite, ClientError> {
        let CurrentInvite { token } = self
            .http
            .get_authorized("/api/v1/invites/current", &session.token)
            .await?;
        let token = Token::parse(&token, InviteKind::Join)
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        Ok(Invite {
            host: self.host.clone(),
            port: self.port,
            kind: InviteKind::Join,
            token,
            fingerprint: self.fingerprint,
        })
    }

    /// LiveKit token for a channel (connect it to [`voice_url`](Self::voice_url)).
    ///
    /// # Errors
    /// [`ErrorCode::Unauthorized`] for an expired session, [`ErrorCode::NotFound`] for an
    /// unknown channel.
    pub async fn room_token(
        &self,
        session: &Session,
        channel_id: &str,
    ) -> Result<RoomToken, ClientError> {
        self.http
            .post(
                &format!("/api/v1/channels/{channel_id}/token"),
                Some(&session.token),
                &(),
            )
            .await
    }
}
