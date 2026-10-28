//! # oc-server
//!
//! The camp a user hosts (ADR-0003, ADR-0016): HTTP API over TLS on one port, the
//! camp database, invites and the one-time admin claim key.
//!
//! The crate has no UI. `src/main.rs` runs it headless (links go to the log); the
//! Tauri server window (M3, step 6) embeds the same [`start`].
//!
//! ```text
//! client ──TLS 7780 (pinned, ADR-0014)──► serve ─► axum: /api/v1/…
//!                                                    └─► Store (SQLite, <data>/camp.db)
//! ```

mod api;
mod livekit;
mod manage;
mod presence;
mod rooms;
mod rtc;
mod serve;
pub mod store;
mod tls;

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use oc_invite::{Fingerprint, Host, Invite, InviteKind, Token};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use livekit::Supervisor;
pub use livekit::{
    find_binary as find_livekit, ManagedLiveKit, DEFAULT_RTC_TCP_PORT, DEFAULT_RTC_UDP_PORT,
};
pub use manage::{ManageError, Manager, MAX_CAMP_NAME, MAX_CHANNEL_NAME};
pub use oc_proto::API_VERSION;
use store::Store;
use tls::Identity;

/// How to run the camp.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where the database and TLS identity live. Back it up: losing it changes the
    /// fingerprint and invalidates every link.
    pub data_dir: PathBuf,
    /// Listen address (`0.0.0.0:7780` for a real camp, port 0 in tests).
    pub bind: SocketAddr,
    /// Address friends use, written into links.
    pub public_host: Host,
    /// Port written into links; `None` = the port actually bound.
    pub public_port: Option<u16>,
    /// Name of a brand-new camp (ignored once the camp exists).
    pub camp_name: String,
    /// Where voice rooms live.
    pub livekit: LiveKitMode,
}

/// The two LiveKit modes of ADR-0016.
#[derive(Debug, Clone)]
pub enum LiveKitMode {
    /// One already running (development: `pnpm livekit` in Docker, ADR-0013).
    External(LiveKit),
    /// The camp starts and watches its own (ADR-0022).
    Managed(ManagedLiveKit),
}

impl From<LiveKit> for LiveKitMode {
    fn from(livekit: LiveKit) -> Self {
        Self::External(livekit)
    }
}

/// A LiveKit server rooms live on.
#[derive(Clone)]
pub struct LiveKit {
    /// Where the camp reaches LiveKit: `ws://host:port`, normally loopback. Clients
    /// never see it — they reach LiveKit through `/rtc` on the camp port (ADR-0019).
    pub url: String,
    /// API key LiveKit was started with.
    pub api_key: String,
    /// Secret for that key; signs room tokens.
    pub api_secret: String,
}

// The secret must never reach logs or panic messages.
impl fmt::Debug for LiveKit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveKit")
            .field("url", &self.url)
            .field("api_key", &self.api_key)
            .finish_non_exhaustive()
    }
}

/// The two links the host hands out.
#[derive(Debug, Clone)]
pub struct Links {
    /// For friends. Roles are given in the server window, not by links (ADR-0026).
    pub join: Invite,
}

/// Why the server did not start.
#[derive(Debug)]
pub enum StartError {
    /// Database could not be opened or migrated.
    Store(store::StoreError),
    /// TLS identity could not be loaded or created.
    Identity(io::Error),
    /// TLS identity is corrupted.
    Tls(rustls::Error),
    /// Port in use or not permitted.
    Bind(SocketAddr, io::Error),
    /// `LiveKit::url` is not `ws://host:port`.
    LiveKitUrl(String),
    /// Managed LiveKit did not start; the text says why (with the end of its log).
    LiveKit(String),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(e) => write!(f, "{e}"),
            Self::Identity(e) => write!(f, "TLS identity: {e}"),
            Self::Tls(e) => write!(f, "TLS identity is invalid: {e}"),
            Self::Bind(addr, e) => write!(f, "cannot listen on {addr}: {e}"),
            Self::LiveKitUrl(url) => write!(f, "LiveKit address must be ws://host:port, got {url}"),
            Self::LiveKit(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for StartError {}

/// A running camp.
pub struct Running {
    /// Address actually bound.
    pub local_addr: SocketAddr,
    /// Certificate fingerprint carried by every link.
    pub fingerprint: Fingerprint,
    /// Links to hand out.
    pub links: Links,
    /// Name and campfires, as at start (the server window labels presence with them).
    pub camp: store::Camp,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
    /// `Some` in `managed` mode.
    livekit: Option<Supervisor>,
    /// The snapshot clients get on `/api/v1/events` (ADR-0021).
    state: watch::Receiver<Arc<oc_proto::CampState>>,
    manager: Manager,
}

impl Running {
    /// The process id of the camp's own LiveKit (`managed` mode), `None` while it is
    /// being restarted or in `external` mode. For the server window and diagnostics.
    #[must_use]
    pub fn livekit_pid(&self) -> Option<u32> {
        self.livekit.as_ref().and_then(Supervisor::pid)
    }

    /// Changes of [`Self::livekit_pid`] (restarts); `None` in `external` mode.
    #[must_use]
    pub fn livekit_pid_changes(&self) -> Option<watch::Receiver<Option<u32>>> {
        self.livekit.as_ref().map(Supervisor::pid_changes)
    }

    /// Who is in the camp and at which campfire — the same snapshots members get on
    /// `/api/v1/events`. For the server window.
    #[must_use]
    pub fn camp_state(&self) -> watch::Receiver<Arc<oc_proto::CampState>> {
        self.state.clone()
    }

    /// The owner's hand on the camp: name, roles, channels (ADR-0026).
    #[must_use]
    pub fn manager(&self) -> Manager {
        self.manager.clone()
    }

    /// Stops accepting connections, waits for the accept loop to end, then stops the
    /// camp's own LiveKit.
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
        if let Some(livekit) = self.livekit {
            livekit.stop().await;
        }
    }
}

/// Opens the camp in `config.data_dir` and starts serving.
///
/// # Errors
/// See [`StartError`].
///
/// # Panics
/// Only if the startup task itself panicked (a bug), which is re-raised here.
pub async fn start(config: Config) -> Result<Running, StartError> {
    if let LiveKitMode::External(livekit) = &config.livekit {
        livekit_authority(&livekit.url)
            .ok_or_else(|| StartError::LiveKitUrl(livekit.url.clone()))?;
    }
    let data_dir = config.data_dir.clone();
    let camp_name = config.camp_name.clone();
    // Disk I/O and key generation: off the async workers.
    let (store, identity, initial) = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&data_dir).map_err(StartError::Identity)?;
        let store =
            Store::open(&data_dir.join("camp.db"), &camp_name).map_err(StartError::Store)?;
        let identity =
            Identity::load_or_create(&data_dir.join("tls")).map_err(StartError::Identity)?;
        let initial = presence::initial(&store).map_err(StartError::Store)?;
        Ok::<_, StartError>((store, identity, initial))
    })
    .await
    .expect("startup task panicked")?;

    let tls = identity.server_config().map_err(StartError::Tls)?;
    let listener = TcpListener::bind(config.bind)
        .await
        .map_err(|e| StartError::Bind(config.bind, e))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| StartError::Bind(config.bind, e))?;

    let link = |kind, token: Token| Invite {
        host: config.public_host.clone(),
        port: config.public_port.unwrap_or(local_addr.port()),
        kind,
        token,
        fingerprint: identity.fingerprint,
    };
    let links = Links {
        join: link(
            InviteKind::Join,
            store.join_token().map_err(StartError::Store)?,
        ),
    };
    let camp = store.camp().map_err(StartError::Store)?;

    // Last of all: nothing after it can fail for a managed LiveKit (whose address is
    // ours), so a started one is never left behind.
    let (livekit, supervisor) = match config.livekit {
        LiveKitMode::External(livekit) => (livekit, None),
        LiveKitMode::Managed(managed) => {
            let (supervisor, livekit) =
                Supervisor::start(&managed, &config.data_dir, &config.public_host)
                    .await
                    .map_err(StartError::LiveKit)?;
            (livekit, Some(supervisor))
        }
    };
    let livekit_authority = livekit_authority(&livekit.url)
        .ok_or_else(|| StartError::LiveKitUrl(livekit.url.clone()))?;

    let (shutdown, shutdown_rx) = watch::channel(false);
    let (events, events_rx) = watch::channel(Arc::new(initial));
    // Events for one member (ADR-0028); a few in flight at most, so 64 is plenty.
    let (directives, _) = tokio::sync::broadcast::channel(64);
    let state_rx = events_rx.clone();
    let store = Arc::new(std::sync::Mutex::new(store));
    let rooms = Arc::new(rooms::RoomService::new(
        livekit_authority.clone(),
        livekit.clone(),
    ));
    let refresh = Arc::new(tokio::sync::Notify::new());
    let manager = Manager::new(
        Arc::clone(&store),
        Arc::clone(&rooms),
        Arc::clone(&refresh),
        directives.clone(),
    );
    let state = api::AppState::new(
        store,
        identity.fingerprint,
        livekit,
        &livekit_authority,
        events_rx,
        shutdown_rx.clone(),
        directives,
        manager.clone(),
    );
    tokio::spawn(presence::run(
        state.store_handle(),
        rooms,
        events,
        refresh,
        shutdown_rx.clone(),
    ));
    let task = tokio::spawn(serve::serve(listener, tls, api::router(state), shutdown_rx));
    Ok(Running {
        local_addr,
        fingerprint: identity.fingerprint,
        links,
        camp,
        shutdown,
        task,
        livekit: supervisor,
        state: state_rx,
        manager,
    })
}

/// This machine's LAN address — the default for links. It is the source address the OS
/// would pick to reach the internet; `connect` on UDP only selects a route, no packet
/// is sent. Falls back to loopback without a network.
#[must_use]
pub fn lan_address() -> Host {
    let ip = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|s| {
            s.connect((std::net::Ipv4Addr::new(192, 0, 2, 1), 9))?; // TEST-NET-1, never routed
            s.local_addr()
        })
        .map(|a| a.ip());
    match ip {
        Ok(std::net::IpAddr::V4(v4)) if !v4.is_unspecified() => Host::Ipv4(v4),
        _ => Host::Ipv4(std::net::Ipv4Addr::LOCALHOST),
    }
}

/// `ws://host:port[/]` → `host:port`. Plain `ws` only: LiveKit sits next to the server;
/// TLS to clients is the camp's own (one certificate, one pin).
fn livekit_authority(url: &str) -> Option<String> {
    let rest = url.strip_prefix("ws://")?;
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = authority.rsplit_once(':')?;
    (!host.is_empty() && !authority.contains('/') && port.parse::<u16>().is_ok())
        .then(|| authority.to_owned())
}

#[cfg(test)]
mod tests {
    use super::livekit_authority;

    #[test]
    fn livekit_url_must_be_plain_ws_with_a_port() {
        assert_eq!(
            livekit_authority("ws://127.0.0.1:7880").as_deref(),
            Some("127.0.0.1:7880")
        );
        assert_eq!(
            livekit_authority("ws://livekit:7880/").as_deref(),
            Some("livekit:7880")
        );
        assert_eq!(
            livekit_authority("wss://127.0.0.1:7880"),
            None,
            "TLS is the camp's job"
        );
        assert_eq!(livekit_authority("ws://127.0.0.1"), None, "explicit port");
        assert_eq!(livekit_authority("ws://h:7880/rtc"), None, "no path");
        assert_eq!(livekit_authority("http://h:7880"), None);
    }
}
