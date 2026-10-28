//! # oc-proto
//!
//! The camp API as both sides see it (ADR-0018): JSON shapes, error codes, and the
//! exact bytes a device signs to log in. Server and client depend on this crate, so
//! they cannot drift apart.

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

/// The bytes a device signs to log into the camp whose certificate is `camp`.
///
/// Binding to the camp's fingerprint means a signature collected by one camp is
/// worthless at any other (ADR-0018).
#[must_use]
pub fn login_message(camp: &Fingerprint, nonce: &str) -> Vec<u8> {
    format!("{LOGIN_CONTEXT}\n{camp}\n{nonce}").into_bytes()
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
        let a = login_message(&Fingerprint([1; 32]), "n");
        let b = login_message(&Fingerprint([2; 32]), "n");
        assert!(a.starts_with(b"ourcamp-login-v1\n"));
        assert_ne!(a, b);
    }

    #[test]
    fn signature_verifies_only_for_its_message_and_key() {
        let k = key();
        let msg = login_message(&Fingerprint([1; 32]), "nonce");
        let sig = k.sign(&msg);
        assert!(verify(&public(&k), &msg, sig.as_ref()));
        let other_camp = login_message(&Fingerprint([2; 32]), "nonce");
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
            }],
            presence: vec![ChannelPresence {
                channel_id: "c0".into(),
                members: vec![PresentMember {
                    user_id: "u".into(),
                    mic_muted: true,
                }],
            }],
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "state");
        assert_eq!(json["members"][0]["role"], "moderator");
        assert_eq!(json["members"][0]["silenced"], true);
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
        assert_eq!(serde_json::from_value::<CampEvent>(json).unwrap(), event);
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
