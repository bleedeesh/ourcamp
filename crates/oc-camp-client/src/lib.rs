//! # oc-camp-client
//!
//! What a device needs to talk to a camp (ADR-0018), without any UI:
//!
//! * [`DeviceKey`] — the Ed25519 key that *is* the user until accounts exist (ADR-0015);
//! * [`CampClient`] — HTTPS to the camp, trusting only the certificate pinned in the
//!   invite link, or an authority's certificate for the camp's domain (`oc-tls`), and
//!   the API calls on top.
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

use oc_invite::{Host, Invite, InviteKind, Token};
use oc_proto::{
    CampInfo, CampKey, Challenge, ChatMessage, CurrentInvite, ErrorCode, History, InviteInfo,
    LoginRequest, LoginResponse, MoveMember, NewMessage, PhotoUploaded, Role, RoomToken, SetName,
    SetSilenced, API_VERSION,
};
pub use oc_tls::Trust;

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

/// What was wrong with the camp's certificate, for a message a person understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateProblem {
    /// Not the certificate pinned in the link: an impostor, or a reinstalled camp.
    NotPinned,
    /// A domain's certificate that has run out (the owner did not renew it).
    Expired,
    /// A certificate for another name: an impostor, or a mistake in the camp's setup.
    WrongName,
    /// Not from an authority this computer trusts (self-signed, or an impostor).
    UnknownIssuer,
    /// Anything else wrong with it.
    Other,
}

impl ClientError {
    /// The certificate problem behind a failed connection, if that is what it was.
    #[must_use]
    pub fn certificate_problem(&self) -> Option<CertificateProblem> {
        use tokio_rustls::rustls::{CertificateError as E, Error};
        let Self::Io(e) = self else { return None };
        let Error::InvalidCertificate(problem) = e.get_ref()?.downcast_ref::<Error>()? else {
            return None;
        };
        Some(match problem {
            E::ApplicationVerificationFailure => CertificateProblem::NotPinned,
            E::Expired | E::ExpiredContext { .. } => CertificateProblem::Expired,
            E::NotValidForName | E::NotValidForNameContext { .. } => CertificateProblem::WrongName,
            E::UnknownIssuer => CertificateProblem::UnknownIssuer,
            _ => CertificateProblem::Other,
        })
    }
}

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

/// One camp, reached over TLS it is trusted by.
pub struct CampClient {
    http: http::Client,
    host: Host,
    port: u16,
}

impl CampClient {
    /// The camp an invite link points to.
    #[must_use]
    pub fn new(invite: &Invite) -> Self {
        Self::at(&invite.host, invite.port, Trust::of(invite.fingerprint))
    }

    /// A camp by address, trusted by `trust` (usually [`Trust::of`] the link's pin).
    #[must_use]
    pub fn at(host: &Host, port: u16, trust: Trust) -> Self {
        Self {
            http: http::Client::new(format!("{host}:{port}"), trust),
            host: host.clone(),
            port,
        }
    }

    /// What the login signature is bound to: what this client checks the camp by.
    fn key(&self) -> (Option<oc_invite::Fingerprint>, String) {
        (self.http.trust.pin(), self.host.to_string())
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
        let (pin, host) = self.key();
        let camp = pin.as_ref().map_or(
            CampKey::Named {
                host: &host,
                port: self.port,
            },
            CampKey::Pinned,
        );
        let signature = key.sign(&oc_proto::login_message(camp, &nonce));
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
    /// ADR-0019). Also registers how the camp is trusted with the LiveKit transport, so
    /// the SDK will connect there and nowhere else (see [`transport`]).
    #[must_use]
    pub fn voice_url(&self) -> String {
        transport::trust(&self.http.authority, self.http.trust.clone());
        format!("wss://{}", self.http.authority)
    }

    /// Live camp state (members, who is at which campfire) until dropped (ADR-0021).
    ///
    /// # Errors
    /// [`ErrorCode::Unauthorized`] for an expired session; network/TLS errors.
    pub async fn events(&self, session: &Session) -> Result<CampEvents, ClientError> {
        CampEvents::connect(&self.http.authority, &self.http.trust, &session.token).await
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
            fingerprint: self.http.trust.pin(),
        })
    }

    /// Leaves the camp: the camp forgets this member and all their sessions. Coming back
    /// takes a new invite.
    ///
    /// # Errors
    /// [`ErrorCode::Unauthorized`] for an expired session (log in again and repeat).
    pub async fn leave(&self, session: &Session) -> Result<(), ClientError> {
        self.http
            .delete_authorized("/api/v1/members/me", &session.token)
            .await
    }

    /// Moves someone sitting in voice to another campfire; their app joins it. Needs a
    /// moderator's or admin's rights over them ([`Role::moves`], ADR-0029).
    ///
    /// # Errors
    /// [`ErrorCode::Forbidden`], [`ErrorCode::NotInVoice`], [`ErrorCode::NotFound`],
    /// [`ErrorCode::BadRequest`] for a chat, [`ErrorCode::Unauthorized`].
    pub async fn move_member(
        &self,
        session: &Session,
        user_id: &str,
        channel_id: &str,
    ) -> Result<(), ClientError> {
        self.http
            .post_no_content(
                &format!("/api/v1/members/{user_id}/move"),
                &session.token,
                &MoveMember {
                    channel_id: channel_id.to_owned(),
                },
            )
            .await
    }

    /// Renames this member (ADR-0057).
    ///
    /// # Errors
    /// [`ErrorCode::BadRequest`] for a name the camp refuses, [`ErrorCode::Unauthorized`].
    pub async fn set_name(&self, session: &Session, name: &str) -> Result<(), ClientError> {
        self.http
            .put_no_content(
                "/api/v1/members/me/name",
                &session.token,
                &SetName {
                    name: name.to_owned(),
                },
            )
            .await
    }

    /// Sends this member's avatar (ADR-0035): a PNG or WebP of at most
    /// [`oc_proto::MAX_AVATAR_BYTES`]; the camp checks both.
    ///
    /// # Errors
    /// [`ErrorCode::BadRequest`] for another format or a bigger file,
    /// [`ErrorCode::Unauthorized`].
    pub async fn set_avatar(&self, session: &Session, picture: Vec<u8>) -> Result<(), ClientError> {
        let format = oc_proto::AvatarFormat::sniff(&picture)
            .ok_or_else(|| ClientError::Protocol("an avatar is a PNG or WebP".into()))?;
        self.http
            .put_bytes(
                "/api/v1/members/me/avatar",
                &session.token,
                format.mime(),
                picture,
            )
            .await
    }

    /// Drops this member's avatar.
    ///
    /// # Errors
    /// [`ErrorCode::Unauthorized`].
    pub async fn clear_avatar(&self, session: &Session) -> Result<(), ClientError> {
        self.http
            .delete_authorized("/api/v1/members/me/avatar", &session.token)
            .await
    }

    /// A member's avatar, as the camp keeps it (check it with
    /// [`oc_proto::AvatarFormat::sniff`] before showing).
    ///
    /// # Errors
    /// [`ErrorCode::NotFound`] without one, [`ErrorCode::Unauthorized`].
    pub async fn avatar(&self, session: &Session, user_id: &str) -> Result<Vec<u8>, ClientError> {
        self.http
            .get_bytes(&format!("/api/v1/members/{user_id}/avatar"), &session.token)
            .await
            .map(|b| b.to_vec())
    }

    /// A page of a chat (ADR-0044), oldest first: up to `limit` messages with `seq` below
    /// `before` (up to the latest when `None`).
    ///
    /// # Errors
    /// [`ErrorCode::NotFound`] for no such chat, [`ErrorCode::Unauthorized`].
    pub async fn history(
        &self,
        session: &Session,
        channel_id: &str,
        before: Option<u64>,
        limit: u32,
    ) -> Result<Vec<ChatMessage>, ClientError> {
        let before = before
            .map(|seq| format!("&before={seq}"))
            .unwrap_or_default();
        let path = format!("/api/v1/channels/{channel_id}/messages?limit={limit}{before}");
        let History { messages } = self.http.get_authorized(&path, &session.token).await?;
        Ok(messages)
    }

    /// Writes in a chat; everyone, this member too, also gets it as a `message` event.
    ///
    /// # Errors
    /// [`ErrorCode::Silenced`], [`ErrorCode::NotFound`], [`ErrorCode::BadRequest`] (too
    /// long, nothing in it, a photo not uploaded), [`ErrorCode::Unauthorized`].
    pub async fn post_message(
        &self,
        session: &Session,
        channel_id: &str,
        message: &NewMessage,
    ) -> Result<ChatMessage, ClientError> {
        self.http
            .post(
                &format!("/api/v1/channels/{channel_id}/messages"),
                Some(&session.token),
                message,
            )
            .await
    }

    /// Removes a message: your own, or that of someone of a lower rank (ADR-0048).
    ///
    /// # Errors
    /// [`ErrorCode::Forbidden`], [`ErrorCode::NotFound`], [`ErrorCode::Unauthorized`].
    pub async fn delete_message(
        &self,
        session: &Session,
        channel_id: &str,
        seq: u64,
    ) -> Result<(), ClientError> {
        self.http
            .delete_authorized(
                &format!("/api/v1/channels/{channel_id}/messages/{seq}"),
                &session.token,
            )
            .await
    }

    /// Uploads a photo for the message that will show it; its hash names it there.
    ///
    /// # Errors
    /// [`ClientError::Protocol`] for anything but WebP, PNG, JPEG or GIF;
    /// [`ErrorCode::BadRequest`] (too big), [`ErrorCode::Silenced`].
    pub async fn upload_photo(
        &self,
        session: &Session,
        photo: Vec<u8>,
    ) -> Result<String, ClientError> {
        let format = oc_proto::PhotoFormat::sniff(&photo)
            .ok_or_else(|| ClientError::Protocol("a photo is WebP, PNG, JPEG or GIF".into()))?;
        let PhotoUploaded { hash } = self
            .http
            .post_bytes("/api/v1/photos", &session.token, format.mime(), photo)
            .await?;
        Ok(hash)
    }

    /// A photo by its hash.
    ///
    /// # Errors
    /// [`ErrorCode::NotFound`], [`ErrorCode::Unauthorized`].
    pub async fn photo(&self, session: &Session, hash: &str) -> Result<Vec<u8>, ClientError> {
        self.http
            .get_bytes(&format!("/api/v1/photos/{hash}"), &session.token)
            .await
            .map(|b| b.to_vec())
    }

    /// Kicks someone of a lower rank ([`Role::governs`], ADR-0033); no ban.
    ///
    /// # Errors
    /// [`ErrorCode::Forbidden`], [`ErrorCode::NotFound`], [`ErrorCode::Unauthorized`].
    pub async fn kick(&self, session: &Session, user_id: &str) -> Result<(), ClientError> {
        self.http
            .delete_authorized(&format!("/api/v1/members/{user_id}"), &session.token)
            .await
    }

    /// Forbids someone of a lower rank to speak, or lifts it ([`Role::governs`]).
    ///
    /// # Errors
    /// [`ErrorCode::Forbidden`], [`ErrorCode::NotFound`], [`ErrorCode::Unauthorized`].
    pub async fn set_silenced(
        &self,
        session: &Session,
        user_id: &str,
        silenced: bool,
    ) -> Result<(), ClientError> {
        self.http
            .put_no_content(
                &format!("/api/v1/members/{user_id}/silenced"),
                &session.token,
                &SetSilenced { silenced },
            )
            .await
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
