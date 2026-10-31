//! Everything the camp remembers, in one SQLite file next to the server.
//!
//! The schema is versioned with `PRAGMA user_version`; each migration runs once, in a
//! transaction. Calls are synchronous and quick — [`crate::api`] runs them
//! off the async workers.

use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use oc_invite::{InviteKind, Token, CLAIM_TOKEN_BYTES, JOIN_TOKEN_BYTES};
pub use oc_proto::{Channel, ChannelKind, Role};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

mod chat;
pub use chat::{Author, Deleted, Posted};

/// Channels of the very first schema (v1). New camps get [`NEW_CAMP_CHANNELS`] instead;
/// this stays as v1 created them, for migrations to recognise.
const DEFAULT_CHANNELS: [&str; 3] = ["Лобби", "Катка", "Отошёл (AFK)"];

/// What a brand-new camp starts with (ADR-0026): (id, name, kind, afk). The owner edits
/// them in the server window.
const NEW_CAMP_CHANNELS: [(&str, &str, &str, bool); 3] = [
    ("c0", "Voice", "voice", false),
    ("c1", "Chat", "text", false),
    ("afk", "AFK", "voice", true),
];

/// A brand-new camp: the current default channels replace whatever the migrations from
/// v1 laid down (they describe how old camps change, not how new ones begin).
fn seed_new_camp(conn: &Connection) -> StoreResult<()> {
    conn.execute("DELETE FROM channels", [])?;
    for (position, (id, name, kind, afk)) in (0_i64..).zip(NEW_CAMP_CHANNELS) {
        conn.execute(
            "INSERT INTO channels (id, name, position, kind, afk) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, position, kind, afk],
        )?;
    }
    Ok(())
}

/// Adds the AFK campfire at the end unless the camp has one (ADR-0025). `MAX()` sits in a
/// subquery: an aggregate over rows the `WHERE` filtered out still yields one row.
const ENSURE_AFK: &str = "INSERT INTO channels (id, name, position, afk)
     SELECT 'afk', 'AFK', (SELECT COALESCE(MAX(position), -1) + 1 FROM channels), 1
     WHERE NOT EXISTS (SELECT 1 FROM channels WHERE afk = 1)";

/// Storage failure (disk, corruption). Not something the user can fix from a request.
#[derive(Debug)]
pub enum StoreError {
    /// SQLite refused.
    Sqlite(rusqlite::Error),
    /// The data does not hold together (a migration found it so).
    Inconsistent(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "camp database: {e}"),
            Self::Inconsistent(why) => write!(f, "camp database: {why}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

/// Result of store operations.
pub type StoreResult<T> = Result<T, StoreError>;

/// Public description of the camp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Camp {
    /// Display name.
    pub name: String,
    /// Channels in display order.
    pub channels: Vec<Channel>,
    /// Anyone may come in without an invite (ADR-0040).
    pub open: bool,
}

/// What a token is, as far as the server knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InviteStatus {
    /// Join or admin claim.
    pub kind: InviteKind,
    /// Whether it lets a new device in now (admin keys never do since ADR-0026).
    pub usable: bool,
}

/// Why a login was refused (the signature is checked before the store is asked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginError {
    /// A new device came without an invite.
    InviteRequired,
    /// A new device came with a token the server never issued.
    InviteInvalid,
}

/// A camp member, as a session resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// See [`oc_proto::user_id`].
    pub user_id: String,
    /// Display name.
    pub name: String,
    /// Role in the camp.
    pub role: Role,
    /// Not allowed to speak (ADR-0028): room tokens come without `canPublish`.
    pub silenced: bool,
}

/// Who is logging in.
#[derive(Debug, Clone, Copy)]
pub struct Device<'a> {
    /// Derived from `public_key`.
    pub user_id: &'a str,
    /// Ed25519 public key.
    pub public_key: &'a [u8],
    /// Display name the user chose.
    pub name: &'a str,
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
        if version < 2 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(
                "CREATE TABLE members (
                    user_id    TEXT PRIMARY KEY,
                    public_key BLOB NOT NULL UNIQUE,
                    name       TEXT NOT NULL,
                    role       TEXT NOT NULL CHECK (role IN ('member', 'admin')),
                    joined_at  INTEGER NOT NULL
                 );
                 -- Only the SHA-256 of a session token is kept: unlike invites it never
                 -- has to be shown again (ADR-0018).
                 CREATE TABLE sessions (
                    token_hash BLOB PRIMARY KEY,
                    user_id    TEXT NOT NULL REFERENCES members (user_id) ON DELETE CASCADE,
                    expires_at INTEGER NOT NULL
                 );",
            )?;
            tx.pragma_update(None, "user_version", 2)?;
            tx.commit()?;
        }
        if version < 3 {
            // The system AFK campfire (ADR-0025). The third default campfire of v1,
            // "Отошёл (AFK)", becomes it (same id = same LiveKit room); a camp without
            // it gets a new one at the end.
            let tx = self.conn.transaction()?;
            tx.execute_batch(
                "ALTER TABLE channels ADD COLUMN afk INTEGER NOT NULL DEFAULT 0;
                 UPDATE channels SET afk = 1, name = 'AFK'
                  WHERE id = 'c2' AND name = 'Отошёл (AFK)';",
            )?;
            tx.execute(ENSURE_AFK, [])?;
            tx.pragma_update(None, "user_version", 3)?;
            tx.commit()?;
        }
        if version < 4 {
            self.migrate_v4()?;
        }
        if version < 5 {
            // Silenced members (ADR-0028): not allowed to speak until lifted.
            let tx = self.conn.transaction()?;
            tx.execute_batch(
                "ALTER TABLE members ADD COLUMN silenced INTEGER NOT NULL DEFAULT 0;",
            )?;
            tx.pragma_update(None, "user_version", 5)?;
            tx.commit()?;
        }
        if version < 6 {
            // Members' avatars (ADR-0035): a copy of what their app sent, kept apart so the
            // member list stays small. Leaving or a kick takes it along.
            let tx = self.conn.transaction()?;
            tx.execute_batch(
                "CREATE TABLE avatars (
                     user_id TEXT PRIMARY KEY REFERENCES members (user_id) ON DELETE CASCADE,
                     hash    TEXT NOT NULL,
                     mime    TEXT NOT NULL,
                     data    BLOB NOT NULL
                 );",
            )?;
            tx.pragma_update(None, "user_version", 6)?;
            tx.commit()?;
        }
        self.migrate_chats(version)?;
        if version == 0 {
            seed_new_camp(&self.conn)?;
        }
        Ok(())
    }

    /// Roles and channel kinds (ADR-0026). SQLite cannot change a CHECK in place: the
    /// members table is rebuilt (the documented 12-step way), with foreign keys off for
    /// the swap — they cannot be switched inside a transaction.
    fn migrate_v4(&mut self) -> StoreResult<()> {
        self.conn.pragma_update(None, "foreign_keys", false)?;
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE members_v4 (
                user_id    TEXT PRIMARY KEY,
                public_key BLOB NOT NULL UNIQUE,
                name       TEXT NOT NULL,
                role       TEXT NOT NULL CHECK (role IN ('member', 'moderator', 'admin')),
                joined_at  INTEGER NOT NULL
             );
             INSERT INTO members_v4 SELECT user_id, public_key, name, role, joined_at FROM members;
             DROP TABLE members;
             ALTER TABLE members_v4 RENAME TO members;
             ALTER TABLE channels ADD COLUMN kind TEXT NOT NULL DEFAULT 'voice'
                CHECK (kind IN ('voice', 'text'));
             -- The admin key is retired: whoever redeemed it stays admin.
             DELETE FROM invites WHERE kind = 'claim' AND redeemed_at IS NULL;",
        )?;
        let broken: Option<String> = tx
            .query_row("PRAGMA foreign_key_check", [], |r| r.get(0))
            .optional()?;
        if let Some(table) = broken {
            // Rolls back on drop; nothing changed.
            return Err(StoreError::Inconsistent(format!(
                "foreign keys of {table} do not hold after the v4 migration"
            )));
        }
        tx.pragma_update(None, "user_version", 4)?;
        tx.commit()?;
        self.conn.pragma_update(None, "foreign_keys", true)?;
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
            .prepare("SELECT id, name, kind, afk FROM channels ORDER BY position")?;
        let channels = stmt
            .query_map([], |r| {
                Ok(Channel {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: channel_kind(&r.get::<_, String>(2)?),
                    afk: r.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(Camp {
            name,
            channels,
            open: is_open(&self.conn)?,
        })
    }

    /// Opens the camp to anyone with its address, or back to invites only (ADR-0040).
    /// Kept in `meta`: no schema change, absent means closed.
    ///
    /// # Errors
    /// On database failure.
    pub fn set_open(&self, open: bool) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('open', ?1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [if open { "1" } else { "0" }],
        )?;
        Ok(())
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

    /// What `token` is, or `None` if the server never issued it. An admin key (retired by
    /// ADR-0026) is reported as such and never usable.
    ///
    /// # Errors
    /// On database failure.
    pub fn invite_status(&self, token: &str) -> StoreResult<Option<InviteStatus>> {
        let kind: Option<String> = self
            .conn
            .query_row("SELECT kind FROM invites WHERE token = ?1", [token], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(kind.map(|kind| match kind.as_str() {
            "join" => InviteStatus {
                kind: InviteKind::Join,
                usable: true,
            },
            _ => InviteStatus {
                kind: InviteKind::ClaimAdmin,
                usable: false,
            },
        }))
    }

    /// Admits a device whose signature the caller has verified (ADR-0018):
    ///
    /// * a known device is let in with its role (its name updated); no invite needed;
    /// * a new device needs the friends invite and joins as a member. Roles are given in
    ///   the server window (ADR-0026), never by a link.
    ///
    /// # Errors
    /// `Ok(Err(_))` when refused; `Err` on database failure.
    pub fn login(
        &mut self,
        device: Device<'_>,
        invite: Option<&str>,
    ) -> StoreResult<Result<Role, LoginError>> {
        // `BEGIN IMMEDIATE`: the check and the insert see the same state.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let known: Option<String> = tx
            .query_row(
                "SELECT role FROM members WHERE user_id = ?1",
                [device.user_id],
                |r| r.get(0),
            )
            .optional()?;
        let role = if let Some(role) = known {
            role_from(&role)
        } else {
            // An open camp lets anyone in (ADR-0040); an invite it may still carry is moot.
            if is_open(&tx)? {
                tx.execute(
                    "INSERT INTO members (user_id, public_key, name, role, joined_at)
                     VALUES (?1, ?2, ?3, 'member', ?4)",
                    params![device.user_id, device.public_key, device.name, now()],
                )?;
                tx.commit()?;
                return Ok(Ok(Role::Member));
            }
            let Some(token) = invite else {
                return Ok(Err(LoginError::InviteRequired));
            };
            let join: Option<()> = tx
                .query_row(
                    "SELECT 1 FROM invites WHERE token = ?1 AND kind = 'join'",
                    [token],
                    |_| Ok(()),
                )
                .optional()?;
            if join.is_none() {
                return Ok(Err(LoginError::InviteInvalid));
            }
            Role::Member
        };

        tx.execute(
            "INSERT INTO members (user_id, public_key, name, role, joined_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (user_id) DO UPDATE SET name = excluded.name",
            params![
                device.user_id,
                device.public_key,
                device.name,
                role_str(role),
                now()
            ],
        )?;
        tx.commit()?;
        Ok(Ok(role))
    }

    /// Stores a session for `user_id`, valid for `ttl_secs`.
    ///
    /// # Errors
    /// On database failure (or an unknown member).
    pub fn create_session(
        &self,
        token_hash: &[u8],
        user_id: &str,
        ttl_secs: i64,
    ) -> StoreResult<()> {
        // Expired sessions are swept whenever a new one is made: no background job.
        self.conn
            .execute("DELETE FROM sessions WHERE expires_at <= ?1", [now()])?;
        self.conn.execute(
            "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
            params![token_hash, user_id, now().saturating_add(ttl_secs)],
        )?;
        Ok(())
    }

    /// Forgets a member; their sessions go with them (`ON DELETE CASCADE`). Returns
    /// whether there was such a member.
    ///
    /// # Errors
    /// On database failure.
    pub fn remove_member(&self, user_id: &str) -> StoreResult<bool> {
        let removed = self
            .conn
            .execute("DELETE FROM members WHERE user_id = ?1", [user_id])?;
        Ok(removed > 0)
    }

    /// The member behind an unexpired session.
    ///
    /// # Errors
    /// On database failure.
    pub fn session_member(&self, token_hash: &[u8]) -> StoreResult<Option<Member>> {
        let row: Option<(String, String, String, bool)> = self
            .conn
            .query_row(
                "SELECT m.user_id, m.name, m.role, m.silenced
                 FROM sessions s JOIN members m ON m.user_id = s.user_id
                 WHERE s.token_hash = ?1 AND s.expires_at > ?2",
                params![token_hash, now()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        Ok(row.map(|(user_id, name, role, silenced)| Member {
            user_id,
            name,
            role: role_from(&role),
            silenced,
        }))
    }

    /// Every member, sorted by id.
    ///
    /// # Errors
    /// On database failure.
    pub fn members(&self) -> StoreResult<Vec<oc_proto::MemberInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.user_id, m.name, m.role, m.silenced, a.hash
             FROM members m LEFT JOIN avatars a ON a.user_id = m.user_id
             ORDER BY m.user_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(oc_proto::MemberInfo {
                user_id: r.get(0)?,
                name: r.get(1)?,
                role: role_from(&r.get::<_, String>(2)?),
                silenced: r.get(3)?,
                avatar: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Keeps `user_id`'s avatar (ADR-0035), replacing the one before. `bytes` were checked
    /// by the caller (`AvatarFormat::sniff`, size). `false`: no such member.
    ///
    /// # Errors
    /// On database failure.
    pub fn set_avatar(
        &self,
        user_id: &str,
        format: oc_proto::AvatarFormat,
        bytes: &[u8],
    ) -> StoreResult<bool> {
        let changed = self.conn.execute(
            "INSERT INTO avatars (user_id, hash, mime, data)
             SELECT user_id, ?2, ?3, ?4 FROM members WHERE user_id = ?1
             ON CONFLICT (user_id) DO UPDATE
                 SET hash = excluded.hash, mime = excluded.mime, data = excluded.data",
            params![user_id, oc_proto::avatar_hash(bytes), format.mime(), bytes],
        )?;
        Ok(changed > 0)
    }

    /// Drops `user_id`'s avatar; they show their initial again.
    ///
    /// # Errors
    /// On database failure.
    pub fn clear_avatar(&self, user_id: &str) -> StoreResult<()> {
        self.conn
            .execute("DELETE FROM avatars WHERE user_id = ?1", [user_id])?;
        Ok(())
    }

    /// `user_id`'s avatar: media type and bytes.
    ///
    /// # Errors
    /// On database failure.
    pub fn avatar(&self, user_id: &str) -> StoreResult<Option<(String, Vec<u8>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT mime, data FROM avatars WHERE user_id = ?1",
                [user_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// Forbids or allows a member to speak (ADR-0028). `false`: no such member.
    ///
    /// # Errors
    /// On database failure.
    pub fn set_silenced(&self, user_id: &str, silenced: bool) -> StoreResult<bool> {
        let changed = self.conn.execute(
            "UPDATE members SET silenced = ?2 WHERE user_id = ?1",
            params![user_id, silenced],
        )?;
        Ok(changed > 0)
    }

    /// Renames the camp.
    ///
    /// # Errors
    /// On database failure.
    pub fn rename_camp(&self, name: &str) -> StoreResult<()> {
        self.conn
            .execute("UPDATE meta SET value = ?1 WHERE key = 'camp_name'", [name])?;
        Ok(())
    }

    /// Gives a member a role. `false`: no such member.
    ///
    /// # Errors
    /// On database failure.
    pub fn set_role(&self, user_id: &str, role: Role) -> StoreResult<bool> {
        let changed = self.conn.execute(
            "UPDATE members SET role = ?2 WHERE user_id = ?1",
            params![user_id, role_str(role)],
        )?;
        Ok(changed > 0)
    }

    /// Adds a channel before the AFK campfire, which stays last. Its id is random, so a
    /// new campfire never lands in the LiveKit room of a removed one.
    ///
    /// # Errors
    /// On database failure.
    ///
    /// # Panics
    /// If the operating system cannot give random bytes — as for invites, unrecoverable.
    pub fn create_channel(&mut self, name: &str, kind: ChannelKind) -> StoreResult<Channel> {
        let mut bytes = [0u8; 5];
        getrandom::fill(&mut bytes).expect("operating system random number generator");
        // The invites' lower-case encoding: 8 characters, fine as a LiveKit room name.
        let id = format!("c{}", Token::from_bytes(&bytes).as_str());
        let kind_str = match kind {
            ChannelKind::Voice => "voice",
            ChannelKind::Text => "text",
        };
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO channels (id, name, position, kind, afk)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(position), -1) + 1 FROM channels), ?3, 0)",
            params![id, name, kind_str],
        )?;
        tx.execute(
            "UPDATE channels SET position = (SELECT MAX(position) + 1 FROM channels)
             WHERE afk = 1",
            [],
        )?;
        tx.commit()?;
        Ok(Channel {
            id,
            name: name.to_owned(),
            kind,
            afk: false,
        })
    }

    /// Renames a channel. `false`: no such channel.
    ///
    /// # Errors
    /// On database failure.
    pub fn rename_channel(&self, id: &str, name: &str) -> StoreResult<bool> {
        let changed = self.conn.execute(
            "UPDATE channels SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(changed > 0)
    }

    /// Removes a channel and returns it; `None`: no such channel. The AFK campfire is not
    /// removable — the caller checks [`Channel::afk`] first (ADR-0025).
    ///
    /// # Errors
    /// On database failure.
    pub fn delete_channel(&self, id: &str) -> StoreResult<Option<Channel>> {
        let channel = self.channel(id)?;
        if channel.is_some() {
            self.conn
                .execute("DELETE FROM channels WHERE id = ?1 AND afk = 0", [id])?;
        }
        Ok(channel)
    }

    /// One channel by id.
    ///
    /// # Errors
    /// On database failure.
    pub fn channel(&self, id: &str) -> StoreResult<Option<Channel>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, kind, afk FROM channels WHERE id = ?1",
                [id],
                |r| {
                    Ok(Channel {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        kind: channel_kind(&r.get::<_, String>(2)?),
                        afk: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// Whether a campfire (voice channel) with this id exists: only those have rooms.
    ///
    /// # Errors
    /// On database failure.
    pub fn campfire_exists(&self, id: &str) -> StoreResult<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM channels WHERE id = ?1 AND kind = 'voice'",
                [id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::Member => "member",
        Role::Moderator => "moderator",
        Role::Admin => "admin",
    }
}

/// The column's `CHECK` allows only the three names; anything else cannot be stored.
fn role_from(role: &str) -> Role {
    match role {
        "admin" => Role::Admin,
        "moderator" => Role::Moderator,
        _ => Role::Member,
    }
}

/// See [`role_from`]: `CHECK (kind IN ('voice', 'text'))`.
fn channel_kind(kind: &str) -> ChannelKind {
    if kind == "text" {
        ChannelKind::Text
    } else {
        ChannelKind::Voice
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
        StoreError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
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

/// Whether the camp is open (ADR-0040).
fn is_open(conn: &Connection) -> StoreResult<bool> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = 'open'", [], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .is_some_and(|v| v == "1"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_proto::AvatarFormat;

    #[test]
    fn a_camp_without_the_old_afk_campfire_gets_exactly_one_at_the_end() {
        let store = Store::open_in_memory("x").unwrap();
        store
            .conn
            .execute("DELETE FROM channels WHERE afk = 1", [])
            .unwrap();
        store.conn.execute(ENSURE_AFK, []).unwrap();
        store.conn.execute(ENSURE_AFK, []).unwrap();
        let ids: Vec<_> = store
            .camp()
            .unwrap()
            .channels
            .into_iter()
            .map(|c| (c.id, c.afk))
            .collect();
        assert_eq!(
            ids,
            [
                ("c0".into(), false),
                ("c1".into(), false),
                ("afk".into(), true)
            ],
            "once, last, even when asked twice"
        );
    }

    #[test]
    fn a_new_camp_starts_with_voice_chat_and_afk_and_a_friends_link() {
        let store = Store::open_in_memory("Вечерний кооп").unwrap();
        let camp = store.camp().unwrap();
        assert_eq!(camp.name, "Вечерний кооп");
        let channels: Vec<_> = camp
            .channels
            .iter()
            .map(|c| (c.id.as_str(), c.name.as_str(), c.kind, c.afk))
            .collect();
        assert_eq!(
            channels,
            [
                ("c0", "Voice", ChannelKind::Voice, false),
                ("c1", "Chat", ChannelKind::Text, false),
                ("afk", "AFK", ChannelKind::Voice, true),
            ]
        );
        assert_eq!(store.join_token().unwrap().as_str().len(), 16);
        let claims: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM invites WHERE kind = 'claim'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(claims, 0, "no admin key any more (ADR-0026)");
    }

    #[test]
    fn reopening_keeps_everything_and_the_first_name() {
        let dir = std::env::temp_dir().join(format!("oc-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("camp.db");
        let _ = std::fs::remove_file(&path);
        let (join, channels) = {
            let s = Store::open(&path, "Первое имя").unwrap();
            (s.join_token().unwrap(), s.camp().unwrap().channels)
        };
        let s = Store::open(&path, "Другое имя").unwrap();
        assert_eq!(s.camp().unwrap().name, "Первое имя", "name is set once");
        assert_eq!(s.join_token().unwrap(), join, "links survive restarts");
        assert_eq!(s.camp().unwrap().channels, channels, "seeded only once");
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A camp as schema v3 left it: an admin who redeemed the old key, a member with a
    /// live session, an admin key nobody redeemed, the v1 campfires.
    fn v3_camp() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE channels (id TEXT PRIMARY KEY, name TEXT NOT NULL,
                position INTEGER NOT NULL, afk INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE invites (token TEXT PRIMARY KEY,
                kind TEXT NOT NULL CHECK (kind IN ('join', 'claim')),
                created_at INTEGER NOT NULL, redeemed_at INTEGER);
             CREATE TABLE members (user_id TEXT PRIMARY KEY, public_key BLOB NOT NULL UNIQUE,
                name TEXT NOT NULL, role TEXT NOT NULL CHECK (role IN ('member', 'admin')),
                joined_at INTEGER NOT NULL);
             CREATE TABLE sessions (token_hash BLOB PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES members (user_id) ON DELETE CASCADE,
                expires_at INTEGER NOT NULL);
             INSERT INTO meta VALUES ('camp_name', 'Старый сервер');
             INSERT INTO channels VALUES ('c0', 'Лобби', 0, 0), ('c1', 'Катка', 1, 0),
                ('c2', 'AFK', 2, 1);
             INSERT INTO invites VALUES ('aaaaaaaaaaaaaaaa', 'join', 1, NULL),
                ('bbbbbbbbbbbbbbbbbbbbbbbbbb', 'claim', 1, NULL),
                ('cccccccccccccccccccccccccc', 'claim', 1, 2);
             INSERT INTO members VALUES ('dima', x'01', 'Дима', 'admin', 1),
                ('lena', x'02', 'Лена', 'member', 1);
             INSERT INTO sessions VALUES (x'aa', 'lena', 99999999999);
             PRAGMA user_version = 3;",
        )
        .unwrap();
        conn
    }

    #[test]
    fn v4_keeps_people_sessions_and_campfires_and_retires_the_admin_key() {
        let store = Store::init(v3_camp(), "ignored").unwrap();
        let roles: Vec<_> = store
            .members()
            .unwrap()
            .into_iter()
            .map(|m| (m.user_id, m.role))
            .collect();
        assert_eq!(
            roles,
            [("dima".into(), Role::Admin), ("lena".into(), Role::Member)],
            "whoever redeemed the key stays admin"
        );
        let lena = store.session_member(&[0xaa]).unwrap().unwrap();
        assert_eq!(lena.user_id, "lena", "sessions survive the table rebuild");
        let camp = store.camp().unwrap();
        assert_eq!(camp.name, "Старый сервер");
        let names: Vec<_> = camp.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["Лобби", "Катка", "AFK"],
            "an old camp is not reseeded"
        );
        assert!(camp.channels.iter().all(|c| c.kind == ChannelKind::Voice));
        let claims: Vec<String> = store
            .conn
            .prepare("SELECT token FROM invites WHERE kind = 'claim'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            claims,
            ["cccccccccccccccccccccccccc"],
            "unredeemed key gone"
        );
        // Moderators fit the new CHECK; foreign keys are back on.
        store
            .conn
            .execute(
                "UPDATE members SET role = 'moderator' WHERE user_id = 'lena'",
                [],
            )
            .unwrap();
        assert!(store
            .conn
            .execute("INSERT INTO sessions VALUES (x'bb', 'nobody', 1)", [])
            .is_err());
    }

    fn device(id: &str) -> Device<'_> {
        Device {
            user_id: id,
            public_key: id.as_bytes(),
            name: "Дима",
        }
    }

    #[test]
    fn new_device_needs_the_friends_invite_and_joins_as_a_member() {
        let mut store = Store::open_in_memory("x").unwrap();
        let join = store.join_token().unwrap().as_str().to_owned();
        assert_eq!(
            store.login(device("a"), None).unwrap(),
            Err(LoginError::InviteRequired)
        );
        assert_eq!(
            store
                .login(device("a"), Some("0".repeat(16).as_str()))
                .unwrap(),
            Err(LoginError::InviteInvalid)
        );
        assert_eq!(
            store.login(device("a"), Some(&join)).unwrap(),
            Ok(Role::Member)
        );
        assert_eq!(
            store.login(device("a"), None).unwrap(),
            Ok(Role::Member),
            "known devices need no invite"
        );
    }

    #[test]
    fn an_open_camp_lets_anyone_in_until_closed() {
        let mut store = Store::open_in_memory("x").unwrap();
        assert!(!store.camp().unwrap().open, "closed by default");
        store.set_open(true).unwrap();
        assert!(store.camp().unwrap().open);
        assert_eq!(
            store.login(device("a"), None).unwrap(),
            Ok(Role::Member),
            "no invite needed"
        );
        assert_eq!(
            store.login(device("a"), None).unwrap(),
            Ok(Role::Member),
            "and back again"
        );
        store.set_open(false).unwrap();
        assert_eq!(
            store.login(device("b"), None).unwrap(),
            Err(LoginError::InviteRequired),
            "closed again: newcomers need an invite"
        );
        assert_eq!(
            store.login(device("a"), None).unwrap(),
            Ok(Role::Member),
            "members stay"
        );
    }

    #[test]
    fn an_old_admin_key_lets_nobody_in_and_does_not_change_roles() {
        let mut store = Store::open_in_memory("x").unwrap();
        let claim = "dddddddddddddddddddddddddd";
        store
            .conn
            .execute(
                "INSERT INTO invites (token, kind, created_at) VALUES (?1, 'claim', 1)",
                [claim],
            )
            .unwrap();
        let status = store.invite_status(claim).unwrap().unwrap();
        assert_eq!(
            (status.kind, status.usable),
            (InviteKind::ClaimAdmin, false)
        );
        assert_eq!(
            store.login(device("new"), Some(claim)).unwrap(),
            Err(LoginError::InviteInvalid)
        );
        let join = store.join_token().unwrap().as_str().to_owned();
        store.login(device("a"), Some(&join)).unwrap().unwrap();
        assert_eq!(
            store.login(device("a"), Some(claim)).unwrap(),
            Ok(Role::Member),
            "a member is let in, still a member"
        );
    }

    /// ADR-0035: a member's avatar is kept, replaced, listed by hash, dropped — and goes
    /// with them when they leave.
    #[test]
    fn avatars_are_kept_per_member_and_leave_with_them() {
        let mut store = Store::open_in_memory("x").unwrap();
        let join = store.join_token().unwrap().as_str().to_owned();
        store.login(device("a"), Some(&join)).unwrap().unwrap();
        let first = b"RIFF\x10\0\0\0WEBPone";
        let second = b"\x89PNG\r\n\x1a\ntwo";

        assert!(store.set_avatar("a", AvatarFormat::Webp, first).unwrap());
        assert!(
            !store
                .set_avatar("ghost", AvatarFormat::Webp, first)
                .unwrap(),
            "no member, no avatar"
        );
        assert_eq!(
            store.members().unwrap()[0].avatar,
            Some(oc_proto::avatar_hash(first))
        );
        store.set_avatar("a", AvatarFormat::Png, second).unwrap();
        assert_eq!(
            store.avatar("a").unwrap(),
            Some(("image/png".to_owned(), second.to_vec())),
            "replaced"
        );
        store.clear_avatar("a").unwrap();
        assert_eq!(store.members().unwrap()[0].avatar, None);

        store.set_avatar("a", AvatarFormat::Webp, first).unwrap();
        store.remove_member("a").unwrap();
        let left: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM avatars", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "leaving takes the avatar along");
    }

    #[test]
    fn sessions_resolve_to_members_until_they_expire() {
        let mut store = Store::open_in_memory("x").unwrap();
        let join = store.join_token().unwrap().as_str().to_owned();
        store.login(device("a"), Some(&join)).unwrap().unwrap();
        store.create_session(b"live", "a", 60).unwrap();
        store.create_session(b"dead", "a", -1).unwrap();
        let member = store.session_member(b"live").unwrap().unwrap();
        assert_eq!(
            (member.user_id.as_str(), member.name.as_str(), member.role),
            ("a", "Дима", Role::Member)
        );
        assert_eq!(store.session_member(b"dead").unwrap(), None);
        assert_eq!(store.session_member(b"nope").unwrap(), None);
    }

    #[test]
    fn invite_status_and_channels() {
        let store = Store::open_in_memory("x").unwrap();
        let join = store.join_token().unwrap().as_str().to_owned();
        let status = store.invite_status(&join).unwrap().unwrap();
        assert_eq!((status.kind, status.usable), (InviteKind::Join, true));
        assert_eq!(store.invite_status("nonexistent").unwrap(), None);
        assert!(store.campfire_exists("c0").unwrap());
        assert!(!store.campfire_exists("c1").unwrap(), "Chat is no campfire");
        assert!(!store.campfire_exists("lobby").unwrap());
    }
}
