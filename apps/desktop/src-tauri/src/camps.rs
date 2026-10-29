//! Camps this device has joined (ADR-0020), kept in the app's data directory:
//!
//! * `device.key` — the device's Ed25519 key, i.e. who you are in every camp (ADR-0015);
//! * `camps.json` — per camp: address, certificate pin, session, cached name and channels,
//!   how far each chat is read (ADR-0044); plus your display name and the camp you were
//!   last in.
//!
//! Every network call goes through `oc-camp-client` (pinned TLS). Errors come back as
//! Russian text for the UI.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use oc_camp_client::{CampClient, CertificateProblem, ClientError, DeviceKey, Session, Trust};
use oc_invite::{Fingerprint, Host, InviteError, Link, Token};
use oc_proto::{
    avatar_hash, AvatarFormat, CampEvent, CampState, Channel, ErrorCode, InviteKindDto, Role,
    MAX_AVATAR_BYTES,
};
use serde::{Deserialize, Serialize};

use crate::voice::room::RoomAccess;

mod chat;
pub use chat::{ChatListener, ChatUpdate, OutgoingPhoto};

const KEY_FILE: &str = "device.key";
const CAMPS_FILE: &str = "camps.json";
/// Your avatar (ADR-0035): the one copy that counts; camps keep theirs.
const AVATAR_FILE: &str = "avatar";
/// Other people's avatars, by content hash: fetched once, kept across restarts.
const AVATAR_CACHE: &str = "avatars";

/// User-facing error text.
pub type CampError = String;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    camps: Vec<SavedCamp>,
    #[serde(default)]
    current: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedCamp {
    /// The certificate fingerprint: unique per camp and stable across addresses. For a
    /// camp checked by an authority (`by_name`), its `host:port` (ADR-0042).
    id: String,
    /// A domain with an authority's certificate, not a pin (ADR-0042).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    by_name: bool,
    host: String,
    port: u16,
    name: String,
    #[serde(default)]
    channels: Vec<Channel>,
    user_id: String,
    role: Role,
    /// Bearer token. A secret, like a browser cookie for this camp.
    session: String,
    /// Hash of the avatar this camp has from us; `None`: none (or never sent).
    #[serde(default)]
    avatar_sent: Option<String>,
    /// Chat id → the last `seq` read there (ADR-0044); a chat not here: nothing read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    read: BTreeMap<String, u64>,
}

impl SavedCamp {
    fn client(&self) -> Result<CampClient, CampError> {
        let host: Host = self
            .host
            .parse()
            .map_err(|_| format!("Сохранённый адрес сервера повреждён: {}", self.host))?;
        let trust = if self.by_name {
            Trust::of(None)
        } else {
            let fingerprint: Fingerprint = self
                .id
                .parse()
                .map_err(|_| "Сохранённый отпечаток сервера повреждён".to_owned())?;
            Trust::Pin(fingerprint)
        };
        Ok(CampClient::at(&host, self.port, trust))
    }

    fn session(&self) -> Session {
        Session::restore(self.session.clone(), self.user_id.clone(), self.role)
    }
}

/// A camp in the switcher.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CampSummary {
    /// Stable id (certificate fingerprint, or `host:port` of a domain camp).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Your role there.
    pub role: Role,
}

/// Everything the UI shows before a camp is open.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CampList {
    /// Name you go by (prefilled in the join form).
    pub display_name: String,
    /// Joined camps, in the order they were joined.
    pub camps: Vec<CampSummary>,
    /// The camp to open on start.
    pub current: Option<String>,
}

/// What a link leads to, shown before joining.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CampPreview {
    /// Camp name.
    pub name: String,
    /// Number of campfires.
    pub channels: usize,
    /// Friends invite or admin key.
    pub kind: InviteKindDto,
    /// `false` for an admin key somebody already used.
    pub usable: bool,
}

/// You, inside an open camp.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfInfo {
    /// See `oc_proto::user_id`.
    pub user_id: String,
    /// Display name.
    pub name: String,
    /// Role in the camp (ADR-0026).
    pub role: Role,
}

/// An opened camp.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCamp {
    /// Stable id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Campfires.
    pub channels: Vec<Channel>,
    /// You.
    #[serde(rename = "self")]
    pub me: SelfInfo,
    /// The server could not be reached; name and channels are the last known ones.
    pub offline: bool,
    /// Chat id → the last `seq` read on this device (ADR-0044).
    pub read: BTreeMap<String, u64>,
}

/// The device key and the joined camps.
pub struct Camps {
    file: PathBuf,
    /// The data directory: the avatar and the avatar cache live here too.
    dir: PathBuf,
    key: DeviceKey,
    saved: Mutex<Saved>,
    /// Where live camp state goes (the app forwards it to the webview).
    listener: Mutex<Option<StateListener>>,
    directives: Mutex<Option<DirectiveListener>>,
    chats: Mutex<Option<ChatListener>>,
    /// The task following the open camp's events; replaced when another camp opens.
    follower: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

/// Live state of the open camp, as sent to the UI (ADR-0021).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CampLive {
    /// Which camp this is about.
    pub camp_id: String,
    /// Whether the event stream is connected right now.
    pub online: bool,
    /// The latest snapshot; `None` while offline.
    pub state: Option<CampState>,
}

/// Receives [`CampLive`] updates.
pub type StateListener = Arc<dyn Fn(CampLive) + Send + Sync>;

/// What the open camp asked of this member alone (ADR-0028), as sent to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CampDirective {
    /// Go to this voice channel.
    Moved { camp_id: String, channel_id: String },
    /// Removed from the camp; this device has already forgotten it.
    Kicked { camp_id: String, camp_name: String },
}

/// Receives [`CampDirective`]s.
pub type DirectiveListener = Arc<dyn Fn(CampDirective) + Send + Sync>;

/// Reconnect delays for the event stream: 1 s, doubling, at most this.
const MAX_BACKOFF: Duration = Duration::from_secs(15);

impl Camps {
    /// Loads (or creates) the device key and the camp list in `dir`.
    ///
    /// # Errors
    /// If the directory or the device key cannot be read or written. A damaged camp list
    /// is set aside as `camps.json.bad` instead: losing the list beats not starting.
    pub fn load(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let key = DeviceKey::load_or_create(&dir.join(KEY_FILE))?;
        let file = dir.join(CAMPS_FILE);
        let saved = match fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!(
                    "[ourcamp] {} is damaged ({e}); starting with no camps",
                    file.display()
                );
                let _ = fs::rename(&file, file.with_extension("json.bad"));
                Saved::default()
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Saved::default(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            file,
            dir: dir.to_owned(),
            key,
            saved: Mutex::new(saved),
            listener: Mutex::new(None),
            directives: Mutex::new(None),
            chats: Mutex::new(None),
            follower: Mutex::new(None),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Saved> {
        self.saved.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn update(&self, change: impl FnOnce(&mut Saved)) -> Result<(), CampError> {
        let mut saved = self.lock();
        change(&mut saved);
        write_atomically(&self.file, &saved)
            .map_err(|e| format!("Не удалось сохранить список серверов: {e}"))
    }

    fn camp(&self, id: &str) -> Result<SavedCamp, CampError> {
        self.lock()
            .camps
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| "Этот сервер больше не в списке".to_owned())
    }

    /// This device's user id (same in every camp).
    #[cfg(test)]
    #[must_use]
    pub fn user_id(&self) -> String {
        self.key.user_id()
    }

    /// Joined camps.
    #[must_use]
    pub fn list(&self) -> CampList {
        let saved = self.lock();
        CampList {
            display_name: saved.display_name.clone(),
            camps: saved
                .camps
                .iter()
                .map(|c| CampSummary {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    role: c.role,
                })
                .collect(),
            current: saved.current.clone(),
        }
    }

    /// What a link leads to, without joining.
    ///
    /// # Errors
    /// Invalid link, unreachable camp, wrong certificate.
    pub async fn preview(&self, link: &str) -> Result<CampPreview, CampError> {
        let link = parse_link(link)?;
        let client = client_for(&link);
        let info = client.camp().await.map_err(describe)?;
        // An open camp's address (ADR-0040): usable while the camp stays open.
        let Some(token) = link.token() else {
            return Ok(CampPreview {
                name: info.name,
                channels: info.channels.len(),
                kind: InviteKindDto::Open,
                usable: info.open,
            });
        };
        let status = client.invite(token.as_str()).await.map_err(|e| match e {
            ClientError::Api(ErrorCode::NotFound) => "Приглашение недействительно".to_owned(),
            other => describe(other),
        })?;
        Ok(CampPreview {
            name: info.name,
            channels: info.channels.len(),
            kind: status.kind,
            usable: status.usable,
        })
    }

    /// Joins (or re-joins) the camp a link points to and makes it current.
    ///
    /// # Errors
    /// See [`preview`](Self::preview); plus refusals (used admin key, bad name).
    pub async fn join(&self, link: &str, name: &str) -> Result<OpenCamp, CampError> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Введите, как вас называть".into());
        }
        let link = parse_link(link)?;
        let client = client_for(&link);
        let session = client
            .login(&self.key, name, link.token().map(Token::as_str))
            .await
            .map_err(|e| match e {
                // An open camp's address, but the camp closed meanwhile.
                ClientError::Api(ErrorCode::InviteRequired) if link.token().is_none() => {
                    "Сервер больше не открытый — попросите ссылку-приглашение".to_owned()
                }
                other => describe(other),
            })?;
        let info = client.camp().await.map_err(describe)?;
        let id = camp_id(&link);
        let mut camp = SavedCamp {
            id: id.clone(),
            by_name: link.fingerprint().is_none(),
            host: link.host().to_string(),
            port: link.port(),
            name: info.name,
            channels: info.channels,
            user_id: session.user_id.clone(),
            role: session.role,
            session: session.token().to_owned(),
            avatar_sent: None,
            read: BTreeMap::new(),
        };
        // Coming back keeps what was read; a first join starts at now. The lock is let
        // go before the await: a guard held across it would make the future not `Send`.
        let known = self
            .lock()
            .camps
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.read.clone());
        camp.read = match known {
            Some(read) => read,
            None => Self::read_up_to_now(&client, &session, &camp).await,
        };
        self.update(|saved| {
            name.clone_into(&mut saved.display_name);
            saved.camps.retain(|c| c.id != id);
            saved.camps.push(camp.clone());
            saved.current = Some(id);
        })?;
        self.sync_avatar(&camp.id).await;
        Ok(self.opened(&camp, false))
    }

    /// Opens a joined camp: refreshes its name and channels and makes it current. If the
    /// camp is unreachable the last known state is returned with `offline: true`.
    ///
    /// # Errors
    /// Unknown id, or the list cannot be saved.
    pub async fn open(&self, id: &str) -> Result<OpenCamp, CampError> {
        let mut camp = self.camp(id)?;
        let fresh = camp.client()?.camp().await;
        let offline = match fresh {
            Ok(info) => {
                camp.name = info.name;
                camp.channels = info.channels;
                false
            }
            Err(e) => {
                eprintln!("[ourcamp] camp {} unreachable: {e}", camp.name);
                true
            }
        };
        self.update(|saved| {
            if let Some(stored) = saved.camps.iter_mut().find(|c| c.id == camp.id) {
                stored.name.clone_from(&camp.name);
                stored.channels.clone_from(&camp.channels);
            }
            saved.current = Some(camp.id.clone());
        })?;
        if !offline {
            self.sync_avatar(&camp.id).await;
        }
        Ok(self.opened(&camp, offline))
    }

    /// Your avatar as this device keeps it (ADR-0035).
    #[must_use]
    pub fn avatar(&self) -> Option<Vec<u8>> {
        fs::read(self.dir.join(AVATAR_FILE)).ok()
    }

    /// Sets (or with `None` drops) your avatar on this device, then on the open camp; the
    /// others get it when next opened.
    ///
    /// # Errors
    /// Not a PNG/WebP within the size limit, or the file cannot be written.
    pub async fn set_avatar(&self, picture: Option<Vec<u8>>) -> Result<(), CampError> {
        let path = self.dir.join(AVATAR_FILE);
        match picture {
            Some(bytes) => {
                if bytes.len() > MAX_AVATAR_BYTES || AvatarFormat::sniff(&bytes).is_none() {
                    return Err("Аватарка — картинка PNG или WebP до 96 КБ".into());
                }
                let tmp = path.with_extension("tmp");
                fs::write(&tmp, &bytes)
                    .and_then(|()| fs::rename(&tmp, &path))
                    .map_err(|e| format!("Не удалось сохранить аватарку: {e}"))?;
            }
            None => match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("Не удалось убрать аватарку: {e}")),
            },
        }
        let current = self.lock().current.clone();
        if let Some(id) = current {
            self.sync_avatar(&id).await;
        }
        Ok(())
    }

    /// Brings camp `id`'s copy of your avatar in line with this device's. Best effort:
    /// a camp that does not answer gets it next time it is opened.
    async fn sync_avatar(&self, id: &str) {
        let Ok(camp) = self.camp(id) else { return };
        let mine = self.avatar();
        let hash = mine.as_deref().map(avatar_hash);
        if hash == camp.avatar_sent {
            return;
        }
        let Ok(client) = camp.client() else { return };
        let send = |session: Session| {
            let (client, mine) = (&client, mine.clone());
            async move {
                match mine {
                    Some(bytes) => client.set_avatar(&session, bytes).await,
                    None => client.clear_avatar(&session).await,
                }
            }
        };
        let result = match send(camp.session()).await {
            Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                match self.relogin(&camp, &client).await {
                    Ok(session) => send(session).await,
                    Err(e) => {
                        eprintln!("[ourcamp] avatar to {}: {e}", camp.name);
                        return;
                    }
                }
            }
            other => other,
        };
        match result {
            Ok(()) => {
                if let Err(e) = self.update(|saved| {
                    if let Some(stored) = saved.camps.iter_mut().find(|c| c.id == id) {
                        stored.avatar_sent.clone_from(&hash);
                    }
                }) {
                    eprintln!("[ourcamp] avatar to {}: {e}", camp.name);
                }
            }
            Err(e) => eprintln!("[ourcamp] avatar to {}: {}", camp.name, describe(e)),
        }
    }

    /// A member's avatar by the hash the camp's snapshot names, from the cache or the camp.
    ///
    /// # Errors
    /// A malformed hash, no such avatar, unreachable camp.
    pub async fn member_avatar(
        &self,
        id: &str,
        user_id: &str,
        hash: &str,
    ) -> Result<Vec<u8>, CampError> {
        // The hash names a file: it must be what `avatar_hash` makes, never a path.
        if hash.len() != 16
            || !hash
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("Неверное имя аватарки".into());
        }
        let cached = self.dir.join(AVATAR_CACHE).join(hash);
        if let Ok(bytes) = fs::read(&cached) {
            if avatar_hash(&bytes) == hash {
                return Ok(bytes);
            }
        }
        let camp = self.camp(id)?;
        let client = camp.client()?;
        let bytes = match client.avatar(&camp.session(), user_id).await {
            Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                let session = self.relogin(&camp, &client).await?;
                client.avatar(&session, user_id).await
            }
            other => other,
        }
        .map_err(describe)?;
        if AvatarFormat::sniff(&bytes).is_none() {
            return Err("Сервер прислал не картинку".into());
        }
        // Kept under its own hash: if it changed meanwhile, the next snapshot names it.
        let file = self.dir.join(AVATAR_CACHE).join(avatar_hash(&bytes));
        if let Err(e) =
            fs::create_dir_all(self.dir.join(AVATAR_CACHE)).and_then(|()| fs::write(&file, &bytes))
        {
            eprintln!("[ourcamp] avatar cache: {e}");
        }
        Ok(bytes)
    }

    fn opened(&self, camp: &SavedCamp, offline: bool) -> OpenCamp {
        OpenCamp {
            id: camp.id.clone(),
            name: camp.name.clone(),
            channels: camp.channels.clone(),
            me: SelfInfo {
                user_id: camp.user_id.clone(),
                name: self.lock().display_name.clone(),
                role: camp.role,
            },
            offline,
            read: camp.read.clone(),
        }
    }

    /// Removes a camp from this device (the camp itself still remembers the member).
    ///
    /// # Errors
    /// If the list cannot be saved.
    pub fn forget(&self, id: &str) -> Result<(), CampError> {
        if self.lock().current.as_deref() == Some(id) {
            self.unfollow();
        }
        self.update(|saved| {
            saved.camps.retain(|c| c.id != id);
            if saved.current.as_deref() == Some(id) {
                saved.current = None;
            }
        })
    }

    /// Where and with what token to join a channel's voice room.
    ///
    /// # Errors
    /// Unreachable camp, unknown channel.
    pub async fn room_access(&self, id: &str, channel_id: &str) -> Result<RoomAccess, CampError> {
        let camp = self.camp(id)?;
        let client = camp.client()?;
        let token = match client.room_token(&camp.session(), channel_id).await {
            Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                let session = self.relogin(&camp, &client).await?;
                client.room_token(&session, channel_id).await
            }
            other => other,
        }
        .map_err(describe)?
        .token;
        Ok(RoomAccess {
            url: client.voice_url(),
            token,
        })
    }

    /// The friends link of a camp, to copy and send.
    ///
    /// # Errors
    /// Unreachable camp.
    pub async fn invite_link(&self, id: &str) -> Result<String, CampError> {
        let camp = self.camp(id)?;
        let client = camp.client()?;
        let invite = match client.current_invite(&camp.session()).await {
            Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                let session = self.relogin(&camp, &client).await?;
                client.current_invite(&session).await
            }
            other => other,
        }
        .map_err(describe)?;
        Ok(invite.to_string())
    }

    /// Moves someone sitting in voice to another campfire; their app joins it. Needs a
    /// moderator's or admin's rights over them (ADR-0029).
    ///
    /// # Errors
    /// No rights, they left voice, unknown member or channel, unreachable camp.
    pub async fn move_member(
        &self,
        id: &str,
        user_id: &str,
        channel_id: &str,
    ) -> Result<(), CampError> {
        let camp = self.camp(id)?;
        let client = camp.client()?;
        match client
            .move_member(&camp.session(), user_id, channel_id)
            .await
        {
            Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                let session = self.relogin(&camp, &client).await?;
                client.move_member(&session, user_id, channel_id).await
            }
            other => other,
        }
        .map_err(|e| match e {
            ClientError::Api(ErrorCode::NotFound) => "Этого участника или канала уже нет".into(),
            ClientError::Api(ErrorCode::BadRequest) => {
                "Перенести можно только в голосовой канал".into()
            }
            other => describe(other),
        })
    }

    /// Kicks someone of a lower rank out of the camp; no ban (ADR-0033).
    ///
    /// # Errors
    /// No rights over them, unknown member, unreachable camp.
    pub async fn kick(&self, id: &str, user_id: &str) -> Result<(), CampError> {
        let camp = self.camp(id)?;
        let client = camp.client()?;
        match client.kick(&camp.session(), user_id).await {
            Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                let session = self.relogin(&camp, &client).await?;
                client.kick(&session, user_id).await
            }
            other => other,
        }
        .map_err(moderation_error)
    }

    /// Forbids someone of a lower rank to speak, or lifts it (ADR-0033).
    ///
    /// # Errors
    /// No rights over them, unknown member, unreachable camp.
    pub async fn set_silenced(
        &self,
        id: &str,
        user_id: &str,
        silenced: bool,
    ) -> Result<(), CampError> {
        let camp = self.camp(id)?;
        let client = camp.client()?;
        match client
            .set_silenced(&camp.session(), user_id, silenced)
            .await
        {
            Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                let session = self.relogin(&camp, &client).await?;
                client.set_silenced(&session, user_id, silenced).await
            }
            other => other,
        }
        .map_err(moderation_error)
    }

    /// Whether `id` is the open camp (the one voice can be in).
    pub fn is_current(&self, id: &str) -> bool {
        self.lock().current.as_deref() == Some(id)
    }

    /// Leaves a camp: the camp forgets you, then this device forgets the camp. Returns
    /// whether the camp heard it; when it is unreachable the camp is still dropped from
    /// the list here, and the camp keeps you as a member until you leave again.
    ///
    /// # Errors
    /// Unknown camp, or the camp list cannot be saved.
    pub async fn leave(&self, id: &str) -> Result<bool, CampError> {
        let camp = self.camp(id)?;
        let client = camp.client()?;
        let told = match client.leave(&camp.session()).await {
            Ok(()) => true,
            // The session expired: log in again (no invite — the camp knows this device)
            // and leave with the new one. "Invite required" means we are no member any
            // more: nothing left to tell.
            Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                let name = self.lock().display_name.clone();
                match client.login(&self.key, &name, None).await {
                    Ok(session) => client.leave(&session).await.is_ok(),
                    Err(ClientError::Api(ErrorCode::InviteRequired)) => true,
                    Err(_) => false,
                }
            }
            Err(e) => {
                eprintln!("[ourcamp] leave {id}: {}", describe(e));
                false
            }
        };
        self.forget(id)?;
        Ok(told)
    }

    /// Where live camp state goes.
    pub fn set_state_listener(&self, listener: StateListener) {
        *self.listener.lock().unwrap_or_else(PoisonError::into_inner) = Some(listener);
    }

    /// Where directives go.
    pub fn set_directive_listener(&self, listener: DirectiveListener) {
        *self
            .directives
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(listener);
    }

    fn direct(&self, directive: CampDirective) {
        let listener = self
            .directives
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(listener) = listener {
            listener(directive);
        }
    }

    /// Kicked out of `id` while following it: forget it here (the camp already has) and
    /// tell the UI. Called from the follower itself, so it detaches rather than aborts.
    fn kicked(&self, id: &str) {
        let Ok(camp) = self.camp(id) else { return };
        if self.lock().current.as_deref() == Some(id) {
            drop(
                self.follower
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take(),
            );
        }
        if let Err(e) = self.update(|saved| {
            saved.camps.retain(|c| c.id != id);
            if saved.current.as_deref() == Some(id) {
                saved.current = None;
            }
        }) {
            eprintln!("[ourcamp] kicked from {id}: {e}");
        }
        self.direct(CampDirective::Kicked {
            camp_id: camp.id,
            camp_name: camp.name,
        });
    }

    fn notify(&self, live: CampLive) {
        let listener = self
            .listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(listener) = listener {
            listener(live);
        }
    }

    /// Follows `id`'s live state until another camp is followed or this one forgotten:
    /// reconnects with backoff, re-logs in when the session expired.
    pub fn follow(self: &Arc<Self>, id: &str) {
        let task = tauri::async_runtime::spawn(Arc::clone(self).follow_loop(id.to_owned()));
        let previous = self
            .follower
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(task);
        if let Some(previous) = previous {
            previous.abort();
        }
    }

    fn unfollow(&self) {
        let current = self
            .follower
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(task) = current {
            task.abort();
        }
    }

    async fn follow_loop(self: Arc<Self>, id: String) {
        let mut backoff = Duration::from_secs(1);
        let mut was_online = false;
        loop {
            match self.open_events(&id).await {
                Ok(mut events) => {
                    backoff = Duration::from_secs(1);
                    while let Some(event) = events.next().await {
                        match event {
                            Ok(CampEvent::State(state)) => {
                                was_online = true;
                                self.notify(CampLive {
                                    camp_id: id.clone(),
                                    online: true,
                                    state: Some(state),
                                });
                            }
                            Ok(CampEvent::Moved { channel_id }) => {
                                self.direct(CampDirective::Moved {
                                    camp_id: id.clone(),
                                    channel_id,
                                });
                            }
                            Ok(CampEvent::Message(message)) => {
                                self.chat_update(ChatUpdate::Message {
                                    camp_id: id.clone(),
                                    message,
                                });
                            }
                            // The camp closes the stream after this: nothing to reconnect to.
                            Ok(CampEvent::Kicked) => {
                                self.kicked(&id);
                                return;
                            }
                            Err(e) => {
                                eprintln!("[ourcamp] camp events: {}", describe(e));
                                break;
                            }
                        }
                    }
                }
                // Logged once per outage, not on every retry.
                Err(e) if was_online => eprintln!("[ourcamp] camp events: {e}"),
                Err(_) => {}
            }
            if was_online {
                was_online = false;
                self.notify(CampLive {
                    camp_id: id.clone(),
                    online: false,
                    state: None,
                });
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    async fn open_events(&self, id: &str) -> Result<oc_camp_client::CampEvents, CampError> {
        let camp = self.camp(id)?;
        let client = camp.client()?;
        match client.events(&camp.session()).await {
            Err(ClientError::Api(ErrorCode::Unauthorized)) => {
                let session = self.relogin(&camp, &client).await?;
                client.events(&session).await
            }
            other => other,
        }
        .map_err(describe)
    }

    /// The session expired: log in again with the device key (no invite needed — the
    /// camp knows this device) and keep the new session.
    async fn relogin(&self, camp: &SavedCamp, client: &CampClient) -> Result<Session, CampError> {
        let name = self.lock().display_name.clone();
        let session = client
            .login(&self.key, &name, None)
            .await
            .map_err(describe)?;
        self.update(|saved| {
            if let Some(stored) = saved.camps.iter_mut().find(|c| c.id == camp.id) {
                session.token().clone_into(&mut stored.session);
                stored.role = session.role;
            }
        })?;
        Ok(session)
    }
}

fn write_atomically(path: &Path, saved: &Saved) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(saved).map_err(io::Error::other)?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

/// A client for what a link points at.
fn client_for(link: &Link) -> CampClient {
    CampClient::at(link.host(), link.port(), Trust::of(link.fingerprint()))
}

/// The camp a link is to: its pin, stable across addresses; a domain camp's name and
/// port (its certificate changes with every renewal, ADR-0042).
fn camp_id(link: &Link) -> String {
    link.fingerprint().map_or_else(
        || format!("{}:{}", link.host(), link.port()).to_ascii_lowercase(),
        |fp| fp.to_string(),
    )
}

fn parse_link(link: &str) -> Result<Link, CampError> {
    link.parse().map_err(|e: InviteError| {
        match e {
            InviteError::NotAnInvite => "Это не ссылка OurCamp — она начинается с ourcamp://",
            InviteError::InvalidHost | InviteError::InvalidPort => {
                "В ссылке неверный адрес сервера"
            }
            InviteError::InvalidPath | InviteError::InvalidToken => {
                "Ссылка повреждена — попросите прислать её заново"
            }
            InviteError::MissingFingerprint | InviteError::InvalidFingerprint => {
                "В ссылке нет отпечатка сервера (он нужен для IP-адреса) — попросите прислать её заново"
            }
        }
        .to_owned()
    })
}

/// What a person can do about the camp's certificate.
fn certificate_problem(problem: CertificateProblem) -> CampError {
    match problem {
        CertificateProblem::NotPinned => {
            "Сертификат сервера не совпадает со ссылкой: это может быть подмена, или сервер \
             переустановлен — попросите новую ссылку"
        }
        CertificateProblem::Expired => {
            "У сертификата сервера истёк срок — владельцу нужно его обновить"
        }
        CertificateProblem::WrongName => {
            "Сертификат сервера выдан для другого адреса: это может быть подмена, или сервер \
             настроен с ошибкой"
        }
        CertificateProblem::UnknownIssuer => {
            "Сертификат сервера не подтверждён удостоверяющим центром: это может быть подмена — \
             или попросите ссылку с отпечатком"
        }
        CertificateProblem::Other => "Сертификат сервера не прошёл проверку",
    }
    .into()
}

/// A client error as a sentence for the UI.
/// Kick and silence refusals: rights here are about rank, not just role (ADR-0033).
fn moderation_error(e: ClientError) -> CampError {
    match e {
        ClientError::Api(ErrorCode::Forbidden) => {
            "Это может только тот, у кого роль выше: модератор — для участников, админ — для модераторов"
                .into()
        }
        ClientError::Api(ErrorCode::NotFound) => "Этого участника на сервере уже нет".into(),
        other => describe(other),
    }
}

fn describe(e: ClientError) -> CampError {
    if let Some(problem) = e.certificate_problem() {
        return certificate_problem(problem);
    }
    match e {
        ClientError::Io(e) => format!("Сервер недоступен: {e}"),
        ClientError::Api(code) => match code {
            ErrorCode::InviteRequired => "Для первого входа нужна ссылка-приглашение",
            ErrorCode::InviteInvalid => "Приглашение недействительно",
            ErrorCode::Forbidden => "Для этого нужны права админа или модератора",
            ErrorCode::BadSignature | ErrorCode::NonceInvalid => {
                "Не удалось подтвердить устройство — попробуйте ещё раз"
            }
            ErrorCode::Unauthorized => "Сессия на сервере истекла",
            ErrorCode::NotFound => "Такого канала на сервере нет",
            ErrorCode::NotInVoice => "Участник уже вышел из голосового канала",
            ErrorCode::Silenced => "Вам запрещено говорить и писать на этом сервере",
            ErrorCode::BadRequest => "Сервер не принял данные — проверьте имя (до 32 символов)",
            ErrorCode::Internal => "Ошибка на сервере",
        }
        .into(),
        ClientError::Protocol(m) => format!("Сервер ответил непонятно: {m}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oc-camps-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn empty_directory_means_no_camps_and_a_new_key() {
        let dir = temp_dir("empty");
        let camps = Camps::load(&dir).unwrap();
        assert!(camps.list().camps.is_empty());
        assert!(dir.join(KEY_FILE).exists());
        let again = Camps::load(&dir).unwrap();
        assert_eq!(
            camps.user_id(),
            again.user_id(),
            "same device across restarts"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_domain_camp_is_known_by_its_name_and_a_pinned_one_by_its_pin() {
        let domain: Link = "ourcamp://OurCamp.kz/join/7k2q9m4x8v1c3b5n"
            .parse()
            .unwrap();
        assert_eq!(camp_id(&domain), "ourcamp.kz:7780");
        let fp = "A".repeat(43);
        let pinned: Link = format!("ourcamp://192.0.2.15:7781?fp={fp}")
            .parse()
            .unwrap();
        assert_eq!(camp_id(&pinned), fp, "the same camp at any address");

        // A camp saved before ADR-0042 has no `byName`: it is pinned, as it was.
        let old: SavedCamp = serde_json::from_value(serde_json::json!({
            "id": fp, "host": "192.0.2.15", "port": 7780, "name": "Кооп",
            "userId": "u", "role": "member", "session": "s",
        }))
        .unwrap();
        assert!(!old.by_name);
        assert!(old.client().is_ok());
        let saved = serde_json::to_value(&old).unwrap();
        assert!(saved.get("byName").is_none(), "nothing new written for it");

        let by_name = SavedCamp {
            id: camp_id(&domain),
            by_name: true,
            host: "ourcamp.kz".into(),
            ..old
        };
        assert!(by_name.client().is_ok(), "no pin to parse");
    }

    #[test]
    fn a_damaged_camp_list_is_set_aside_not_fatal() {
        let dir = temp_dir("damaged");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(CAMPS_FILE), b"{ not json").unwrap();
        let camps = Camps::load(&dir).unwrap();
        assert!(camps.list().camps.is_empty());
        assert!(dir.join("camps.json.bad").exists(), "kept for inspection");
        let _ = fs::remove_dir_all(dir);
    }

    /// A camp server in this process. Its LiveKit is a port nobody listens on: nothing
    /// here connects voice, and results must not depend on a dev LiveKit being up.
    async fn camp(name: &str) -> (oc_server::Running, PathBuf) {
        let dir = temp_dir(&format!("server-{name}"));
        let free = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let livekit = format!("ws://{}", free.local_addr().unwrap());
        drop(free);
        let running = oc_server::start(oc_server::Config {
            data_dir: dir.clone(),
            bind: (std::net::Ipv4Addr::LOCALHOST, 0).into(),
            public_host: Host::Ipv4(std::net::Ipv4Addr::LOCALHOST),
            public_port: None,
            camp_name: "Вечерний кооп".into(),
            router: None,
            certificate: None,
            livekit: oc_server::LiveKit {
                url: livekit,
                api_key: "devkey".into(),
                api_secret: "secret".into(),
            }
            .into(),
        })
        .await
        .unwrap();
        (running, dir)
    }

    #[test]
    fn join_by_link_then_reopen_after_restart() {
        tauri::async_runtime::block_on(async {
            let (server, server_dir) = camp("reopen").await;
            let dir = temp_dir("reopen");
            let link = server.links.join.to_string();

            let camps = Camps::load(&dir).unwrap();
            let preview = camps.preview(&link).await.unwrap();
            assert_eq!(preview.name, "Вечерний кооп");
            assert_eq!(preview.channels, 3);
            assert_eq!((preview.kind, preview.usable), (InviteKindDto::Join, true));

            let opened = camps.join(&link, "  Дима  ").await.unwrap();
            assert_eq!(
                opened.id,
                server.fingerprint.to_string(),
                "camp id = its pin"
            );
            assert_eq!(opened.channels[0].name, "Voice");
            assert_eq!(opened.me.name, "Дима", "trimmed");
            assert_eq!(
                opened.me.role,
                Role::Member,
                "roles come from the owner, not links"
            );
            assert_eq!(opened.me.user_id, camps.user_id());
            assert!(!opened.offline);

            // Restart: the camp, the name and the current choice survive.
            drop(camps);
            let camps = Camps::load(&dir).unwrap();
            let list = camps.list();
            assert_eq!(list.display_name, "Дима");
            assert_eq!(list.camps.len(), 1);
            assert_eq!(list.current.as_deref(), Some(opened.id.as_str()));
            let reopened = camps.open(&opened.id).await.unwrap();
            assert_eq!(reopened.channels, opened.channels);

            let access = camps.room_access(&opened.id, "c0").await.unwrap();
            assert_eq!(
                access.url,
                format!("wss://127.0.0.1:{}", server.local_addr.port())
            );
            assert!(access.token.split('.').count() == 3, "a JWT");
            assert!(camps
                .room_access(&opened.id, "nope")
                .await
                .unwrap_err()
                .contains("канала"));

            server.stop().await;
            let _ = fs::remove_dir_all(dir);
            let _ = fs::remove_dir_all(server_dir);
        });
    }

    #[test]
    fn a_copied_invite_brings_a_friend_in_and_offline_camps_still_open() {
        tauri::async_runtime::block_on(async {
            let (server, server_dir) = camp("invite").await;
            let host_dir = temp_dir("invite-host");
            let friend_dir = temp_dir("invite-friend");
            let host = Camps::load(&host_dir).unwrap();
            let opened = host
                .join(&server.links.join.to_string(), "Дима")
                .await
                .unwrap();

            let link = host.invite_link(&opened.id).await.unwrap();
            let friend = Camps::load(&friend_dir).unwrap();
            let joined = friend.join(&link, "Лена").await.unwrap();
            assert_eq!(joined.me.role, Role::Member);
            assert_ne!(joined.me.user_id, opened.me.user_id, "a different device");

            // The camp goes down: it still opens from cache, flagged offline.
            server.stop().await;
            let offline = host.open(&opened.id).await.unwrap();
            assert!(offline.offline);
            assert_eq!(offline.channels, opened.channels);

            host.forget(&opened.id).unwrap();
            assert!(host.list().camps.is_empty());
            assert_eq!(host.list().current, None);

            for d in [host_dir, friend_dir, server_dir] {
                let _ = fs::remove_dir_all(d);
            }
        });
    }

    #[test]
    fn the_open_camp_is_followed_live_and_going_offline_is_reported() {
        tauri::async_runtime::block_on(async {
            let (server, server_dir) = camp("follow").await;
            let dir = temp_dir("follow");
            let camps = Arc::new(Camps::load(&dir).unwrap());
            let seen: Arc<Mutex<Vec<CampLive>>> = Arc::default();
            let sink = Arc::clone(&seen);
            camps.set_state_listener(Arc::new(move |live| sink.lock().unwrap().push(live)));

            let link = server.links.join.to_string();
            let opened = camps.join(&link, "Дима").await.unwrap();
            camps.follow(&opened.id);

            let wait_for = |pred: &dyn Fn(&CampLive) -> bool| {
                let seen = Arc::clone(&seen);
                let found = (0..60).any(|_| {
                    if seen.lock().unwrap().iter().any(pred) {
                        return true;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    false
                });
                found
            };
            let me = opened.me.user_id.clone();
            assert!(
                wait_for(&|l| l.online
                    && l.state
                        .as_ref()
                        .is_some_and(|s| s.members.iter().any(|m| m.user_id == me))),
                "a snapshot with us: {:?}",
                seen.lock().unwrap()
            );
            assert!(seen.lock().unwrap().iter().all(|l| l.camp_id == opened.id));

            server.stop().await;
            assert!(
                wait_for(&|l| !l.online && l.state.is_none()),
                "offline is reported"
            );

            camps.forget(&opened.id).unwrap();
            let _ = fs::remove_dir_all(dir);
            let _ = fs::remove_dir_all(server_dir);
        });
    }

    /// ADR-0028: the owner kicks us while we follow the camp — the UI is told, the camp
    /// is gone from this device, and the follower stops instead of knocking again.
    #[test]
    fn a_kick_reaches_the_ui_and_drops_the_camp_here() {
        tauri::async_runtime::block_on(async {
            let (server, server_dir) = camp("kick").await;
            let dir = temp_dir("kick");
            let camps = Arc::new(Camps::load(&dir).unwrap());
            let seen: Arc<Mutex<Vec<CampDirective>>> = Arc::default();
            let sink = Arc::clone(&seen);
            camps.set_directive_listener(Arc::new(move |d| sink.lock().unwrap().push(d)));
            let online = Arc::new(tokio::sync::Notify::new());
            let ready = Arc::clone(&online);
            camps.set_state_listener(Arc::new(move |live| {
                if live.online {
                    ready.notify_one();
                }
            }));

            let opened = camps
                .join(&server.links.join.to_string(), "Лена")
                .await
                .unwrap();
            camps.follow(&opened.id);
            tokio::time::timeout(Duration::from_secs(5), online.notified())
                .await
                .expect("following");

            server.manager().kick(&opened.me.user_id).await.unwrap();
            let directive = (0..50).find_map(|_| {
                let first = seen.lock().unwrap().first().cloned();
                if first.is_none() {
                    std::thread::sleep(Duration::from_millis(100));
                }
                first
            });
            assert_eq!(
                directive,
                Some(CampDirective::Kicked {
                    camp_id: opened.id.clone(),
                    camp_name: "Вечерний кооп".into(),
                })
            );
            assert!(camps.list().camps.is_empty());
            assert_eq!(camps.list().current, None);
            assert!(
                camps.follower.lock().unwrap().is_none(),
                "no reconnecting to a camp that threw us out"
            );

            server.stop().await;
            let _ = fs::remove_dir_all(dir);
            let _ = fs::remove_dir_all(server_dir);
        });
    }

    /// ADR-0044: what was said before you came is not unread; what is said after reaches
    /// the UI live; read marks only move forward and outlive a restart; a photo is
    /// fetched once and then comes from the cache, even with the camp down.
    #[test]
    fn chats_are_read_from_the_join_on_and_photos_kept() {
        tauri::async_runtime::block_on(async {
            let (server, server_dir) = camp("chat").await;
            let link = server.links.join.to_string();
            let (bob_dir, dir) = (temp_dir("chat-bob"), temp_dir("chat-lena"));
            let bob = Camps::load(&bob_dir).unwrap();
            let bobs = bob.join(&link, "Боб").await.unwrap();
            bob.send(&bobs.id, "c1", "Был тут раньше".into(), None)
                .await
                .unwrap();

            let lena = Arc::new(Camps::load(&dir).unwrap());
            let seen: Arc<Mutex<Vec<ChatUpdate>>> = Arc::default();
            let sink = Arc::clone(&seen);
            lena.set_chat_listener(Arc::new(move |u| sink.lock().unwrap().push(u)));
            let online = Arc::new(tokio::sync::Notify::new());
            let ready = Arc::clone(&online);
            lena.set_state_listener(Arc::new(move |live| {
                if live.online {
                    ready.notify_one();
                }
            }));
            let opened = lena.join(&link, "Лена").await.unwrap();
            assert_eq!(
                opened.read.get("c1"),
                Some(&1),
                "Боб's old message is not unread"
            );
            lena.follow(&opened.id);
            tokio::time::timeout(Duration::from_secs(5), online.notified())
                .await
                .expect("following");

            let sent = bob
                .send(&bobs.id, "c1", "Привет, Лена".into(), None)
                .await
                .unwrap();
            let live = (0..50).find_map(|_| {
                let got = seen.lock().unwrap().first().cloned();
                if got.is_none() {
                    std::thread::sleep(Duration::from_millis(100));
                }
                got
            });
            assert_eq!(
                live,
                Some(ChatUpdate::Message {
                    camp_id: opened.id.clone(),
                    message: sent.clone(),
                })
            );
            let history = lena.history(&opened.id, "c1", None, 50).await.unwrap();
            assert_eq!(history.len(), 2);
            assert_eq!(history[1], sent);

            seen.lock().unwrap().clear();
            lena.mark_read(&opened.id, "c1", 2).unwrap();
            lena.mark_read(&opened.id, "c1", 1).unwrap();
            assert_eq!(
                *seen.lock().unwrap(),
                [ChatUpdate::Read {
                    camp_id: opened.id.clone(),
                    channel_id: "c1".into(),
                    seq: 2,
                }],
                "forward only"
            );

            let png = b"\x89PNG\r\n\x1a\nnot a real picture".to_vec();
            let photo = OutgoingPhoto {
                bytes: png.clone(),
                width: 10,
                height: 10,
            };
            let shown = lena
                .send(&opened.id, "c1", String::new(), Some(photo))
                .await
                .unwrap();
            let hash = shown.photo.expect("a photo").hash;
            assert_eq!(bob.photo(&bobs.id, &hash).await.unwrap(), png);

            lena.unfollow();
            server.stop().await;
            assert_eq!(
                bob.photo(&bobs.id, &hash).await.unwrap(),
                png,
                "from the cache"
            );
            let again = Camps::load(&dir).unwrap();
            assert_eq!(again.camp(&opened.id).unwrap().read.get("c1"), Some(&2));

            for d in [dir, bob_dir, server_dir] {
                let _ = fs::remove_dir_all(d);
            }
        });
    }

    /// ADR-0035: the avatar lives on the device and reaches the camp on joining; another
    /// member fetches it once by hash, then from the cache — even with the camp down.
    #[test]
    fn your_avatar_reaches_the_camp_and_others_fetch_it_once() {
        tauri::async_runtime::block_on(async {
            let (server, server_dir) = camp("avatar").await;
            let link = server.links.join.to_string();
            let (lena_dir, bob_dir) = (temp_dir("avatar-lena"), temp_dir("avatar-bob"));
            let lena = Camps::load(&lena_dir).unwrap();
            let picture = b"RIFF\x1a\0\0\0WEBPVP8 \x0e\0\0\0lena's face".to_vec();

            assert!(lena.set_avatar(Some(b"GIF89a".to_vec())).await.is_err());
            lena.set_avatar(Some(picture.clone())).await.unwrap();
            assert_eq!(lena.avatar(), Some(picture.clone()), "kept on the device");
            let opened = lena.join(&link, "Лена").await.unwrap();

            let hash = avatar_hash(&picture);
            let has = |want: Option<&str>| {
                let mut state = server.camp_state();
                let me = opened.me.user_id.clone();
                let want = want.map(str::to_owned);
                async move {
                    tokio::time::timeout(
                        Duration::from_secs(5),
                        state.wait_for(|s| {
                            s.members
                                .iter()
                                .any(|m| m.user_id == me && m.avatar == want)
                        }),
                    )
                    .await
                    .is_ok()
                }
            };
            assert!(has(Some(&hash)).await, "sent on joining");

            let bob = Camps::load(&bob_dir).unwrap();
            let theirs = bob.join(&link, "Боб").await.unwrap();
            let fetched = bob
                .member_avatar(&theirs.id, &opened.me.user_id, &hash)
                .await
                .unwrap();
            assert_eq!(fetched, picture);
            assert!(bob
                .member_avatar(&theirs.id, &opened.me.user_id, "../../device.key")
                .await
                .is_err());

            lena.set_avatar(None).await.unwrap();
            assert!(has(None).await, "dropped on the open camp at once");

            server.stop().await;
            assert_eq!(
                bob.member_avatar(&theirs.id, &opened.me.user_id, &hash)
                    .await
                    .unwrap(),
                picture,
                "from the cache, the camp being down"
            );
            for d in [lena_dir, bob_dir, server_dir] {
                let _ = fs::remove_dir_all(d);
            }
        });
    }

    /// ADR-0040: an open camp's address lets anyone in; closed again, it says so.
    #[test]
    fn an_open_camp_is_entered_by_its_address_alone() {
        tauri::async_runtime::block_on(async {
            let (server, server_dir) = camp("open").await;
            let address = server.links.open.to_string();
            let dir = temp_dir("open");
            let camps = Camps::load(&dir).unwrap();

            let closed = camps.preview(&address).await.unwrap();
            assert_eq!((closed.kind, closed.usable), (InviteKindDto::Open, false));
            assert!(camps
                .join(&address, "Лена")
                .await
                .unwrap_err()
                .contains("ссылку-приглашение"));

            server.manager().set_open(true).await.unwrap();
            assert!(camps.preview(&address).await.unwrap().usable);
            let opened = camps.join(&address, "Лена").await.unwrap();
            assert_eq!(
                opened.id,
                server.fingerprint.to_string(),
                "the same camp as by invite"
            );
            assert_eq!(opened.me.role, Role::Member);

            server.stop().await;
            let _ = fs::remove_dir_all(dir);
            let _ = fs::remove_dir_all(server_dir);
        });
    }

    #[test]
    fn leaving_tells_the_camp_and_drops_it_here_even_when_it_is_down() {
        tauri::async_runtime::block_on(async {
            let (server, server_dir) = camp("leave").await;
            let dir = temp_dir("leave");
            let camps = Camps::load(&dir).unwrap();
            let link = server.links.join.to_string();
            let opened = camps.join(&link, "Лена").await.unwrap();
            assert!(camps.is_current(&opened.id));

            assert!(camps.leave(&opened.id).await.unwrap(), "the camp heard it");
            assert!(camps.list().camps.is_empty());
            assert_eq!(camps.list().current, None);

            // Back with the invite, then the camp goes down: leaving still works here.
            let again = camps.join(&link, "Лена").await.unwrap();
            server.stop().await;
            assert!(!camps.leave(&again.id).await.unwrap(), "unreachable");
            assert!(camps.list().camps.is_empty(), "gone from the list anyway");

            let _ = fs::remove_dir_all(dir);
            let _ = fs::remove_dir_all(server_dir);
        });
    }

    #[test]
    fn bad_input_is_explained_before_any_network() {
        tauri::async_runtime::block_on(async {
            let dir = temp_dir("bad");
            let camps = Camps::load(&dir).unwrap();
            let link = "ourcamp://127.0.0.1:9/join/0000000000000000?fp=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
            assert!(camps
                .join(link, "   ")
                .await
                .unwrap_err()
                .contains("называть"));
            assert!(camps
                .join("hello", "Дима")
                .await
                .unwrap_err()
                .contains("ourcamp://"));
            let _ = fs::remove_dir_all(dir);
        });
    }

    #[test]
    fn links_are_explained_in_russian() {
        assert!(parse_link("https://example.com")
            .unwrap_err()
            .contains("ourcamp://"));
        assert!(parse_link("ourcamp://1.2.3.4/join/abc?fp=x")
            .unwrap_err()
            .contains("повреждена"));
    }
}
