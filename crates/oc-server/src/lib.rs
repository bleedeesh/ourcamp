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
mod serve;
pub mod store;
mod tls;

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;

use oc_invite::{Fingerprint, Host, Invite, InviteKind, Token};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

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
    /// LiveKit the camp hands out room tokens for.
    pub livekit: LiveKit,
}

/// The LiveKit server rooms live on (ADR-0016: `external` mode in M3 step 2).
#[derive(Clone)]
pub struct LiveKit {
    /// Address put into room tokens for clients. Step 3 replaces it with the `/rtc`
    /// proxy on the camp port.
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
    /// For friends.
    pub join: Invite,
    /// For the host: makes whoever redeems it admin. `None` once redeemed.
    pub claim: Option<Invite>,
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
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(e) => write!(f, "{e}"),
            Self::Identity(e) => write!(f, "TLS identity: {e}"),
            Self::Tls(e) => write!(f, "TLS identity is invalid: {e}"),
            Self::Bind(addr, e) => write!(f, "cannot listen on {addr}: {e}"),
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
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl Running {
    /// Stops accepting connections and waits for the accept loop to end.
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
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
    let data_dir = config.data_dir.clone();
    let camp_name = config.camp_name.clone();
    // Disk I/O and key generation: off the async workers.
    let (store, identity) = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&data_dir).map_err(StartError::Identity)?;
        let store =
            Store::open(&data_dir.join("camp.db"), &camp_name).map_err(StartError::Store)?;
        let identity =
            Identity::load_or_create(&data_dir.join("tls")).map_err(StartError::Identity)?;
        Ok::<_, StartError>((store, identity))
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
        claim: store
            .unclaimed_admin_token()
            .map_err(StartError::Store)?
            .map(|t| link(InviteKind::ClaimAdmin, t)),
    };

    let (shutdown, shutdown_rx) = watch::channel(false);
    let app = api::router(api::AppState::new(
        store,
        identity.fingerprint,
        config.livekit.clone(),
    ));
    let task = tokio::spawn(serve::serve(listener, tls, app, shutdown_rx));
    Ok(Running {
        local_addr,
        fingerprint: identity.fingerprint,
        links,
        shutdown,
        task,
    })
}
