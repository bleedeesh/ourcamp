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

/// A voice channel ("костёр").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// Stable id; also the LiveKit room name.
    pub id: String,
    /// Display name.
    pub name: String,
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
    /// Invite or claim token: required on first login, optional later (a member
    /// can still redeem the claim key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
}

/// Camp role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Regular member.
    Member,
    /// Redeemed the claim key.
    Admin,
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
/// frames tagged by `type`. The server sends `state` on connect and on every change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CampEvent {
    /// Full snapshot: a missed message can never leave a client out of sync.
    State(CampState),
}

/// Who is in the camp and who sits at which campfire.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CampState {
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
    /// Redeemed the admin key.
    pub is_admin: bool,
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
    /// Invite unknown.
    InviteInvalid,
    /// Admin key already redeemed.
    ClaimUsed,
    /// Signature does not verify.
    BadSignature,
    /// Nonce unknown, expired or reused.
    NonceInvalid,
    /// Missing or expired session.
    Unauthorized,
    /// No such resource.
    NotFound,
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
            members: vec![MemberInfo {
                user_id: "u".into(),
                name: "Лена".into(),
                is_admin: false,
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
        assert_eq!(json["members"][0]["isAdmin"], false);
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
