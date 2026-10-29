//! Chats of the open camp (M5, ADR-0044): history, writing, photos, and what this device
//! has read — a `seq` per chat in `camps.json`, so the count of unread survives restarts.
//!
//! New messages reach the UI as [`ChatUpdate::Message`] from the camp's event stream;
//! marking a chat read is [`ChatUpdate::Read`], so every window shows the same count.

use std::collections::BTreeMap;
use std::fs;

use oc_camp_client::{CampClient, ClientError, Session};
use oc_proto::{avatar_hash, ChannelKind, ChatMessage, ErrorCode, NewMessage, Photo, PhotoFormat};
use serde::Serialize;

use super::{describe, CampError, Camps, SavedCamp};

/// Photos of chats, by content hash: fetched once, kept across restarts.
const PHOTO_CACHE: &str = "photos";

/// What changed in the open camp's chats, as sent to every window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ChatUpdate {
    /// Someone wrote (you too).
    Message {
        camp_id: String,
        message: ChatMessage,
    },
    /// This device has read the chat up to `seq`.
    Read {
        camp_id: String,
        channel_id: String,
        seq: u64,
    },
}

/// Receives [`ChatUpdate`]s.
pub type ChatListener = std::sync::Arc<dyn Fn(ChatUpdate) + Send + Sync>;

/// The open camp as the chat window needs it when it opens (ADR-0046): what the main
/// window got from opening it, and the latest snapshot (members, chats, who is silenced).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatContext {
    /// As opened, with what this device has read.
    pub camp: super::OpenCamp,
    /// The latest live update about it; `None` until the first one.
    pub live: Option<super::CampLive>,
}

/// A photo to send, as the window made it (at most 1600 px on its long side).
#[derive(Debug, Clone)]
pub struct OutgoingPhoto {
    /// The encoded picture: WebP, PNG or JPEG.
    pub bytes: Vec<u8>,
    /// Pixels.
    pub width: u32,
    /// Pixels.
    pub height: u32,
}

/// The hash names a file: it must be what `avatar_hash` makes, never a path.
fn valid_hash(hash: &str) -> bool {
    hash.len() == 16
        && hash
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Camps {
    /// Up to `limit` messages of a chat before `before` (up to the latest when `None`),
    /// oldest first.
    ///
    /// # Errors
    /// No such chat, unreachable camp.
    pub async fn history(
        &self,
        id: &str,
        channel_id: &str,
        before: Option<u64>,
        limit: u32,
    ) -> Result<Vec<ChatMessage>, CampError> {
        let camp = self.camp(id)?;
        let client = camp.client()?;
        match client
            .history(&camp.session(), channel_id, before, limit)
            .await
        {
            Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                let session = self.relogin(&camp, &client).await?;
                client.history(&session, channel_id, before, limit).await
            }
            other => other,
        }
        .map_err(chat_error)
    }

    /// Writes in a chat: the photo goes up first. The message also comes back as a
    /// [`ChatUpdate::Message`], like everyone else's.
    ///
    /// # Errors
    /// Forbidden to speak, too long, unreachable camp.
    pub async fn send(
        &self,
        id: &str,
        channel_id: &str,
        text: String,
        photo: Option<OutgoingPhoto>,
    ) -> Result<ChatMessage, CampError> {
        let camp = self.camp(id)?;
        let client = camp.client()?;
        // A stale session is renewed once, on the first call that hits it.
        let mut session = camp.session();
        let photo = match photo {
            None => None,
            Some(photo) => {
                if PhotoFormat::sniff(&photo.bytes).is_none() {
                    return Err("Это не фото: подойдёт JPEG, PNG или WebP".into());
                }
                let hash = match client.upload_photo(&session, photo.bytes.clone()).await {
                    Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                        session = self.relogin(&camp, &client).await?;
                        client.upload_photo(&session, photo.bytes).await
                    }
                    other => other,
                }
                .map_err(chat_error)?;
                Some(Photo {
                    hash,
                    width: photo.width,
                    height: photo.height,
                })
            }
        };
        let message = NewMessage { text, photo };
        match client.post_message(&session, channel_id, &message).await {
            Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                let session = self.relogin(&camp, &client).await?;
                client.post_message(&session, channel_id, &message).await
            }
            other => other,
        }
        .map_err(chat_error)
    }

    /// A photo by its hash, from the cache or the camp.
    ///
    /// # Errors
    /// A malformed hash, no such photo, unreachable camp.
    pub async fn photo(&self, id: &str, hash: &str) -> Result<Vec<u8>, CampError> {
        if !valid_hash(hash) {
            return Err("Неверное имя фото".into());
        }
        let cached = self.dir.join(PHOTO_CACHE).join(hash);
        if let Ok(bytes) = fs::read(&cached) {
            if avatar_hash(&bytes) == hash {
                return Ok(bytes);
            }
        }
        let camp = self.camp(id)?;
        let client = camp.client()?;
        let bytes = match client.photo(&camp.session(), hash).await {
            Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                let session = self.relogin(&camp, &client).await?;
                client.photo(&session, hash).await
            }
            other => other,
        }
        .map_err(chat_error)?;
        // The camp names a photo by its bytes: other bytes are not that photo.
        if avatar_hash(&bytes) != hash || PhotoFormat::sniff(&bytes).is_none() {
            return Err("Сервер прислал не то фото".into());
        }
        if let Err(e) =
            fs::create_dir_all(self.dir.join(PHOTO_CACHE)).and_then(|()| fs::write(&cached, &bytes))
        {
            eprintln!("[ourcamp] photo cache: {e}");
        }
        Ok(bytes)
    }

    /// The open camp for a window that opens now; `None` with no camp open.
    #[must_use]
    pub fn chat_context(&self) -> Option<ChatContext> {
        let id = self.lock().current.clone()?;
        let camp = self.camp(&id).ok()?;
        let live = self
            .last_live
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .filter(|live| live.camp_id == id);
        let offline = live.as_ref().is_some_and(|live| !live.online);
        Some(ChatContext {
            camp: self.opened(&camp, offline),
            live,
        })
    }

    /// Marks a chat read up to `seq` (never back) and tells every window.
    ///
    /// # Errors
    /// Unknown camp, or the list cannot be saved.
    pub fn mark_read(&self, id: &str, channel_id: &str, seq: u64) -> Result<(), CampError> {
        let mut moved = false;
        let mut known = false;
        self.update(|saved| {
            if let Some(camp) = saved.camps.iter_mut().find(|c| c.id == id) {
                known = true;
                let read = camp.read.entry(channel_id.to_owned()).or_default();
                if seq > *read {
                    *read = seq;
                    moved = true;
                }
            }
        })?;
        if !known {
            return Err("Этот сервер больше не в списке".into());
        }
        if moved {
            self.chat_update(ChatUpdate::Read {
                camp_id: id.to_owned(),
                channel_id: channel_id.to_owned(),
                seq,
            });
        }
        Ok(())
    }

    /// Where this device starts reading a camp it just joined: at each chat's latest
    /// message, so the history from before is not "unread" (ADR-0044). Chats made later
    /// start at nothing read.
    pub(super) async fn read_up_to_now(
        client: &CampClient,
        session: &Session,
        camp: &SavedCamp,
    ) -> BTreeMap<String, u64> {
        let mut read = BTreeMap::new();
        for chat in camp.channels.iter().filter(|c| c.kind == ChannelKind::Text) {
            match client.history(session, &chat.id, None, 1).await {
                Ok(last) => {
                    if let Some(message) = last.last() {
                        read.insert(chat.id.clone(), message.seq);
                    }
                }
                // Without it the chat shows its history as unread: worth a line, not a
                // failed join.
                Err(e) => eprintln!("[ourcamp] chat {}: {}", chat.name, describe(e)),
            }
        }
        read
    }

    /// Sets where the chat updates go (the app forwards them to every window).
    pub fn set_chat_listener(&self, listener: ChatListener) {
        *self
            .chats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(listener);
    }

    pub(super) fn chat_update(&self, update: ChatUpdate) {
        let listener = self
            .chats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(listener) = listener {
            listener(update);
        }
    }
}

/// A chat refusal as a sentence for the UI.
fn chat_error(e: ClientError) -> CampError {
    match e {
        ClientError::Api(ErrorCode::NotFound) => "Этого чата на сервере уже нет".into(),
        ClientError::Api(ErrorCode::BadRequest) => {
            "Сервер не принял сообщение: до 2000 символов, фото — до 2 МБ".into()
        }
        other => describe(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photo_names_are_hashes_never_paths() {
        assert!(valid_hash("AbCdEf0123456789"));
        assert!(valid_hash("-_-_-_-_-_-_-_-_"));
        assert!(!valid_hash("../../etc/passwd"));
        assert!(!valid_hash("short"));
        assert!(!valid_hash("AbCdEf012345678/"));
    }

    #[test]
    fn chat_updates_are_tagged_for_the_windows() {
        let read = ChatUpdate::Read {
            camp_id: "k".into(),
            channel_id: "c1".into(),
            seq: 4,
        };
        assert_eq!(
            serde_json::to_value(read).unwrap(),
            serde_json::json!({"type": "read", "campId": "k", "channelId": "c1", "seq": 4})
        );
    }
}
