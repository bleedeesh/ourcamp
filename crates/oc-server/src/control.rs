//! The owner's hand on a running camp from the command line (ADR-0041).
//!
//! ```text
//! oc-server role Лена admin ─► control.json (port, key) ─► 127.0.0.1:<port>  POST /command
//!                                                           └─ Manager (as the server window)
//! ```
//!
//! The running camp listens on loopback only, on a port of its own choosing, and writes
//! the port and a fresh secret into the data directory: whoever can read that directory
//! owns the camp — the same person who could copy its database. Commands go through
//! [`Manager`], so they act at once and fully: a kick tells the person, a silence takes
//! their microphone down in LiveKit, a role shows in everyone's tree.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HOST};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use oc_proto::{CampState, ChannelKind, Role};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::manage::{ManageError, Manager};

/// Where a running camp says how to reach its control.
pub const CONTROL_FILE: &str = "control.json";
/// A command must be answered within this.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a command waits for the camp's snapshot to show it: a LiveKit round
/// (up to 2 s when it is slow) and the snapshot itself. Well within `CALL_TIMEOUT`.
const SETTLE: Duration = Duration::from_secs(5);

/// The contents of [`CONTROL_FILE`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlFile {
    /// Loopback port of the control.
    pub port: u16,
    /// Bearer secret, new on every start.
    pub key: String,
}

/// One thing the owner asks of the camp. People and channels are named by id or by
/// name (any case); a name two people share is refused with both ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum Command {
    /// Gives someone a role.
    Role {
        /// Id or name.
        who: String,
        /// The new role.
        role: Role,
    },
    /// Renames the camp.
    Rename {
        /// The new name.
        name: String,
    },
    /// Adds a voice channel, or a chat with `text`.
    AddChannel {
        /// Its name.
        name: String,
        /// A chat rather than a voice channel.
        text: bool,
    },
    /// Renames a channel.
    RenameChannel {
        /// Id or name.
        channel: String,
        /// The new name.
        name: String,
    },
    /// Removes a channel; those sitting there leave voice.
    RemoveChannel {
        /// Id or name.
        channel: String,
    },
    /// Kicks someone out (no ban).
    Kick {
        /// Id or name.
        who: String,
    },
    /// Forbids someone to speak, or lifts it.
    Silence {
        /// Id or name.
        who: String,
        /// Forbidden (`true`) or allowed again.
        silenced: bool,
    },
    /// Moves someone in voice to another voice channel.
    Move {
        /// Id or name.
        who: String,
        /// Id or name of a voice channel.
        channel: String,
    },
    /// Opens the camp to anyone with its address, or back to invites only.
    Open {
        /// Open (`true`) or invites only.
        open: bool,
    },
}

/// What [`Command`]s answer: a sentence for the owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    /// Done: what happened. Refused: why.
    pub text: String,
}

/// `GET /status`: the camp now, as the owner reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The snapshot friends' apps get too.
    pub state: CampState,
    /// The link to hand out: the address while open, the invite otherwise.
    pub link: String,
}

#[derive(Clone)]
struct Control {
    manager: Manager,
    camp: watch::Receiver<Arc<CampState>>,
    join_link: String,
    open_link: String,
    key: Arc<str>,
}

/// Starts the control and writes [`CONTROL_FILE`]; on `shutdown` it stops and the file
/// goes with it.
pub(crate) async fn start(
    data_dir: &Path,
    manager: Manager,
    camp: watch::Receiver<Arc<CampState>>,
    links: (String, String),
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<JoinHandle<()>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(std::io::Error::other)?;
    let key = oc_proto::encode_bytes(&key);
    let file = data_dir.join(CONTROL_FILE);
    write_private(
        &file,
        &serde_json::to_vec_pretty(&ControlFile {
            port,
            key: key.clone(),
        })
        .map_err(std::io::Error::other)?,
    )?;
    let control = Control {
        manager,
        camp,
        join_link: links.0,
        open_link: links.1,
        key: Arc::from(key),
    };
    let app = Router::new()
        .route("/status", get(status))
        .route("/command", post(command))
        .with_state(control);
    Ok(tokio::spawn(async move {
        let stop = async move {
            let _ = shutdown.wait_for(|stop| *stop).await;
        };
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(stop)
            .await
        {
            eprintln!("[oc-server] control: {e}");
        }
        let _ = std::fs::remove_file(&file);
    }))
}

/// The file only its owner may read: it carries the key to the camp.
fn write_private(path: &PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(bytes)?;
    }
    // On Windows a user's own profile folder (where the data lives) is theirs already.
    #[cfg(not(unix))]
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn refused(status: StatusCode, text: impl Into<String>) -> Response {
    (status, Json(Answer { text: text.into() })).into_response()
}

/// The request carries the key of this run.
fn allowed(control: &Control, headers: &HeaderMap) -> bool {
    let given = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    same_secret(given.as_bytes(), control.key.as_bytes())
}

/// Equal, in a time that does not depend on where the inputs first differ.
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn status(State(control): State<Control>, headers: HeaderMap) -> Response {
    if !allowed(&control, &headers) {
        return refused(StatusCode::UNAUTHORIZED, "неверный ключ управления");
    }
    let state = control.camp.borrow().as_ref().clone();
    let link = if state.open {
        control.open_link.clone()
    } else {
        control.join_link.clone()
    };
    Json(Status { state, link }).into_response()
}

/// The one person `who` names: an id, or a name (any case) nobody else has.
fn person(state: &CampState, who: &str) -> Result<(String, String), String> {
    if let Some(m) = state.members.iter().find(|m| m.user_id == who) {
        return Ok((m.user_id.clone(), m.name.clone()));
    }
    let wanted = who.trim().to_lowercase();
    let found: Vec<_> = state
        .members
        .iter()
        .filter(|m| m.name.to_lowercase() == wanted)
        .collect();
    match found.as_slice() {
        [one] => Ok((one.user_id.clone(), one.name.clone())),
        [] => Err(format!("на сервере нет «{who}»")),
        many => Err(format!(
            "«{who}» зовут нескольких — укажите id: {}",
            many.iter()
                .map(|m| m.user_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The one channel `channel` names: an id, or a name (any case).
fn channel(state: &CampState, channel: &str) -> Result<(String, String), String> {
    if let Some(c) = state.channels.iter().find(|c| c.id == channel) {
        return Ok((c.id.clone(), c.name.clone()));
    }
    let wanted = channel.trim().to_lowercase();
    let found: Vec<_> = state
        .channels
        .iter()
        .filter(|c| c.name.to_lowercase() == wanted)
        .collect();
    match found.as_slice() {
        [one] => Ok((one.id.clone(), one.name.clone())),
        [] => Err(format!("нет канала «{channel}»")),
        many => Err(format!(
            "каналов «{channel}» несколько — укажите id: {}",
            many.iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn managed(e: &ManageError) -> String {
    match e {
        ManageError::NotFound => "такого участника или канала уже нет".into(),
        ManageError::BadName(max) => format!("название — от 1 до {max} символов"),
        ManageError::AfkIsPermanent => {
            "канал AFK не удаляется — его можно только переименовать".into()
        }
        ManageError::NotInVoice => "этот человек сейчас не в голосовом канале".into(),
        ManageError::NotACampfire => "перенести можно только в голосовой канал".into(),
        ManageError::Store(e) => format!("не удалось сохранить: {e}"),
        ManageError::RoomNotClosed(e) | ManageError::LiveKit(e) => {
            format!("сохранено, но голосовой сервер не ответил: {e}")
        }
    }
}

/// A role as people say it.
#[must_use]
pub const fn role_name(role: Role) -> &'static str {
    match role {
        Role::Member => "участник",
        Role::Moderator => "модератор",
        Role::Admin => "админ",
    }
}

/// Carries out `command`; the sentence to print, or why not.
async fn run(control: &Control, command: Command) -> Result<String, String> {
    let state = control.camp.borrow().clone();
    let m = &control.manager;
    let done = |r: Result<(), ManageError>, text: String| r.map(|()| text).map_err(|e| managed(&e));
    match command {
        Command::Role { who, role } => {
            let (id, name) = person(&state, &who)?;
            done(
                m.set_role(&id, role).await,
                format!("{name} — теперь {}", role_name(role)),
            )
        }
        Command::Rename { name } => {
            let shown = name.trim().to_owned();
            done(
                m.rename_camp(&name).await,
                format!("сервер называется «{shown}»"),
            )
        }
        Command::AddChannel { name, text } => {
            let kind = if text {
                ChannelKind::Text
            } else {
                ChannelKind::Voice
            };
            m.create_channel(&name, kind)
                .await
                .map(|c| format!("добавлен канал «{}» ({})", c.name, c.id))
                .map_err(|e| managed(&e))
        }
        Command::RenameChannel {
            channel: which,
            name,
        } => {
            let (id, old) = channel(&state, &which)?;
            let shown = name.trim().to_owned();
            done(
                m.rename_channel(&id, &name).await,
                format!("«{old}» теперь «{shown}»"),
            )
        }
        Command::RemoveChannel { channel: which } => {
            let (id, old) = channel(&state, &which)?;
            done(m.delete_channel(&id).await, format!("канал «{old}» удалён"))
        }
        Command::Kick { who } => {
            let (id, name) = person(&state, &who)?;
            done(
                m.kick(&id).await,
                format!("{name} выгнан — вернуться можно по ссылке для друзей"),
            )
        }
        Command::Silence { who, silenced } => {
            let (id, name) = person(&state, &who)?;
            let text = if silenced {
                format!("{name}: говорить запрещено")
            } else {
                format!("{name}: снова может говорить")
            };
            done(m.set_silenced(&id, silenced).await, text)
        }
        Command::Move { who, channel: to } => {
            let (id, name) = person(&state, &who)?;
            let (channel_id, channel_name) = channel(&state, &to)?;
            done(
                m.move_member(&id, &channel_id).await,
                format!("{name} переходит в «{channel_name}»"),
            )
        }
        Command::Open { open } => {
            let text = if open {
                "сервер открытый: зайдёт любой, у кого есть адрес"
            } else {
                "сервер по приглашению: новым нужна ссылка для друзей"
            };
            done(m.set_open(open).await, text.into())
        }
    }
}

async fn command(
    State(control): State<Control>,
    headers: HeaderMap,
    body: Result<Json<Command>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if !allowed(&control, &headers) {
        return refused(StatusCode::UNAUTHORIZED, "неверный ключ управления");
    }
    let Ok(Json(command)) = body else {
        return refused(StatusCode::BAD_REQUEST, "непонятная команда");
    };
    match run(&control, command).await {
        Ok(text) => {
            // Answer once the camp shows the change: `role …; status` in a script, or a
            // channel added then named in the next command, see it. Past `SETTLE` the
            // change is still saved and will show up; the answer just does not wait.
            let _ = tokio::time::timeout(SETTLE, control.manager.settled()).await;
            Json(Answer { text }).into_response()
        }
        Err(text) => refused(StatusCode::CONFLICT, text),
    }
}

// ---- the command line's side ----

/// Why a command did not reach the camp, or what the camp refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// No [`CONTROL_FILE`]: the camp is not running in this data directory.
    NotRunning,
    /// It was there, but nothing answers (a camp that crashed).
    Unreachable(String),
    /// The camp refused: its words.
    Refused(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRunning => f.write_str(
                "сервер в этой папке данных не запущен (запустите oc-server, или укажите --data)",
            ),
            Self::Unreachable(why) => write!(f, "сервер не отвечает: {why}"),
            Self::Refused(why) => f.write_str(why),
        }
    }
}

/// A running camp's control, found by its data directory.
pub struct ControlClient {
    port: u16,
    key: String,
}

impl ControlClient {
    /// Reads [`CONTROL_FILE`] in `data_dir`.
    ///
    /// # Errors
    /// [`CallError::NotRunning`] without one.
    pub fn find(data_dir: &Path) -> Result<Self, CallError> {
        let bytes =
            std::fs::read(data_dir.join(CONTROL_FILE)).map_err(|_| CallError::NotRunning)?;
        let file: ControlFile =
            serde_json::from_slice(&bytes).map_err(|e| CallError::Unreachable(e.to_string()))?;
        Ok(Self {
            port: file.port,
            key: file.key,
        })
    }

    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        method: Method,
        path: &str,
        body: String,
    ) -> Result<T, CallError> {
        let unreachable = |e: &dyn std::fmt::Display| CallError::Unreachable(e.to_string());
        let exchange = async {
            let tcp = TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)))
                .await
                .map_err(|e| unreachable(&e))?;
            let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tcp))
                .await
                .map_err(|e| unreachable(&e))?;
            tokio::spawn(connection);
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header(HOST, "localhost")
                .header(CONTENT_TYPE, "application/json")
                .header(AUTHORIZATION, format!("Bearer {}", self.key))
                .body(Full::new(Bytes::from(body)))
                .map_err(|e| unreachable(&e))?;
            let response = sender
                .send_request(request)
                .await
                .map_err(|e| unreachable(&e))?;
            let ok = response.status().is_success();
            let bytes = response
                .into_body()
                .collect()
                .await
                .map_err(|e| unreachable(&e))?
                .to_bytes();
            if ok {
                serde_json::from_slice(&bytes).map_err(|e| unreachable(&e))
            } else {
                let answer: Answer = serde_json::from_slice(&bytes).map_err(|e| unreachable(&e))?;
                Err(CallError::Refused(answer.text))
            }
        };
        tokio::time::timeout(CALL_TIMEOUT, exchange)
            .await
            .map_err(|_| unreachable(&"no answer"))?
    }

    /// The camp now.
    ///
    /// # Errors
    /// See [`CallError`].
    pub async fn status(&self) -> Result<Status, CallError> {
        self.call(Method::GET, "/status", String::new()).await
    }

    /// Carries out `command`; what happened, in words.
    ///
    /// # Errors
    /// See [`CallError`]; [`CallError::Refused`] says why in words.
    pub async fn send(&self, command: &Command) -> Result<String, CallError> {
        let body =
            serde_json::to_string(command).map_err(|e| CallError::Unreachable(e.to_string()))?;
        self.call::<Answer>(Method::POST, "/command", body)
            .await
            .map(|a| a.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_proto::{Channel, MemberInfo};

    fn state() -> CampState {
        let member = |id: &str, name: &str| MemberInfo {
            user_id: id.into(),
            name: name.into(),
            role: Role::Member,
            silenced: false,
            avatar: None,
        };
        let voice = |id: &str, name: &str| Channel {
            id: id.into(),
            name: name.into(),
            kind: ChannelKind::Voice,
            afk: false,
        };
        CampState {
            name: "x".into(),
            channels: vec![
                voice("c0", "Voice"),
                voice("c7", "Катка"),
                voice("c8", "катка"),
            ],
            members: vec![
                member("u1", "Лена"),
                member("u2", "Боб"),
                member("u3", "боб"),
            ],
            presence: Vec::new(),
            open: false,
        }
    }

    #[test]
    fn people_and_channels_are_named_by_id_or_by_a_name_of_their_own() {
        let s = state();
        assert_eq!(
            person(&s, "лена").unwrap(),
            ("u1".into(), "Лена".into()),
            "any case"
        );
        assert_eq!(person(&s, "u2").unwrap().1, "Боб", "by id");
        let shared = person(&s, "БОБ").unwrap_err();
        assert!(shared.contains("u2, u3"), "two people share it: {shared}");
        assert!(person(&s, "Артём").unwrap_err().contains("нет"));
        assert_eq!(channel(&s, "voice").unwrap().0, "c0");
        assert!(channel(&s, "Катка").unwrap_err().contains("c7, c8"));
    }

    #[test]
    fn keys_compare_whole() {
        assert!(same_secret(b"abc", b"abc"));
        assert!(!same_secret(b"abc", b"abd"));
        assert!(!same_secret(b"abc", b"ab"), "a prefix is no key");
        assert!(!same_secret(b"", b"abc"));
    }

    #[test]
    fn commands_read_as_json_the_cli_sends() {
        let json = serde_json::to_value(Command::AddChannel {
            name: "Катка".into(),
            text: true,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"do": "add-channel", "name": "Катка", "text": true})
        );
        let back: Command = serde_json::from_value(
            serde_json::json!({"do": "role", "who": "Лена", "role": "admin"}),
        )
        .unwrap();
        assert_eq!(
            back,
            Command::Role {
                who: "Лена".into(),
                role: Role::Admin
            }
        );
    }
}
