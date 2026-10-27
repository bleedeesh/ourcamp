//! Where room access comes from until the camp server exists (ADR-0010).
//!
//! Debug builds sign their own token with the well-known key of `livekit-server --dev`.
//! Release builds have no way in until M3, when the camp API issues tokens.

/// Server address and access token for one room.
pub struct RoomAccess {
    /// `ws://` / `wss://` address.
    pub url: String,
    /// Signed access token.
    pub token: String,
}

/// LiveKit room for a channel: unique across camps on the same server.
#[must_use]
pub fn room_name(camp_id: &str, channel_id: &str) -> String {
    format!("{camp_id}/{channel_id}")
}

/// Identity of the local user. Until accounts exist (M4) it comes from `OC_IDENTITY`,
/// so two clients on one machine can be different people.
#[must_use]
pub fn local_identity() -> String {
    std::env::var("OC_IDENTITY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "dima".into())
}

#[cfg(debug_assertions)]
mod dev {
    use std::time::Duration;

    use livekit_token::{AccessToken, VideoGrants};

    /// Key pair `livekit-server --dev` starts with. Public knowledge, local use only.
    const DEV_KEY: &str = "devkey";
    const DEV_SECRET: &str = "secret";
    const DEFAULT_URL: &str = "ws://127.0.0.1:7880";

    pub fn access(room: &str, identity: &str) -> Result<super::RoomAccess, String> {
        let url = std::env::var("OC_LIVEKIT_URL").unwrap_or_else(|_| DEFAULT_URL.into());
        let token = AccessToken::with_api_key(DEV_KEY, DEV_SECRET)
            .with_identity(identity)
            .with_name(identity)
            .with_ttl(Duration::from_secs(12 * 60 * 60))
            .with_grants(VideoGrants {
                room_join: true,
                room: room.into(),
                can_publish: Some(true),
                can_subscribe: Some(true),
                ..VideoGrants::default()
            })
            .to_jwt()
            .map_err(|e| format!("Не удалось подписать токен комнаты: {e}"))?;
        Ok(super::RoomAccess { url, token })
    }
}

/// Address and token for `room` as `identity`.
///
/// # Errors
/// In release builds always (no camp server yet); in debug builds if signing fails.
pub fn access(room: &str, identity: &str) -> Result<RoomAccess, String> {
    #[cfg(debug_assertions)]
    {
        dev::access(room, identity)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (room, identity);
        Err("Голосовые каналы заработают с сервером лагеря (M3). Пока — только сборка для разработки.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_name_scopes_channel_by_camp() {
        assert_eq!(room_name("vk", "lobby"), "vk/lobby");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dev_token_is_a_signed_jwt_for_the_room() {
        let access = access("vk/lobby", "artem").unwrap();
        let parts: Vec<&str> = access.token.split('.').collect();
        assert_eq!(parts.len(), 3, "header.payload.signature");
        let claims = livekit_token::TokenVerifier::with_api_key("devkey", "secret")
            .verify(&access.token)
            .unwrap();
        assert_eq!(claims.sub, "artem");
        assert_eq!(claims.video.room, "vk/lobby");
        assert!(claims.video.room_join);
    }
}
