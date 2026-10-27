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
//! let room = camp.room_token(&session, "c0").await?; // → LiveKit url + token
//! # Ok(()) }
//! ```

mod device;
mod http;

use std::fmt;
use std::io;

use oc_invite::{Fingerprint, Host, Invite};
use oc_proto::{
    CampInfo, Challenge, ErrorCode, InviteInfo, LoginRequest, LoginResponse, Role, RoomToken,
    API_VERSION,
};

pub use device::DeviceKey;

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

    /// LiveKit address and token for a channel.
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
