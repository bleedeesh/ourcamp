//! Everything the camp remembers, in one SQLite file next to the server.
//!
//! The schema is versioned with `PRAGMA user_version`; each migration runs once, in a
//! transaction. Calls are synchronous and quick — [`crate::api`] runs them
//! off the async workers.

use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use oc_invite::{InviteKind, Token, CLAIM_TOKEN_BYTES, JOIN_TOKEN_BYTES};
use rusqlite::{params, Connection, OptionalExtension};

/// Channels a new camp starts with. The admin edits them later (M3 window / M5).
const DEFAULT_CHANNELS: [&str; 3] = ["Лобби", "Катка", "Отошёл (AFK)"];

/// Storage failure (disk, corruption). Not something the user can fix from a request.
#[derive(Debug)]
pub struct StoreError(rusqlite::Error);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "camp database: {}", self.0)
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self(e)
    }
}

/// Result of store operations.
pub type StoreResult<T> = Result<T, StoreError>;

/// A voice channel ("костёр").
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Channel {
    /// Stable id, used for the LiveKit room name.
    pub id: String,
    /// Display name.
    pub name: String,
}

/// Public description of the camp.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Camp {
    /// Display name.
    pub name: String,
    /// Channels in display order.
    pub channels: Vec<Channel>,
}

/// What a token is, as far as the server knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InviteStatus {
    /// Join or admin claim.
    pub kind: InviteKind,
    /// Whether redeeming it would work now (claims are single-use).
    pub usable: bool,
}

/// Why a claim key could not be redeemed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimError {
    /// No claim key with this value.
    Unknown,
    /// Somebody already redeemed it.
    AlreadyUsed,
}

/// The camp database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating and migrating if needed) the database at `path`. A brand-new
    /// camp gets `camp_name`, the default channels, an invite and a claim key.
    ///
    /// # Errors
    /// If the file cannot be opened or migrated.
    pub fn open(path: &Path, camp_name: &str) -> StoreResult<Self> {
        Self::init(Connection::open(path)?, camp_name)
    }

    /// In-memory database (tests).
    ///
    /// # Errors
    /// Practically never.
    pub fn open_in_memory(camp_name: &str) -> StoreResult<Self> {
        Self::init(Connection::open_in_memory()?, camp_name)
    }

    fn init(conn: Connection, camp_name: &str) -> StoreResult<Self> {
        // WAL: readers do not block the writer; the default for a server.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let mut store = Self { conn };
        store.migrate(camp_name)?;
        Ok(store)
    }

    fn migrate(&mut self, camp_name: &str) -> StoreResult<()> {
        let version: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < 1 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(
                "CREATE TABLE meta (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                 );
                 CREATE TABLE channels (
                    id       TEXT PRIMARY KEY,
                    name     TEXT NOT NULL,
                    position INTEGER NOT NULL
                 );
                 CREATE TABLE invites (
                    token       TEXT PRIMARY KEY,
                    kind        TEXT NOT NULL CHECK (kind IN ('join', 'claim')),
                    created_at  INTEGER NOT NULL,
                    redeemed_at INTEGER
                 );",
            )?;
            tx.execute(
                "INSERT INTO meta (key, value) VALUES ('camp_name', ?1)",
                [camp_name],
            )?;
            for (position, name) in (0_i64..).zip(DEFAULT_CHANNELS) {
                tx.execute(
                    "INSERT INTO channels (id, name, position) VALUES (?1, ?2, ?3)",
                    params![channel_id(position), name, position],
                )?;
            }
            insert_invite(&tx, InviteKind::Join)?;
            insert_invite(&tx, InviteKind::ClaimAdmin)?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Name and channels.
    ///
    /// # Errors
    /// On database failure.
    pub fn camp(&self) -> StoreResult<Camp> {
        let name =
            self.conn
                .query_row("SELECT value FROM meta WHERE key = 'camp_name'", [], |r| {
                    r.get(0)
                })?;
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM channels ORDER BY position")?;
        let channels = stmt
            .query_map([], |r| {
                Ok(Channel {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(Camp { name, channels })
    }

    /// The current friends invite.
    ///
    /// # Errors
    /// On database failure.
    pub fn join_token(&self) -> StoreResult<Token> {
        let token: String = self.conn.query_row(
            "SELECT token FROM invites WHERE kind = 'join' ORDER BY created_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )?;
        stored_token(&token, InviteKind::Join)
    }

    /// The admin claim key, until someone redeems it.
    ///
    /// # Errors
    /// On database failure.
    pub fn unclaimed_admin_token(&self) -> StoreResult<Option<Token>> {
        let token: Option<String> = self
            .conn
            .query_row(
                "SELECT token FROM invites WHERE kind = 'claim' AND redeemed_at IS NULL",
                [],
                |r| r.get(0),
            )
            .optional()?;
        token
            .map(|t| stored_token(&t, InviteKind::ClaimAdmin))
            .transpose()
    }

    /// What `token` is, or `None` if the server never issued it.
    ///
    /// # Errors
    /// On database failure.
    pub fn invite_status(&self, token: &str) -> StoreResult<Option<InviteStatus>> {
        let row: Option<(String, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT kind, redeemed_at FROM invites WHERE token = ?1",
                [token],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(kind, redeemed_at)| match kind.as_str() {
            "claim" => InviteStatus {
                kind: InviteKind::ClaimAdmin,
                usable: redeemed_at.is_none(),
            },
            _ => InviteStatus {
                kind: InviteKind::Join,
                usable: true,
            },
        }))
    }

    /// Redeems the admin claim key — exactly once, even under concurrent requests (the
    /// check and the update are one statement).
    ///
    /// # Errors
    /// `Ok(Err(_))` when the key is unknown or used; `Err` on database failure.
    pub fn redeem_claim(&self, token: &str) -> StoreResult<Result<(), ClaimError>> {
        let changed = self.conn.execute(
            "UPDATE invites SET redeemed_at = ?2
             WHERE token = ?1 AND kind = 'claim' AND redeemed_at IS NULL",
            params![token, now()],
        )?;
        if changed == 1 {
            return Ok(Ok(()));
        }
        Ok(Err(match self.invite_status(token)? {
            Some(InviteStatus {
                kind: InviteKind::ClaimAdmin,
                ..
            }) => ClaimError::AlreadyUsed,
            _ => ClaimError::Unknown,
        }))
    }
}

fn channel_id(position: i64) -> String {
    format!("c{position}")
}

fn insert_invite(conn: &Connection, kind: InviteKind) -> StoreResult<()> {
    let (len, kind_str) = match kind {
        InviteKind::Join => (JOIN_TOKEN_BYTES, "join"),
        InviteKind::ClaimAdmin => (CLAIM_TOKEN_BYTES, "claim"),
    };
    let mut bytes = vec![0u8; len];
    // The OS CSPRNG failing is unrecoverable for a server that issues secrets.
    getrandom::fill(&mut bytes).expect("operating system random number generator");
    conn.execute(
        "INSERT INTO invites (token, kind, created_at) VALUES (?1, ?2, ?3)",
        params![Token::from_bytes(&bytes).as_str(), kind_str, now()],
    )?;
    Ok(())
}

/// Reads back a token the store wrote. Only a corrupted database fails here.
fn stored_token(s: &str, kind: InviteKind) -> StoreResult<Token> {
    Token::parse(s, kind).map_err(|e| {
        StoreError(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(e),
        ))
    })
}

/// Canonical form of a token a client sent, trying both kinds (they differ in length).
#[must_use]
pub fn canonical_token(s: &str) -> Option<Token> {
    Token::parse(s, InviteKind::Join)
        .or_else(|_| Token::parse(s, InviteKind::ClaimAdmin))
        .ok()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_camp_has_name_default_channels_and_both_links() {
        let store = Store::open_in_memory("Вечерний кооп").unwrap();
        let camp = store.camp().unwrap();
        assert_eq!(camp.name, "Вечерний кооп");
        let names: Vec<_> = camp.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, DEFAULT_CHANNELS);
        assert_eq!(store.join_token().unwrap().as_str().len(), 16);
        assert_eq!(
            store
                .unclaimed_admin_token()
                .unwrap()
                .unwrap()
                .as_str()
                .len(),
            26
        );
    }

    #[test]
    fn reopening_keeps_everything_and_the_first_name() {
        let dir = std::env::temp_dir().join(format!("oc-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("camp.db");
        let _ = std::fs::remove_file(&path);
        let (join, claim) = {
            let s = Store::open(&path, "Первое имя").unwrap();
            (s.join_token().unwrap(), s.unclaimed_admin_token().unwrap())
        };
        let s = Store::open(&path, "Другое имя").unwrap();
        assert_eq!(s.camp().unwrap().name, "Первое имя", "name is set once");
        assert_eq!(s.join_token().unwrap(), join, "links survive restarts");
        assert_eq!(s.unclaimed_admin_token().unwrap(), claim);
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claim_key_works_exactly_once() {
        let store = Store::open_in_memory("x").unwrap();
        let claim = store.unclaimed_admin_token().unwrap().unwrap();
        let status = store.invite_status(claim.as_str()).unwrap().unwrap();
        assert_eq!(status.kind, InviteKind::ClaimAdmin);
        assert!(status.usable);

        assert_eq!(store.redeem_claim(claim.as_str()).unwrap(), Ok(()));
        assert_eq!(
            store.redeem_claim(claim.as_str()).unwrap(),
            Err(ClaimError::AlreadyUsed)
        );
        assert!(!store.invite_status(claim.as_str()).unwrap().unwrap().usable);
        assert_eq!(
            store.unclaimed_admin_token().unwrap(),
            None,
            "link no longer shown"
        );
    }

    #[test]
    fn join_invite_cannot_be_used_as_a_claim_and_unknown_tokens_are_unknown() {
        let store = Store::open_in_memory("x").unwrap();
        let join = store.join_token().unwrap();
        assert_eq!(
            store.redeem_claim(join.as_str()).unwrap(),
            Err(ClaimError::Unknown)
        );
        let status = store.invite_status(join.as_str()).unwrap().unwrap();
        assert_eq!((status.kind, status.usable), (InviteKind::Join, true));
        assert_eq!(store.invite_status("nonexistent").unwrap(), None);
    }
}
