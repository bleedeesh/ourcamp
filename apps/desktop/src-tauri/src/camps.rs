//! Camps this device has joined (ADR-0020), kept in the app's data directory:
//!
//! * `device.key` — the device's Ed25519 key, i.e. who you are in every camp (ADR-0015);
//! * `camps.json` — per camp: address, certificate pin, session, cached name and channels;
//!   plus your display name and the camp you were last in.
//!
//! Every network call goes through `oc-camp-client` (pinned TLS). Errors come back as
//! Russian text for the UI.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use oc_camp_client::{CampClient, ClientError, DeviceKey, Session};
use oc_invite::{Fingerprint, Host, Invite, InviteError};
use oc_proto::{CampEvent, CampState, Channel, ErrorCode, InviteKindDto, Role};
use serde::{Deserialize, Serialize};

use crate::voice::room::RoomAccess;

const KEY_FILE: &str = "device.key";
const CAMPS_FILE: &str = "camps.json";

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
    /// The certificate fingerprint: unique per camp and stable across addresses.
    id: String,
    host: String,
    port: u16,
    name: String,
    #[serde(default)]
    channels: Vec<Channel>,
    user_id: String,
    role: Role,
    /// Bearer token. A secret, like a browser cookie for this camp.
    session: String,
}

impl SavedCamp {
    fn client(&self) -> Result<CampClient, CampError> {
        let host: Host = self
            .host
            .parse()
            .map_err(|_| format!("Сохранённый адрес лагеря повреждён: {}", self.host))?;
        let fingerprint: Fingerprint = self
            .id
            .parse()
            .map_err(|_| "Сохранённый отпечаток лагеря повреждён".to_owned())?;
        Ok(CampClient::at(&host, self.port, fingerprint))
    }

    fn session(&self) -> Session {
        Session::restore(self.session.clone(), self.user_id.clone(), self.role)
    }
}

/// A camp in the switcher.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CampSummary {
    /// Stable id (certificate fingerprint).
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
    /// Holder of the admin key.
    pub is_admin: bool,
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
}

/// The device key and the joined camps.
pub struct Camps {
    file: PathBuf,
    key: DeviceKey,
    saved: Mutex<Saved>,
    /// Where live camp state goes (the app forwards it to the webview).
    listener: Mutex<Option<StateListener>>,
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
            key,
            saved: Mutex::new(saved),
            listener: Mutex::new(None),
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
            .map_err(|e| format!("Не удалось сохранить список лагерей: {e}"))
    }

    fn camp(&self, id: &str) -> Result<SavedCamp, CampError> {
        self.lock()
            .camps
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| "Этот лагерь больше не в списке".to_owned())
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
        let invite = parse_link(link)?;
        let client = CampClient::new(&invite);
        let info = client.camp().await.map_err(describe)?;
        let status = client
            .invite(invite.token.as_str())
            .await
            .map_err(|e| match e {
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
        let invite = parse_link(link)?;
        let client = CampClient::new(&invite);
        let session = client
            .login(&self.key, name, Some(invite.token.as_str()))
            .await
            .map_err(describe)?;
        let info = client.camp().await.map_err(describe)?;
        let id = invite.fingerprint.to_string();
        let camp = SavedCamp {
            id: id.clone(),
            host: invite.host.to_string(),
            port: invite.port,
            name: info.name,
            channels: info.channels,
            user_id: session.user_id.clone(),
            role: session.role,
            session: session.token().to_owned(),
        };
        self.update(|saved| {
            name.clone_into(&mut saved.display_name);
            saved.camps.retain(|c| c.id != id);
            saved.camps.push(camp.clone());
            saved.current = Some(id);
        })?;
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
        Ok(self.opened(&camp, offline))
    }

    fn opened(&self, camp: &SavedCamp, offline: bool) -> OpenCamp {
        OpenCamp {
            id: camp.id.clone(),
            name: camp.name.clone(),
            channels: camp.channels.clone(),
            me: SelfInfo {
                user_id: camp.user_id.clone(),
                name: self.lock().display_name.clone(),
                is_admin: camp.role == Role::Admin,
            },
            offline,
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

    /// Where live camp state goes.
    pub fn set_state_listener(&self, listener: StateListener) {
        *self.listener.lock().unwrap_or_else(PoisonError::into_inner) = Some(listener);
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

fn parse_link(link: &str) -> Result<Invite, CampError> {
    link.parse().map_err(|e: InviteError| {
        match e {
            InviteError::NotAnInvite => "Это не ссылка OurCamp — она начинается с ourcamp://",
            InviteError::InvalidHost | InviteError::InvalidPort => "В ссылке неверный адрес лагеря",
            InviteError::InvalidPath | InviteError::InvalidToken => {
                "Ссылка повреждена — попросите прислать её заново"
            }
            InviteError::MissingFingerprint | InviteError::InvalidFingerprint => {
                "В ссылке нет отпечатка лагеря — попросите прислать её заново"
            }
        }
        .to_owned()
    })
}

/// A client error as a sentence for the UI.
fn describe(e: ClientError) -> CampError {
    match e {
        ClientError::Io(e) if e.to_string().contains("certificate") => {
            "Сертификат лагеря не совпадает со ссылкой: это может быть подмена, или лагерь \
             переустановлен — попросите новую ссылку"
                .into()
        }
        ClientError::Io(e) => format!("Лагерь недоступен: {e}"),
        ClientError::Api(code) => match code {
            ErrorCode::InviteRequired => "Для первого входа нужна ссылка-приглашение",
            ErrorCode::InviteInvalid => "Приглашение недействительно",
            ErrorCode::ClaimUsed => "Этот ключ админа уже использован",
            ErrorCode::BadSignature | ErrorCode::NonceInvalid => {
                "Не удалось подтвердить устройство — попробуйте ещё раз"
            }
            ErrorCode::Unauthorized => "Сессия в лагере истекла",
            ErrorCode::NotFound => "Такого костра в лагере нет",
            ErrorCode::BadRequest => "Лагерь не принял данные — проверьте имя (до 32 символов)",
            ErrorCode::Internal => "Ошибка на сервере лагеря",
        }
        .into(),
        ClientError::Protocol(m) => format!("Лагерь ответил непонятно: {m}"),
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
    fn a_damaged_camp_list_is_set_aside_not_fatal() {
        let dir = temp_dir("damaged");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(CAMPS_FILE), b"{ not json").unwrap();
        let camps = Camps::load(&dir).unwrap();
        assert!(camps.list().camps.is_empty());
        assert!(dir.join("camps.json.bad").exists(), "kept for inspection");
        let _ = fs::remove_dir_all(dir);
    }

    /// A camp server in this process (no LiveKit needed: nothing here connects voice).
    async fn camp(name: &str) -> (oc_server::Running, PathBuf) {
        let dir = temp_dir(&format!("server-{name}"));
        let running = oc_server::start(oc_server::Config {
            data_dir: dir.clone(),
            bind: (std::net::Ipv4Addr::LOCALHOST, 0).into(),
            public_host: Host::Ipv4(std::net::Ipv4Addr::LOCALHOST),
            public_port: None,
            camp_name: "Вечерний кооп".into(),
            livekit: oc_server::LiveKit {
                url: "ws://127.0.0.1:7880".into(),
                api_key: "devkey".into(),
                api_secret: "secret".into(),
            },
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
            let link = server.links.claim.as_ref().unwrap().to_string();

            let camps = Camps::load(&dir).unwrap();
            let preview = camps.preview(&link).await.unwrap();
            assert_eq!(preview.name, "Вечерний кооп");
            assert_eq!(preview.channels, 3);
            assert_eq!((preview.kind, preview.usable), (InviteKindDto::Claim, true));

            let opened = camps.join(&link, "  Дима  ").await.unwrap();
            assert_eq!(
                opened.id,
                server.fingerprint.to_string(),
                "camp id = its pin"
            );
            assert_eq!(opened.channels[0].name, "Лобби");
            assert_eq!(opened.me.name, "Дима", "trimmed");
            assert!(opened.me.is_admin, "the claim link made us admin");
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
                .contains("костра"));

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
            assert!(!joined.me.is_admin);
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

            let link = server.links.claim.as_ref().unwrap().to_string();
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
                        .is_some_and(|s| s.members.iter().any(|m| m.user_id == me && m.is_admin))),
                "a snapshot with us, crowned: {:?}",
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
