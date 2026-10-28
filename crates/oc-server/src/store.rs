//! Everything the camp remembers, in one SQLite file next to the server.
//!
//! The schema is versioned with `PRAGMA user_version`; each migration runs once, in a
//! transaction. Calls are synchronous and quick — [`crate::api`] runs them
//! off the async workers.

use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use oc_invite::{InviteKind, Token, CLAIM_TOKEN_BYTES, JOIN_TOKEN_BYTES};
pub use oc_proto::{Channel, Role};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

/// Channels a new camp starts with. The admin edits them later (M3 window / M5).
const DEFAULT_CHANNELS: [&str; 3] = ["Лобби", "Катка", "Отошёл (AFK)"];

/// Adds the AFK campfire at the end unless the camp has one (ADR-0025). `MAX()` sits in a
/// subquery: an aggregate over rows the `WHERE` filtered out still yields one row.
const ENSURE_AFK: &str = "INSERT INTO channels (id, name, position, afk)
     SELECT 'afk', 'AFK', (SELECT COALESCE(MAX(position), -1) + 1 FROM channels), 1
     WHERE NOT EXISTS (SELECT 1 FROM channels WHERE afk = 1)";

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

/// Public description of the camp.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Why a login was refused (the signature is checked before the store is asked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginError {
    /// A new device came without an invite.
    InviteRequired,
    /// A new device came with a token the server never issued.
    InviteInvalid,
    /// A new device came with the admin key somebody already redeemed.
    ClaimUsed,
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
            .prepare("SELECT id, name, afk FROM channels ORDER BY position")?;
        let channels = stmt
            .query_map([], |r| {
                Ok(Channel {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    afk: r.get(2)?,
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

    /// Admits a device whose signature the caller has verified (ADR-0018):
    ///
    /// * a known device is let in (its name updated); an unredeemed admin key promotes it;
    /// * a new device needs a valid invite; the admin key makes it admin.
    ///
    /// One write transaction (`BEGIN IMMEDIATE`): two devices racing for the admin key
    /// cannot both win.
    ///
    /// # Errors
    /// `Ok(Err(_))` when refused; `Err` on database failure.
    pub fn login(
        &mut self,
        device: Device<'_>,
        invite: Option<&str>,
    ) -> StoreResult<Result<Role, LoginError>> {
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
        let claimed = match invite {
            Some(token) => {
                tx.execute(
                    "UPDATE invites SET redeemed_at = ?2
                     WHERE token = ?1 AND kind = 'claim' AND redeemed_at IS NULL",
                    params![token, now()],
                )? == 1
            }
            None => false,
        };

        let role = match (known.as_deref(), claimed) {
            (_, true) | (Some("admin"), false) => Role::Admin,
            (Some(_), false) => Role::Member,
            (None, false) => {
                // New device without a fresh admin key: the invite decides.
                let kind: Option<String> = match invite {
                    Some(token) => tx
                        .query_row("SELECT kind FROM invites WHERE token = ?1", [token], |r| {
                            r.get(0)
                        })
                        .optional()?,
                    None => return Ok(Err(LoginError::InviteRequired)),
                };
                match kind.as_deref() {
                    Some("join") => Role::Member,
                    Some(_) => return Ok(Err(LoginError::ClaimUsed)),
                    None => return Ok(Err(LoginError::InviteInvalid)),
                }
            }
        };

        tx.execute(
            "INSERT INTO members (user_id, public_key, name, role, joined_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (user_id) DO UPDATE SET name = excluded.name, role = excluded.role",
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
        let row: Option<(String, String, String)> = self
            .conn
            .query_row(
                "SELECT m.user_id, m.name, m.role
                 FROM sessions s JOIN members m ON m.user_id = s.user_id
                 WHERE s.token_hash = ?1 AND s.expires_at > ?2",
                params![token_hash, now()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(row.map(|(user_id, name, role)| Member {
            user_id,
            name,
            role: if role == "admin" {
                Role::Admin
            } else {
                Role::Member
            },
        }))
    }

    /// Every member, sorted by id.
    ///
    /// # Errors
    /// On database failure.
    pub fn members(&self) -> StoreResult<Vec<oc_proto::MemberInfo>> {
        let mut stmt = self
            .conn
            .prepare("SELECT user_id, name, role FROM members ORDER BY user_id")?;
        let rows = stmt.query_map([], |r| {
            Ok(oc_proto::MemberInfo {
                user_id: r.get(0)?,
                name: r.get(1)?,
                is_admin: r.get::<_, String>(2)? == "admin",
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Whether a channel with this id exists.
    ///
    /// # Errors
    /// On database failure.
    pub fn channel_exists(&self, id: &str) -> StoreResult<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM channels WHERE id = ?1", [id], |_| Ok(()))
            .optional()?
            .is_some())
    }
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::Member => "member",
        Role::Admin => "admin",
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
    fn new_camp_has_name_default_channels_and_both_links() {
        let store = Store::open_in_memory("Вечерний кооп").unwrap();
        let camp = store.camp().unwrap();
        assert_eq!(camp.name, "Вечерний кооп");
        let names: Vec<_> = camp.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["Лобби", "Катка", "AFK"],
            "v3 turns the third into AFK"
        );
        let afk: Vec<_> = camp
            .channels
            .iter()
            .filter(|c| c.afk)
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(afk, ["c2"], "one AFK campfire, same LiveKit room as before");
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

    fn device(id: &str) -> Device<'_> {
        Device {
            user_id: id,
            public_key: id.as_bytes(),
            name: "Дима",
        }
    }

    fn tokens(store: &Store) -> (String, String) {
        (
            store.join_token().unwrap().as_str().to_owned(),
            store
                .unclaimed_admin_token()
                .unwrap()
                .unwrap()
                .as_str()
                .to_owned(),
        )
    }

    #[test]
    fn new_device_needs_a_valid_invite() {
        let mut store = Store::open_in_memory("x").unwrap();
        let (join, _) = tokens(&store);
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
    fn admin_key_works_exactly_once_and_its_link_disappears() {
        let mut store = Store::open_in_memory("x").unwrap();
        let (_, claim) = tokens(&store);
        assert_eq!(
            store.login(device("host"), Some(&claim)).unwrap(),
            Ok(Role::Admin)
        );
        assert_eq!(
            store.login(device("intruder"), Some(&claim)).unwrap(),
            Err(LoginError::ClaimUsed)
        );
        assert_eq!(store.unclaimed_admin_token().unwrap(), None);
        assert!(!store.invite_status(&claim).unwrap().unwrap().usable);
        assert_eq!(
            store.login(device("host"), Some(&claim)).unwrap(),
            Ok(Role::Admin),
            "the admin reopening their own claim link is still let in"
        );
    }

    #[test]
    fn a_member_can_redeem_the_admin_key_later_and_admins_stay_admins() {
        let mut store = Store::open_in_memory("x").unwrap();
        let (join, claim) = tokens(&store);
        store.login(device("a"), Some(&join)).unwrap().unwrap();
        assert_eq!(
            store.login(device("a"), Some(&claim)).unwrap(),
            Ok(Role::Admin)
        );
        assert_eq!(store.login(device("a"), None).unwrap(), Ok(Role::Admin));
        assert_eq!(
            store.login(device("a"), Some(&join)).unwrap(),
            Ok(Role::Admin)
        );
    }

    #[test]
    fn racing_for_the_admin_key_has_one_winner() {
        // Two connections to one file = two server requests in flight.
        let dir = std::env::temp_dir().join(format!("oc-store-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("camp.db");
        let claim = tokens(&Store::open(&path, "x").unwrap()).1;
        let results: Vec<_> = ["a", "b", "c", "d"]
            .map(|id| {
                let (path, claim) = (path.clone(), claim.clone());
                std::thread::spawn(move || {
                    let mut store = Store::open(&path, "x").unwrap();
                    store
                        .conn
                        .busy_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    store.login(device(id), Some(&claim)).unwrap()
                })
            })
            .map(|t| t.join().unwrap())
            .into_iter()
            .collect();
        let admins = results.iter().filter(|r| **r == Ok(Role::Admin)).count();
        let refused = results
            .iter()
            .filter(|r| **r == Err(LoginError::ClaimUsed))
            .count();
        assert_eq!((admins, refused), (1, 3), "{results:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sessions_resolve_to_members_until_they_expire() {
        let mut store = Store::open_in_memory("x").unwrap();
        let (join, _) = tokens(&store);
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
        let (join, claim) = tokens(&store);
        let status = store.invite_status(&join).unwrap().unwrap();
        assert_eq!((status.kind, status.usable), (InviteKind::Join, true));
        let status = store.invite_status(&claim).unwrap().unwrap();
        assert_eq!((status.kind, status.usable), (InviteKind::ClaimAdmin, true));
        assert_eq!(store.invite_status("nonexistent").unwrap(), None);
        assert!(store.channel_exists("c0").unwrap());
        assert!(!store.channel_exists("lobby").unwrap());
    }
}
