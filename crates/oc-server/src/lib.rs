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
pub mod certificate;
pub mod control;
mod livekit;
mod manage;
pub mod network;
pub mod ports;
mod presence;
mod rooms;
mod rtc;
mod serve;
pub mod store;
mod tls;
pub mod upnp;

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use oc_invite::{Address, Fingerprint, Host, Invite, InviteKind};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use certificate::DomainCertificate;
pub use certificate::{CertificateError, CertificateFiles};
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
    /// The home router to open the camp's ports on (ADR-0039); `None`: leave it alone.
    pub router: Option<upnp::Gateway>,
    /// A certificate from an authority for `public_host`, a domain (ADR-0042): links then
    /// carry no fingerprint. `None`: only the camp's own self-signed certificate.
    pub certificate: Option<CertificateFiles>,
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
    /// The camp's address alone: lets anyone in while the camp is open (ADR-0040).
    pub open: Address,
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
    /// The certificate for the domain cannot be served.
    Certificate(CertificateError),
    /// A certificate was given, but friends are to come by an IP address.
    CertificateNeedsDomain(Host),
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
            Self::Certificate(e) => write!(f, "certificate: {e}"),
            Self::CertificateNeedsDomain(host) => write!(
                f,
                "a certificate is for a domain, but friends would come by {host}; set the \
                 domain with --host"
            ),
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
    /// The ports on the router (ADR-0039); closed by `stop`.
    ports: watch::Receiver<ports::PortsState>,
    ports_task: Option<JoinHandle<()>>,
    /// The owner's command line (ADR-0041); stops with the camp.
    control: Option<JoinHandle<()>>,
    /// Rereads the domain's certificate after a renewal (ADR-0042).
    certificate_task: Option<JoinHandle<()>>,
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

    /// Where the camp's ports on the router stand (ADR-0039).
    #[must_use]
    pub fn ports(&self) -> watch::Receiver<ports::PortsState> {
        self.ports.clone()
    }

    /// Stops accepting connections, waits for the accept loop to end, closes the ports
    /// on the router, then stops the camp's own LiveKit.
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
        // The router's mappings go before the process may: they would outlive it.
        if let Some(ports) = self.ports_task {
            let _ = ports.await;
        }
        if let Some(control) = self.control {
            let _ = control.await;
        }
        if let Some(certificate) = self.certificate_task {
            let _ = certificate.await;
        }
        if let Some(livekit) = self.livekit {
            livekit.stop().await;
        }
    }
}

/// What `open_camp` reads from disk.
struct Opened {
    store: Store,
    identity: Identity,
    initial: oc_proto::CampState,
    /// The authority's certificate for the domain, when one is given.
    domain: Option<Arc<DomainCertificate>>,
}

/// The camp's database, TLS identity, certificate for the domain and first snapshot.
/// Disk I/O and key generation run off the async workers.
async fn open_camp(config: &Config) -> Result<Opened, StartError> {
    let data_dir = config.data_dir.clone();
    let camp_name = config.camp_name.clone();
    let certificate = config.certificate.clone();
    let host = config.public_host.clone();
    tokio::task::spawn_blocking(move || {
        // The certificate first: a mistake in it is the owner's to fix before anything
        // is created.
        let domain = match (certificate, &host) {
            (None, _) => None,
            (Some(files), Host::Domain(domain)) => Some(Arc::new(
                DomainCertificate::open(files, domain).map_err(StartError::Certificate)?,
            )),
            (Some(_), _) => return Err(StartError::CertificateNeedsDomain(host)),
        };
        std::fs::create_dir_all(&data_dir).map_err(StartError::Identity)?;
        let store =
            Store::open(&data_dir.join("camp.db"), &camp_name).map_err(StartError::Store)?;
        let identity =
            Identity::load_or_create(&data_dir.join("tls")).map_err(StartError::Identity)?;
        let initial = presence::initial(&store).map_err(StartError::Store)?;
        Ok(Opened {
            store,
            identity,
            initial,
            domain,
        })
    })
    .await
    .expect("startup task panicked")
}

/// The links to hand out: to the domain without a fingerprint when it has an
/// authority's certificate (ADR-0042), else pinned.
fn links_for(
    config: &Config,
    port: u16,
    store: &Store,
    pinned: bool,
    fp: Fingerprint,
) -> Result<Links, StartError> {
    let fingerprint = pinned.then_some(fp);
    Ok(Links {
        join: Invite {
            host: config.public_host.clone(),
            port,
            kind: InviteKind::Join,
            token: store.join_token().map_err(StartError::Store)?,
            fingerprint,
        },
        open: Address {
            host: config.public_host.clone(),
            port,
            fingerprint,
        },
    })
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
    let Opened {
        store,
        identity,
        initial,
        domain,
    } = open_camp(&config).await?;

    let tls = identity
        .server_config(domain.clone())
        .map_err(StartError::Tls)?;
    let listener = TcpListener::bind(config.bind)
        .await
        .map_err(|e| StartError::Bind(config.bind, e))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| StartError::Bind(config.bind, e))?;

    let port = config.public_port.unwrap_or(local_addr.port());
    let links = links_for(
        &config,
        port,
        &store,
        domain.is_none(),
        identity.fingerprint,
    )?;
    // What a client signs when it checked the domain's certificate (ADR-0042).
    let named = domain
        .is_some()
        .then(|| (config.public_host.to_string(), port));
    let camp = store.camp().map_err(StartError::Store)?;
    let wanted = wanted_ports(local_addr.port(), &config.livekit);

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
    let refresh = Arc::new(presence::Refresh::new());
    let manager = Manager::new(
        Arc::clone(&store),
        Arc::clone(&rooms),
        Arc::clone(&refresh),
        directives.clone(),
    );
    let state = api::AppState::new(
        store,
        identity.fingerprint,
        named,
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
    let (ports, ports_task) = open_ports(config.router, wanted, &shutdown_rx);
    let control = start_control(&config.data_dir, &manager, &state_rx, &links, &shutdown_rx).await;
    let certificate_task = domain.map(|d| certificate::spawn_reload(d, shutdown_rx.clone()));
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
        ports,
        ports_task,
        control,
        certificate_task,
    })
}

/// Keeps the ports open on the router, when one was given (ADR-0039).
fn open_ports(
    router: Option<upnp::Gateway>,
    wanted: Vec<ports::Port>,
    shutdown: &watch::Receiver<bool>,
) -> (watch::Receiver<ports::PortsState>, Option<JoinHandle<()>>) {
    match router {
        Some(gateway) => {
            let (ports, task) = ports::spawn(gateway, wanted, shutdown.clone());
            (ports, Some(task))
        }
        None => (watch::channel(ports::PortsState::Off).1, None),
    }
}

/// The owner's command line (ADR-0041). Without it the camp still serves friends.
async fn start_control(
    data_dir: &std::path::Path,
    manager: &Manager,
    state: &watch::Receiver<Arc<oc_proto::CampState>>,
    links: &Links,
    shutdown: &watch::Receiver<bool>,
) -> Option<JoinHandle<()>> {
    control::start(
        data_dir,
        manager.clone(),
        state.clone(),
        (links.join.to_string(), links.open.to_string()),
        shutdown.clone(),
    )
    .await
    .map_err(|e| eprintln!("[oc-server] owner's control unavailable: {e}"))
    .ok()
}

/// The ports friends come in by (ADR-0039): the camp's own, and voice's when the
/// LiveKit is the camp's (an external one keeps its own ports).
fn wanted_ports(camp_port: u16, livekit: &LiveKitMode) -> Vec<ports::Port> {
    let mut wanted = vec![ports::Port {
        protocol: upnp::Protocol::Tcp,
        number: camp_port,
    }];
    if let LiveKitMode::Managed(managed) = livekit {
        wanted.push(ports::Port {
            protocol: upnp::Protocol::Tcp,
            number: managed.rtc_tcp_port,
        });
        wanted.push(ports::Port {
            protocol: upnp::Protocol::Udp,
            number: managed.rtc_udp_port,
        });
    }
    wanted
}

/// This machine's LAN address — the default for links. It is the source address the OS
/// would pick to reach the internet; `connect` on UDP only selects a route, no packet
/// is sent. Falls back to loopback without a network.
#[must_use]
pub fn lan_address() -> Host {
    Host::Ipv4(network::default_address().unwrap_or(std::net::Ipv4Addr::LOCALHOST))
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
