//! Chats (M5, ADR-0044): messages of text channels and the photos in them.
//!
//! Each chat numbers its messages 1, 2, 3… (`seq`), so an app knows exactly how many it
//! has not read and which ones it missed while away. Photos are kept by the hash of
//! their bytes; one uploaded but never sent is dropped after [`ORPHAN_AFTER_MS`].

use std::collections::BTreeMap;

use oc_proto::{ChatMessage, Photo, PhotoFormat};
use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};

use super::{Store, StoreResult};

/// How long an uploaded photo waits for the message that names it.
const ORPHAN_AFTER_MS: i64 = 60 * 60 * 1000;

/// What [`Store::post_message`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Posted {
    /// Kept, with its number in the chat.
    Message(ChatMessage),
    /// No text channel with that id.
    NoChat,
    /// The photo it names was never uploaded (or was dropped as an orphan).
    NoPhoto,
}

/// Who writes: the author's id and their name at the time.
#[derive(Debug, Clone, Copy)]
pub struct Author<'a> {
    /// See [`oc_proto::user_id`].
    pub user_id: &'a str,
    /// Their name now; it stays with the message.
    pub name: &'a str,
}

impl Store {
    /// Chats (schema v7). Messages are numbered within their chat and go with it; they
    /// outlive their author (no key on members), whose name they keep. Photos are kept
    /// by hash, for as long as a message names them.
    pub(super) fn migrate_v7(&mut self) -> StoreResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE photos (
                 hash        TEXT PRIMARY KEY,
                 mime        TEXT NOT NULL,
                 data        BLOB NOT NULL,
                 uploaded_at INTEGER NOT NULL
             );
             CREATE TABLE messages (
                 channel_id   TEXT NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
                 seq          INTEGER NOT NULL,
                 author_id    TEXT NOT NULL,
                 author_name  TEXT NOT NULL,
                 sent_at      INTEGER NOT NULL,
                 text         TEXT NOT NULL,
                 photo_hash   TEXT REFERENCES photos (hash),
                 photo_width  INTEGER,
                 photo_height INTEGER,
                 PRIMARY KEY (channel_id, seq)
             ) WITHOUT ROWID;
             CREATE INDEX messages_photo ON messages (photo_hash) WHERE photo_hash IS NOT NULL;",
        )?;
        tx.pragma_update(None, "user_version", 7)?;
        tx.commit()?;
        Ok(())
    }

    /// Keeps a message in a chat as its next one. `text` and `photo` were checked by the
    /// caller (length, sizes).
    ///
    /// # Errors
    /// On database failure.
    pub fn post_message(
        &mut self,
        channel_id: &str,
        author: Author<'_>,
        text: &str,
        photo: Option<&Photo>,
    ) -> StoreResult<Posted> {
        // IMMEDIATE: the next `seq` is read and written under one write lock.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let is_chat = tx
            .query_row(
                "SELECT 1 FROM channels WHERE id = ?1 AND kind = 'text'",
                [channel_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !is_chat {
            return Ok(Posted::NoChat);
        }
        if let Some(photo) = photo {
            let uploaded = tx
                .query_row(
                    "SELECT 1 FROM photos WHERE hash = ?1",
                    [&photo.hash],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !uploaded {
                return Ok(Posted::NoPhoto);
            }
        }
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE channel_id = ?1",
            [channel_id],
            |r| r.get(0),
        )?;
        let message = ChatMessage {
            channel_id: channel_id.to_owned(),
            seq: from_sql(seq),
            author_id: author.user_id.to_owned(),
            author_name: author.name.to_owned(),
            sent_at: now_ms(),
            text: text.to_owned(),
            photo: photo.cloned(),
        };
        tx.execute(
            "INSERT INTO messages (channel_id, seq, author_id, author_name, sent_at, text,
                                   photo_hash, photo_width, photo_height)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                message.channel_id,
                seq,
                message.author_id,
                message.author_name,
                message.sent_at,
                message.text,
                photo.map(|p| &p.hash),
                photo.map(|p| p.width),
                photo.map(|p| p.height),
            ],
        )?;
        tx.commit()?;
        Ok(Posted::Message(message))
    }

    /// Up to `limit` messages of a chat with `seq` below `before` (all, when `None`),
    /// oldest first. `None`: no text channel with that id.
    ///
    /// # Errors
    /// On database failure.
    pub fn history(
        &self,
        channel_id: &str,
        before: Option<u64>,
        limit: u32,
    ) -> StoreResult<Option<Vec<ChatMessage>>> {
        let is_chat = self
            .conn
            .query_row(
                "SELECT 1 FROM channels WHERE id = ?1 AND kind = 'text'",
                [channel_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !is_chat {
            return Ok(None);
        }
        // The newest `limit` below `before`, then turned oldest first.
        let mut stmt = self.conn.prepare(
            "SELECT channel_id, seq, author_id, author_name, sent_at, text,
                    photo_hash, photo_width, photo_height
             FROM messages
             WHERE channel_id = ?1 AND seq < ?2
             ORDER BY seq DESC LIMIT ?3",
        )?;
        let before = before.map_or(i64::MAX, to_sql);
        let mut messages = stmt
            .query_map(params![channel_id, before, limit], message)?
            .collect::<Result<Vec<_>, _>>()?;
        messages.reverse();
        Ok(Some(messages))
    }

    /// Chat id → the `seq` of its last message, for chats with any.
    ///
    /// # Errors
    /// On database failure.
    pub fn latest_messages(&self) -> StoreResult<BTreeMap<String, u64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT channel_id, MAX(seq) FROM messages GROUP BY channel_id")?;
        let latest = stmt
            .query_map([], |r| Ok((r.get(0)?, from_sql(r.get(1)?))))?
            .collect::<Result<_, _>>()?;
        Ok(latest)
    }

    /// Keeps a photo by its hash and returns the hash; the same bytes twice are kept
    /// once. Also drops photos uploaded long ago that no message names. `bytes` were
    /// checked by the caller ([`PhotoFormat::sniff`], size).
    ///
    /// # Errors
    /// On database failure.
    pub fn put_photo(&self, format: PhotoFormat, bytes: &[u8]) -> StoreResult<String> {
        let now = now_ms();
        self.conn.execute(
            "DELETE FROM photos WHERE uploaded_at < ?1
                 AND NOT EXISTS (SELECT 1 FROM messages WHERE photo_hash = photos.hash)",
            [now - ORPHAN_AFTER_MS],
        )?;
        let hash = oc_proto::avatar_hash(bytes);
        // A photo uploaded again is fresh again: its message is about to come.
        self.conn.execute(
            "INSERT INTO photos (hash, mime, data, uploaded_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (hash) DO UPDATE SET uploaded_at = excluded.uploaded_at",
            params![hash, format.mime(), bytes, now],
        )?;
        Ok(hash)
    }

    /// A photo's media type and bytes.
    ///
    /// # Errors
    /// On database failure.
    pub fn photo(&self, hash: &str) -> StoreResult<Option<(String, Vec<u8>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT mime, data FROM photos WHERE hash = ?1",
                [hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }
}

fn message(r: &Row<'_>) -> rusqlite::Result<ChatMessage> {
    let hash: Option<String> = r.get(6)?;
    Ok(ChatMessage {
        channel_id: r.get(0)?,
        seq: from_sql(r.get(1)?),
        author_id: r.get(2)?,
        author_name: r.get(3)?,
        sent_at: r.get(4)?,
        text: r.get(5)?,
        photo: match hash {
            Some(hash) => Some(Photo {
                hash,
                width: r.get(7)?,
                height: r.get(8)?,
            }),
            None => None,
        },
    })
}

/// SQLite integers are signed; a `seq` starts at 1 and never nears `i64::MAX`.
fn from_sql(seq: i64) -> u64 {
    u64::try_from(seq).unwrap_or(0)
}

fn to_sql(seq: u64) -> i64 {
    i64::try_from(seq).unwrap_or(i64::MAX)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ChannelKind, Device};

    const LENA: Author<'static> = Author {
        user_id: "lena",
        name: "Лена",
    };

    fn camp() -> Store {
        Store::open_in_memory("Кооп").unwrap()
    }

    fn post(store: &mut Store, chat: &str, text: &str) -> ChatMessage {
        match store.post_message(chat, LENA, text, None).unwrap() {
            Posted::Message(m) => m,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn each_chat_numbers_its_own_messages() {
        let mut store = camp();
        let other = store.create_channel("Флуд", ChannelKind::Text).unwrap();
        assert_eq!(post(&mut store, "c1", "Привет").seq, 1);
        assert_eq!(post(&mut store, "c1", "Как дела").seq, 2);
        assert_eq!(post(&mut store, &other.id, "Тут тоже").seq, 1);
        assert_eq!(
            store.latest_messages().unwrap(),
            BTreeMap::from([("c1".to_owned(), 2), (other.id, 1)])
        );
    }

    #[test]
    fn only_text_channels_are_chats() {
        let mut store = camp();
        assert_eq!(
            store.post_message("c0", LENA, "в голосовой", None).unwrap(),
            Posted::NoChat,
            "c0 is Voice"
        );
        assert_eq!(
            store.post_message("nope", LENA, "x", None).unwrap(),
            Posted::NoChat
        );
        assert_eq!(store.history("c0", None, 50).unwrap(), None);
    }

    #[test]
    fn history_pages_back_oldest_first() {
        let mut store = camp();
        for i in 1..=5 {
            post(&mut store, "c1", &format!("№{i}"));
        }
        let texts = |page: Vec<ChatMessage>| page.into_iter().map(|m| m.text).collect::<Vec<_>>();
        let last = store.history("c1", None, 2).unwrap().unwrap();
        assert_eq!(texts(last), ["№4", "№5"]);
        let before = store.history("c1", Some(4), 2).unwrap().unwrap();
        assert_eq!(texts(before), ["№2", "№3"]);
        assert_eq!(
            texts(store.history("c1", Some(2), 50).unwrap().unwrap()),
            ["№1"]
        );
        assert_eq!(store.history("c1", Some(1), 50).unwrap().unwrap(), []);
    }

    #[test]
    fn a_photo_is_kept_once_and_must_be_uploaded_first() {
        let mut store = camp();
        let bytes = b"\xFF\xD8\xFF\xE0 a jpeg";
        let hash = store.put_photo(PhotoFormat::Jpeg, bytes).unwrap();
        assert_eq!(store.put_photo(PhotoFormat::Jpeg, bytes).unwrap(), hash);
        assert_eq!(
            store.photo(&hash).unwrap(),
            Some(("image/jpeg".to_owned(), bytes.to_vec()))
        );

        let photo = Photo {
            hash,
            width: 1600,
            height: 900,
        };
        let Posted::Message(sent) = store.post_message("c1", LENA, "", Some(&photo)).unwrap()
        else {
            panic!("posted")
        };
        assert_eq!(sent.photo.as_ref(), Some(&photo));
        assert_eq!(store.history("c1", None, 1).unwrap().unwrap(), [sent]);

        let never = Photo {
            hash: "never-uploaded00".into(),
            ..photo
        };
        assert_eq!(
            store.post_message("c1", LENA, "", Some(&never)).unwrap(),
            Posted::NoPhoto
        );
    }

    #[test]
    fn orphans_go_but_photos_in_messages_stay() {
        let mut store = camp();
        let sent = store
            .put_photo(PhotoFormat::Png, b"\x89PNG\r\n\x1a\nsent")
            .unwrap();
        let orphan = store
            .put_photo(PhotoFormat::Png, b"\x89PNG\r\n\x1a\norphan")
            .unwrap();
        let photo = Photo {
            hash: sent.clone(),
            width: 10,
            height: 10,
        };
        assert!(matches!(
            store.post_message("c1", LENA, "", Some(&photo)).unwrap(),
            Posted::Message(_)
        ));
        // Both uploaded long ago; the next upload sweeps.
        store
            .conn
            .execute("UPDATE photos SET uploaded_at = 0", [])
            .unwrap();
        store
            .put_photo(PhotoFormat::Png, b"\x89PNG\r\n\x1a\nnew")
            .unwrap();
        assert!(store.photo(&sent).unwrap().is_some(), "named by a message");
        assert!(store.photo(&orphan).unwrap().is_none(), "never sent");
    }

    #[test]
    fn messages_outlive_their_author_and_go_with_their_chat() {
        let mut store = camp();
        let device = Device {
            user_id: "lena",
            public_key: &[7; 32],
            name: "Лена",
        };
        let token = store.join_token().unwrap();
        store.login(device, Some(token.as_str())).unwrap().unwrap();
        post(&mut store, "c1", "Пока всем");
        assert!(store.remove_member("lena").unwrap());
        let kept = store.history("c1", None, 50).unwrap().unwrap();
        assert_eq!(
            kept[0].author_name, "Лена",
            "the name stays with the message"
        );

        store.delete_channel("c1").unwrap();
        assert!(store.latest_messages().unwrap().is_empty());
    }
}
