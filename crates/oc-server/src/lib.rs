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
mod rtc;
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
    /// `LiveKit::url` is not `ws://host:port`.
    LiveKitUrl(String),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(e) => write!(f, "{e}"),
            Self::Identity(e) => write!(f, "TLS identity: {e}"),
            Self::Tls(e) => write!(f, "TLS identity is invalid: {e}"),
            Self::Bind(addr, e) => write!(f, "cannot listen on {addr}: {e}"),
            Self::LiveKitUrl(url) => write!(f, "LiveKit address must be ws://host:port, got {url}"),
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
    let livekit_authority = livekit_authority(&config.livekit.url)
        .ok_or_else(|| StartError::LiveKitUrl(config.livekit.url.clone()))?;
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
        &livekit_authority,
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
