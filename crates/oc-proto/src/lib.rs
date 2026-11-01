//! # oc-proto
//!
//! The camp API as both sides see it (ADR-0018): JSON shapes, error codes, and the
//! exact bytes a device signs to log in. Server and client depend on this crate, so
//! they cannot drift apart.

use std::collections::BTreeMap;

use oc_invite::{base32, base64url, Fingerprint};
use ring::signature::{UnparsedPublicKey, ED25519};
use serde::{Deserialize, Serialize};

/// Bumped on breaking changes; clients refuse servers they do not understand.
pub const API_VERSION: u32 = 1;

/// Domain separation for login signatures: a signature made for anything else
/// (including a future protocol version) can never be replayed as a login.
const LOGIN_CONTEXT: &str = "ourcamp-login-v1";

/// Length of an Ed25519 public key.
pub const PUBLIC_KEY_LEN: usize = 32;

/// Which camp a login is for: what the client checked the camp's certificate against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampKey<'a> {
    /// The pinned self-signed certificate (a link with `fp`).
    Pinned(&'a Fingerprint),
    /// A domain whose certificate an authority issued (ADR-0042), and the port.
    Named {
        /// The domain, as in the link (any case).
        host: &'a str,
        /// The camp's port.
        port: u16,
    },
}

/// The bytes a device signs to log into `camp`.
///
/// Binding to what the client verified means a signature collected by one camp is
/// worthless at any other (ADR-0018): an impostor has neither the pinned certificate's
/// key nor an authority's certificate for the domain. A fingerprint is base64url and
/// never contains `:`, so the two kinds cannot be mistaken for each other.
#[must_use]
pub fn login_message(camp: CampKey<'_>, nonce: &str) -> Vec<u8> {
    match camp {
        CampKey::Pinned(fp) => format!("{LOGIN_CONTEXT}\n{fp}\n{nonce}"),
        CampKey::Named { host, port } => format!(
            "{LOGIN_CONTEXT}\nname:{}:{port}\n{nonce}",
            host.to_ascii_lowercase()
        ),
    }
    .into_bytes()
}

/// Stable user id derived from a device public key: Crockford base32 of the first 10
/// bytes of its SHA-256 (16 characters, like invite tokens).
#[must_use]
pub fn user_id(public_key: &[u8; PUBLIC_KEY_LEN]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, public_key);
    base32::encode(&digest.as_ref()[..10])
}

/// Whether `signature` is `public_key`'s Ed25519 signature of `message`.
#[must_use]
pub fn verify(public_key: &[u8; PUBLIC_KEY_LEN], message: &[u8], signature: &[u8]) -> bool {
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(message, signature)
        .is_ok()
}

/// Encodes binary fields (keys, signatures, nonces) for JSON.
#[must_use]
pub fn encode_bytes(bytes: &[u8]) -> String {
    base64url::encode(bytes)
}

/// Decodes a binary JSON field.
#[must_use]
pub fn decode_bytes(s: &str) -> Option<Vec<u8>> {
    base64url::decode(s)
}

// ---- avatars (ADR-0035) ----

/// The largest avatar a camp keeps. The app sends a 128×128 picture of a few kilobytes.
pub const MAX_AVATAR_BYTES: usize = 96 * 1024;

/// Picture formats an avatar may be in: what webviews encode from a canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarFormat {
    /// `image/webp`: what webviews that can encode it make.
    Webp,
    /// `image/png`: the fallback where a webview cannot encode WebP.
    Png,
}

impl AvatarFormat {
    /// The format by the file's own signature, never by what the sender claims.
    #[must_use]
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else {
            None
        }
    }

    /// Its media type.
    #[must_use]
    pub fn mime(self) -> &'static str {
        match self {
            Self::Webp => "image/webp",
            Self::Png => "image/png",
        }
    }
}

/// Names one version of an avatar: base64url of the first 12 bytes of its SHA-256 (16
/// characters). Snapshots carry it; apps fetch the picture only when it changes.
#[must_use]
pub fn avatar_hash(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    encode_bytes(&digest.as_ref()[..12])
}

// ---- chats (M5, ADR-0044) ----

/// The longest message, in characters.
pub const MAX_MESSAGE_CHARS: usize = 2000;
/// The largest photo a camp keeps. The app sends one at most 1600 px on its long side.
pub const MAX_PHOTO_BYTES: usize = 2 * 1024 * 1024;
/// The largest GIF a camp keeps: it goes as it is, re-encoding would stop it moving (ADR-0076).
pub const MAX_GIF_BYTES: usize = 8 * 1024 * 1024;
/// The largest side a photo may claim, in pixels.
pub const MAX_PHOTO_SIDE: u32 = 4096;
/// The most messages one history request returns.
pub const MAX_HISTORY: u32 = 100;

/// Picture formats a photo may be in: what webviews encode, and what cameras save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoFormat {
    /// `image/webp`.
    Webp,
    /// `image/png`.
    Png,
    /// `image/jpeg`.
    Jpeg,
    /// `image/gif`.
    Gif,
}

impl PhotoFormat {
    /// The format by the file's own signature, never by what the sender claims.
    #[must_use]
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        match AvatarFormat::sniff(bytes) {
            Some(AvatarFormat::Webp) => Some(Self::Webp),
            Some(AvatarFormat::Png) => Some(Self::Png),
            None if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) => Some(Self::Jpeg),
            None if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") => Some(Self::Gif),
            None => None,
        }
    }

    /// Its media type.
    #[must_use]
    pub fn mime(self) -> &'static str {
        match self {
            Self::Webp => "image/webp",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
        }
    }

    /// The most bytes a camp keeps of a photo in this format.
    #[must_use]
    pub fn max_bytes(self) -> usize {
        match self {
            Self::Webp | Self::Png | Self::Jpeg => MAX_PHOTO_BYTES,
            Self::Gif => MAX_GIF_BYTES,
        }
    }
}

/// A photo in a message: which one, and its size for laying it out before it loads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Photo {
    /// [`avatar_hash`] of its bytes, as `POST /api/v1/photos` answered.
    pub hash: String,
    /// Pixels, 1–[`MAX_PHOTO_SIDE`].
    pub width: u32,
    /// Pixels, 1–[`MAX_PHOTO_SIDE`].
    pub height: u32,
}

/// One message in a chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    /// The chat (a text channel).
    pub channel_id: String,
    /// 1, 2, 3… within the chat: unread = the chat's last `seq` − the last one read.
    pub seq: u64,
    /// See [`user_id`]. The author may have left since.
    pub author_id: String,
    /// The author's name when they wrote it: for authors no longer in the camp.
    pub author_name: String,
    /// Unix time, milliseconds.
    pub sent_at: i64,
    /// Up to [`MAX_MESSAGE_CHARS`]; empty only with a photo.
    pub text: String,
    /// At most one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<Photo>,
    /// Removed (ADR-0048): the text and the photo are gone, the number stays taken.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

/// `POST /api/v1/channels/{id}/messages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewMessage {
    /// What to say; may be empty with a photo.
    pub text: String,
    /// A photo uploaded with `POST /api/v1/photos` just before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<Photo>,
}

/// `GET /api/v1/channels/{id}/messages`: oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    /// Up to the asked `limit`, all with `seq` below the asked `before`.
    pub messages: Vec<ChatMessage>,
}

/// `POST /api/v1/photos`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoUploaded {
    /// What a message names it by.
    pub hash: String,
}

// ---- camp ----

/// What a channel is for (ADR-0026).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// A campfire: a LiveKit room.
    #[default]
    Voice,
    /// A chat (M5).
    Text,
}

/// A channel: a campfire ("костёр") or a chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// Stable id; for a campfire also the LiveKit room name.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Absent in older camps' answers = voice.
    #[serde(default)]
    pub kind: ChannelKind,
    /// The system AFK campfire: always there, cannot be removed; whoever sits there has
    /// mic and sound off (ADR-0025). Absent in older camps' answers = `false`.
    #[serde(default)]
    pub afk: bool,
}

/// `GET /api/v1/camp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CampInfo {
    /// [`API_VERSION`] of the server.
    pub api_version: u32,
    /// Display name.
    pub name: String,
    /// Channels in display order.
    pub channels: Vec<Channel>,
    /// Anyone with the address may come in, no invite (ADR-0040). Absent in older
    /// answers = `false`.
    #[serde(default)]
    pub open: bool,
}

/// `GET /api/v1/invites/{token}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteInfo {
    /// Join or admin claim.
    pub kind: InviteKindDto,
    /// Whether redeeming it would work now.
    pub usable: bool,
}

/// Invite kind on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteKindDto {
    /// Friends invite.
    Join,
    /// One-time admin key.
    Claim,
    /// No invite: an open camp's address (ADR-0040). Never on the wire from a server;
    /// the app uses it for what a link leads to.
    Open,
}

// ---- login ----

/// `POST /api/v1/auth/challenge`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Challenge {
    /// Random, single-use, short-lived; goes into [`login_message`].
    pub nonce: String,
}

/// `POST /api/v1/auth/login`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    /// Ed25519 public key (base64url).
    pub public_key: String,
    /// Display name the user chose.
    pub name: String,
    /// From [`Challenge`].
    pub nonce: String,
    /// Signature of [`login_message`] (base64url).
    pub signature: String,
    /// Invite token: required on first login, ignored later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
}

/// Camp role of a member (ADR-0026). The owner — whoever runs the server — is not a
/// member and has no role here. Ordered: a role can do everything the ones below can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Joined with the friends link.
    Member,
    /// Kicks, moves, mutes.
    Moderator,
    /// Plus the camp's name, channels and moderators.
    Admin,
}

impl Role {
    /// Whether this role may move someone of `target`'s role (ADR-0029): moderators and
    /// admins, over their own rank and below. The owner (the server window) moves anyone.
    #[must_use]
    pub fn moves(self, target: Role) -> bool {
        self >= Role::Moderator && target <= self
    }

    /// Whether this role may kick or silence someone of `target`'s role (ADR-0033):
    /// moderators and admins, over lower ranks only — a moderator does not kick a
    /// moderator, nor an admin an admin; that is the owner's call.
    #[must_use]
    pub fn governs(self, target: Role) -> bool {
        self >= Role::Moderator && target < self
    }
}

/// `PUT /api/v1/members/{userId}/silenced` (ADR-0033).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetSilenced {
    /// Forbidden to speak until lifted.
    pub silenced: bool,
}

/// The longest name a member can have, in characters.
pub const MAX_NAME_CHARS: usize = 32;

/// A member's name as a camp keeps it, or `None` for one it refuses: empty after trimming,
/// too long, or with control characters.
#[must_use]
pub fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    let fits = !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && !name.chars().any(char::is_control);
    fits.then(|| name.to_owned())
}

/// `PUT /api/v1/members/me/name` (ADR-0057).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetName {
    /// The new name; see [`clean_name`].
    pub name: String,
}

/// `POST /api/v1/members/{userId}/move` (ADR-0029).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveMember {
    /// The voice channel to move them to.
    pub channel_id: String,
}

/// Successful login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    /// Bearer token for further requests.
    pub session: String,
    /// See [`user_id`].
    pub user_id: String,
    /// Role in this camp.
    pub role: Role,
}

/// `GET /api/v1/invites/current` (members only): the friends invite token, so the app
/// can offer "copy invite" — the link is built from the camp address it already knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentInvite {
    /// Friends invite token.
    pub token: String,
}

// ---- voice ----

/// `POST /api/v1/channels/{id}/token`. The voice server is the camp itself:
/// `wss://<the address the API was reached at>`, path `/rtc` (ADR-0019).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomToken {
    /// LiveKit access token (JWT).
    pub token: String,
}

// ---- live camp state ----

/// A message on `GET /api/v1/events` (WebSocket, `Authorization: Bearer`): JSON text
/// frames tagged by `type`. The server sends `state` on connect and on every change;
/// `moved` and `kicked` go only to the member they are about (ADR-0028).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CampEvent {
    /// Full snapshot: a missed message can never leave a client out of sync.
    State(CampState),
    /// A moderator moved you: join this campfire.
    #[serde(rename_all = "camelCase")]
    Moved {
        /// The campfire to join.
        channel_id: String,
    },
    /// You were removed from the camp; the stream ends after this.
    Kicked,
    /// Someone wrote in a chat (ADR-0044); everyone gets it.
    Message(ChatMessage),
    /// A message was removed (ADR-0048); everyone gets it.
    #[serde(rename_all = "camelCase")]
    MessageDeleted {
        /// The chat.
        channel_id: String,
        /// The message's number there.
        seq: u64,
    },
}

/// The camp as it is now: its name and channels, who is in it and who sits at which
/// campfire. Renames and channel changes travel here too (ADR-0026).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CampState {
    /// Display name of the camp.
    pub name: String,
    /// Channels in display order.
    pub channels: Vec<Channel>,
    /// Every member of the camp, sorted by id.
    pub members: Vec<MemberInfo>,
    /// Campfires with people in them, in channel order.
    pub presence: Vec<ChannelPresence>,
    /// Anyone with the address may come in (ADR-0040).
    #[serde(default)]
    pub open: bool,
    /// Chat id → the `seq` of its last message, for chats with any (ADR-0044): what an
    /// app compares with what it has read, and how it notices messages it missed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub chats: BTreeMap<String, u64>,
}

/// A camp member's profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberInfo {
    /// See [`user_id`].
    pub user_id: String,
    /// Display name.
    pub name: String,
    /// Role in the camp.
    pub role: Role,
    /// Not allowed to speak until a moderator lifts it (ADR-0028). Absent in older
    /// answers = `false`.
    #[serde(default)]
    pub silenced: bool,
    /// Hash of their avatar ([`avatar_hash`]), `None` without one (ADR-0035).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
}

/// People at one campfire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelPresence {
    /// Channel id.
    pub channel_id: String,
    /// Sorted by user id.
    pub members: Vec<PresentMember>,
}

/// Someone at a campfire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentMember {
    /// See [`user_id`].
    pub user_id: String,
    /// No microphone published, or it is muted.
    pub mic_muted: bool,
    /// Showing their screen (ADR-0049). Absent from servers older than streams.
    #[serde(default)]
    pub streaming: bool,
}

// ---- errors ----

/// Machine-readable error code; the client maps it to a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Malformed request.
    BadRequest,
    /// First login without an invite.
    InviteRequired,
    /// Invite unknown, or an admin key (retired by ADR-0026).
    InviteInvalid,
    /// The member's role does not allow the action (ADR-0026).
    Forbidden,
    /// Signature does not verify.
    BadSignature,
    /// Nonce unknown, expired or reused.
    NonceInvalid,
    /// Missing or expired session.
    Unauthorized,
    /// No such resource.
    NotFound,
    /// Moving someone who is not in voice right now (ADR-0029).
    NotInVoice,
    /// Writing while forbidden to speak (ADR-0044): it covers the chats too.
    Silenced,
    /// Writing faster than a person does (ADR-0048); a moment later it goes through.
    TooFast,
    /// Server-side failure.
    Internal,
}

/// Body of every error response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// What went wrong.
    pub error: ErrorCode,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn key() -> Ed25519KeyPair {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    fn public(key: &Ed25519KeyPair) -> [u8; PUBLIC_KEY_LEN] {
        key.public_key().as_ref().try_into().unwrap()
    }

    #[test]
    fn moderators_and_admins_move_their_rank_and_below() {
        use Role::{Admin, Member, Moderator};
        assert!(
            !Member.moves(Member),
            "members only move themselves, by joining"
        );
        assert!(Moderator.moves(Member) && Moderator.moves(Moderator));
        assert!(!Moderator.moves(Admin));
        assert!(Admin.moves(Admin) && Admin.moves(Member));
    }

    #[test]
    fn avatars_are_known_by_their_signature_not_their_claim() {
        let webp = b"RIFF\x10\0\0\0WEBPVP8 ";
        assert_eq!(AvatarFormat::sniff(webp), Some(AvatarFormat::Webp));
        assert_eq!(
            AvatarFormat::sniff(b"\x89PNG\r\n\x1a\n\0\0"),
            Some(AvatarFormat::Png)
        );
        assert_eq!(AvatarFormat::sniff(b"GIF89a"), None);
        assert_eq!(
            AvatarFormat::sniff(b"<svg onload=alert(1)>"),
            None,
            "no SVG: it runs code"
        );
        assert_eq!(AvatarFormat::sniff(b"RIFF"), None, "too short");
        let hash = avatar_hash(webp);
        assert_eq!(hash.len(), 16);
        assert_eq!(hash, avatar_hash(webp), "stable");
        assert_ne!(hash, avatar_hash(b"other"));
    }

    #[test]
    fn kicking_and_silencing_reach_lower_ranks_only() {
        use Role::{Admin, Member, Moderator};
        assert!(!Member.governs(Member));
        assert!(Moderator.governs(Member));
        assert!(
            !Moderator.governs(Moderator),
            "moderators do not kick each other"
        );
        assert!(Admin.governs(Moderator) && Admin.governs(Member));
        assert!(!Admin.governs(Admin), "only the owner kicks an admin");
    }

    #[test]
    fn login_message_is_domain_separated_and_bound_to_the_camp() {
        let a = login_message(CampKey::Pinned(&Fingerprint([1; 32])), "n");
        let b = login_message(CampKey::Pinned(&Fingerprint([2; 32])), "n");
        assert!(a.starts_with(b"ourcamp-login-v1\n"));
        assert_ne!(a, b);
        let named = |host, port| login_message(CampKey::Named { host, port }, "n");
        assert_eq!(
            named("OurCamp.kz", 7780),
            b"ourcamp-login-v1\nname:ourcamp.kz:7780\nn"
        );
        assert_ne!(
            named("ourcamp.kz", 7780),
            named("ourcamp.kz", 7781),
            "another port, another camp"
        );
        assert_ne!(named("ourcamp.kz", 7780), named("evil.kz", 7780));
    }

    #[test]
    fn signature_verifies_only_for_its_message_and_key() {
        let k = key();
        let msg = login_message(CampKey::Pinned(&Fingerprint([1; 32])), "nonce");
        let sig = k.sign(&msg);
        assert!(verify(&public(&k), &msg, sig.as_ref()));
        let other_camp = login_message(CampKey::Pinned(&Fingerprint([2; 32])), "nonce");
        assert!(
            !verify(&public(&k), &other_camp, sig.as_ref()),
            "replay at another camp"
        );
        assert!(
            !verify(&public(&key()), &msg, sig.as_ref()),
            "someone else's key"
        );
        assert!(!verify(&public(&k), &msg, &[0; 64]));
    }

    #[test]
    fn user_id_is_stable_short_and_key_specific() {
        let k = key();
        let id = user_id(&public(&k));
        assert_eq!(id.len(), 16);
        assert_eq!(id, user_id(&public(&k)));
        assert_ne!(id, user_id(&public(&key())));
    }

    #[test]
    fn camp_events_are_tagged_by_type() {
        let event = CampEvent::State(CampState {
            name: "Вечерний кооп".into(),
            channels: vec![Channel {
                id: "c1".into(),
                name: "Chat".into(),
                kind: ChannelKind::Text,
                afk: false,
            }],
            members: vec![MemberInfo {
                user_id: "u".into(),
                name: "Лена".into(),
                role: Role::Moderator,
                silenced: true,
                avatar: Some("AAAAAAAAAAAAAAAA".into()),
            }],
            presence: vec![ChannelPresence {
                channel_id: "c0".into(),
                members: vec![PresentMember {
                    user_id: "u".into(),
                    mic_muted: true,
                    streaming: true,
                }],
            }],
            open: true,
            chats: BTreeMap::from([("c1".into(), 7)]),
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "state");
        assert_eq!(json["members"][0]["role"], "moderator");
        assert_eq!(json["members"][0]["silenced"], true);
        assert_eq!(json["members"][0]["avatar"], "AAAAAAAAAAAAAAAA");
        assert_eq!(json["open"], true);
        let moved = serde_json::to_value(CampEvent::Moved {
            channel_id: "c0".into(),
        })
        .unwrap();
        assert_eq!(
            moved,
            serde_json::json!({ "type": "moved", "channelId": "c0" })
        );
        assert_eq!(
            serde_json::to_value(CampEvent::Kicked).unwrap(),
            serde_json::json!({ "type": "kicked" })
        );
        assert_eq!(json["channels"][0]["kind"], "text");
        assert!(Role::Admin > Role::Moderator && Role::Moderator > Role::Member);
        let old: Channel = serde_json::from_str(r#"{"id":"c0","name":"Лобби"}"#).unwrap();
        assert_eq!(
            (old.kind, old.afk),
            (ChannelKind::Voice, false),
            "older camps"
        );
        assert_eq!(json["presence"][0]["channelId"], "c0");
        assert_eq!(json["presence"][0]["members"][0]["micMuted"], true);
        assert_eq!(json["presence"][0]["members"][0]["streaming"], true);
        assert_eq!(json["chats"]["c1"], 7);
        assert_eq!(serde_json::from_value::<CampEvent>(json).unwrap(), event);
    }

    #[test]
    fn a_chat_message_travels_flat_and_without_an_absent_photo() {
        let message = ChatMessage {
            channel_id: "c1".into(),
            seq: 3,
            author_id: "u".into(),
            author_name: "Лена".into(),
            sent_at: 1_790_000_000_000,
            text: "Привет".into(),
            photo: None,
            deleted: false,
        };
        let json = serde_json::to_value(CampEvent::Message(message.clone())).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "message", "channelId": "c1", "seq": 3, "authorId": "u",
                "authorName": "Лена", "sentAt": 1_790_000_000_000_i64, "text": "Привет",
            })
        );
        assert_eq!(
            serde_json::from_value::<CampEvent>(json).unwrap(),
            CampEvent::Message(message)
        );
        assert_eq!(
            serde_json::to_value(CampEvent::MessageDeleted {
                channel_id: "c1".into(),
                seq: 3,
            })
            .unwrap(),
            serde_json::json!({ "type": "message_deleted", "channelId": "c1", "seq": 3 })
        );
        let old: CampState =
            serde_json::from_str(r#"{"name":"x","channels":[],"members":[],"presence":[]}"#)
                .unwrap();
        assert!(old.chats.is_empty(), "a camp from before chats");
    }

    #[test]
    fn photos_are_known_by_their_signature() {
        assert_eq!(
            PhotoFormat::sniff(b"\xFF\xD8\xFF\xE0rest"),
            Some(PhotoFormat::Jpeg)
        );
        assert_eq!(
            PhotoFormat::sniff(b"\x89PNG\r\n\x1a\nrest"),
            Some(PhotoFormat::Png)
        );
        assert_eq!(
            PhotoFormat::sniff(b"RIFF\0\0\0\0WEBPVP8 "),
            Some(PhotoFormat::Webp)
        );
        assert_eq!(PhotoFormat::sniff(b"GIF87a"), Some(PhotoFormat::Gif));
        assert_eq!(PhotoFormat::sniff(b"GIF89a"), Some(PhotoFormat::Gif));
        assert_eq!(PhotoFormat::sniff(b"GIF88a"), None);
        assert_eq!(PhotoFormat::sniff(b"BM...."), None);
        assert_eq!(PhotoFormat::Jpeg.mime(), "image/jpeg");
        assert_eq!(PhotoFormat::Gif.mime(), "image/gif");
        assert_eq!(PhotoFormat::Jpeg.max_bytes(), MAX_PHOTO_BYTES);
        assert_eq!(PhotoFormat::Gif.max_bytes(), MAX_GIF_BYTES);
    }

    #[test]
    fn json_shapes_are_camel_case_with_snake_case_enums() {
        let body = serde_json::to_value(ErrorBody {
            error: ErrorCode::InviteRequired,
        })
        .unwrap();
        assert_eq!(body, serde_json::json!({ "error": "invite_required" }));
        let login = serde_json::to_value(LoginResponse {
            session: "s".into(),
            user_id: "u".into(),
            role: Role::Admin,
        })
        .unwrap();
        assert_eq!(
            login,
            serde_json::json!({ "session": "s", "userId": "u", "role": "admin" })
        );
        let req: LoginRequest = serde_json::from_value(serde_json::json!({
            "publicKey": "k", "name": "Дима", "nonce": "n", "signature": "s"
        }))
        .unwrap();
        assert_eq!(req.invite, None, "invite is optional");
    }
}
