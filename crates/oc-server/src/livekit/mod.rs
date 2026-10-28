//! `managed` LiveKit (ADR-0016, ADR-0022): the camp runs livekit-server as its own child
//! process. The API key is generated on first start and kept with the camp; config and
//! keys go through the environment (never the command line, which other users can
//! read); signalling listens on a free loopback port only the camp knows. A crash is
//! followed by a restart; the camp stopping or dying takes LiveKit with it.
//!
//! ```text
//! camp ──spawn──► livekit-server   LIVEKIT_CONFIG (ports, node IP), LIVEKIT_KEYS
//!   │               ├─ 127.0.0.1:<free>  signalling + API (proxied as /rtc, polled)
//!   │               ├─ 7881/tcp, 7882/udp  media, all interfaces
//!   └─ Supervisor: restart with backoff · Windows job / unix lifeline: dies with us
//! ```

#[cfg(windows)]
mod job;
mod process;

use std::fmt::Write as _;
use std::io;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use oc_invite::Host;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::tls::write_atomically;
use crate::LiveKit;
use process::{Launch, Process};

/// Media ports of a camp: friends' traffic arrives here, so they are fixed (the host
/// forwards them on the router, M3 step 7).
pub const DEFAULT_RTC_TCP_PORT: u16 = 7881;
/// See [`DEFAULT_RTC_TCP_PORT`]. One UDP port for every peer (LiveKit muxes them).
pub const DEFAULT_RTC_UDP_PORT: u16 = 7882;

/// API key and secret, generated once per camp.
const KEYS_FILE: &str = "livekit-keys.json";
/// A fresh LiveKit must answer within this, or the start counts as failed.
const READY_TIMEOUT: Duration = Duration::from_secs(20);
/// Restart delays after a crash: doubling from the first to the last.
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// A run this long counts as healthy: the next crash restarts without delay growth.
const HEALTHY_RUN: Duration = Duration::from_secs(60);

/// How the camp runs its own LiveKit.
#[derive(Debug, Clone)]
pub struct ManagedLiveKit {
    /// The `livekit-server` executable ([`find_binary`] finds the usual one).
    pub binary: PathBuf,
    /// WebRTC over TCP, the fallback when UDP is blocked.
    pub rtc_tcp_port: u16,
    /// WebRTC over UDP.
    pub rtc_udp_port: u16,
}

impl ManagedLiveKit {
    /// `binary` on the default media ports.
    #[must_use]
    pub fn new(binary: PathBuf) -> Self {
        Self {
            binary,
            rtc_tcp_port: DEFAULT_RTC_TCP_PORT,
            rtc_udp_port: DEFAULT_RTC_UDP_PORT,
        }
    }
}

/// The usual `livekit-server`: next to this executable (where the installer puts it,
/// M8), in this checkout's `dev/bin` (debug builds, `pnpm livekit:fetch`), then on
/// `PATH` (macOS has no release binary: `brew install livekit`).
#[must_use]
pub fn find_binary() -> Option<PathBuf> {
    let name = format!("livekit-server{}", std::env::consts::EXE_SUFFIX);
    let beside_us = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)));
    let checkout = cfg!(debug_assertions).then(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../dev/bin")
            .join(&name)
    });
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(&name));
    beside_us
        .into_iter()
        .chain(checkout)
        .chain(on_path)
        .find(|path| path.is_file())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Keys {
    api_key: String,
    api_secret: String,
}

impl Keys {
    fn generate() -> io::Result<Self> {
        let mut key = [0u8; 6];
        let mut secret = [0u8; 32];
        getrandom::fill(&mut key).map_err(io::Error::other)?;
        getrandom::fill(&mut secret).map_err(io::Error::other)?;
        Ok(Self {
            api_key: format!("OC{}", hex(&key)),
            api_secret: hex(&secret),
        })
    }

    /// The camp's keys, created on first start. A damaged file is replaced: the keys
    /// never leave this machine, so new ones only cost tokens already handed out.
    fn load_or_create(dir: &Path) -> io::Result<Self> {
        let path = dir.join(KEYS_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(keys) => return Ok(keys),
                Err(e) => eprintln!("[oc-server] {KEYS_FILE} is damaged ({e}); new LiveKit keys"),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let keys = Self::generate()?;
        let json = serde_json::to_vec_pretty(&keys).map_err(io::Error::other)?;
        write_atomically(&path, &json)?;
        Ok(keys)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// LiveKit's config (YAML), built here rather than with a YAML library: every value is
/// a number or an IP address, nothing needs quoting.
fn config_body(signal_port: u16, managed: &ManagedLiveKit, node_ip: Option<Ipv4Addr>) -> String {
    let mut yaml = format!(
        "port: {signal_port}\n\
         bind_addresses: [127.0.0.1]\n\
         rtc:\n  tcp_port: {}\n  udp_port: {}\n  use_external_ip: false\n",
        managed.rtc_tcp_port, managed.rtc_udp_port
    );
    // The address put into ICE candidates: the one friends use for the camp. A domain
    // or IPv6 is left to LiveKit's own detection (public IPs arrive with step 7, UPnP).
    if let Some(ip) = node_ip {
        let _ = writeln!(yaml, "  node_ip: {ip}");
    }
    yaml.push_str("logging:\n  level: info\n");
    yaml
}

/// The address to advertise for media, from the address in the camp's links.
fn node_ip(public_host: &Host) -> Option<Ipv4Addr> {
    match public_host {
        Host::Ipv4(ip) if !ip.is_unspecified() => Some(*ip),
        _ => None,
    }
}

/// A free loopback port for signalling. Only the camp talks to it, so it need not be
/// stable across runs — and it never collides with a dev LiveKit on 7880.
fn free_loopback_port() -> io::Result<u16> {
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
}

/// Keeps LiveKit running until [`Supervisor::stop`].
pub struct Supervisor {
    stop: watch::Sender<bool>,
    pid: watch::Receiver<Option<u32>>,
    task: JoinHandle<()>,
}

impl Supervisor {
    /// Starts LiveKit and waits until it answers. Returns how the camp reaches it.
    pub async fn start(
        managed: &ManagedLiveKit,
        data_dir: &Path,
        public_host: &Host,
    ) -> Result<(Self, LiveKit), String> {
        let dir = data_dir.to_owned();
        let keys = tokio::task::spawn_blocking(move || Keys::load_or_create(&dir))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("LiveKit keys: {e}"))?;
        let signal_port =
            free_loopback_port().map_err(|e| format!("no free loopback port: {e}"))?;
        let launch = Arc::new(Launch {
            binary: managed.binary.clone(),
            config: config_body(signal_port, managed, node_ip(public_host)),
            keys: format!("{}: {}", keys.api_key, keys.api_secret),
            signal_port,
            media_ports: (managed.rtc_tcp_port, managed.rtc_udp_port),
        });

        let first = Process::spawn(&launch)
            .map_err(|e| format!("cannot start {}: {e}", launch.binary.display()))?;
        let first = first.ready(READY_TIMEOUT).await?;
        eprintln!(
            "[oc-server] LiveKit (managed): pid {}, signalling 127.0.0.1:{signal_port}, \
             media {}/tcp {}/udp",
            first.pid(),
            managed.rtc_tcp_port,
            managed.rtc_udp_port
        );

        let (stop, stop_rx) = watch::channel(false);
        let (pid_tx, pid) = watch::channel(Some(first.pid()));
        let task = tokio::spawn(supervise(launch, first, stop_rx, pid_tx));
        let livekit = LiveKit {
            url: format!("ws://127.0.0.1:{signal_port}"),
            api_key: keys.api_key,
            api_secret: keys.api_secret,
        };
        Ok((Self { stop, pid, task }, livekit))
    }

    /// The running LiveKit's process id; `None` while it is being restarted.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        *self.pid.borrow()
    }

    /// Changes of [`Self::pid`].
    #[must_use]
    pub fn pid_changes(&self) -> watch::Receiver<Option<u32>> {
        self.pid.clone()
    }

    /// Stops LiveKit and waits until it is gone.
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}

/// Until a stop is asked for. The read guard `wait_for` returns (`watch::Ref`, not
/// `Send`) is dropped here, so callers can `await` in the `select!` branch after it.
async fn stopping(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|&stopping| stopping).await;
}

async fn supervise(
    launch: Arc<Launch>,
    first: Process,
    mut stop: watch::Receiver<bool>,
    pid: watch::Sender<Option<u32>>,
) {
    let mut current = Some(first);
    let mut backoff = FIRST_BACKOFF;
    loop {
        if let Some(mut process) = current.take() {
            let started = tokio::time::Instant::now();
            tokio::select! {
                status = process.wait() => {
                    pid.send_replace(None);
                    if started.elapsed() >= HEALTHY_RUN {
                        backoff = FIRST_BACKOFF;
                    }
                    eprintln!(
                        "[oc-server] LiveKit exited ({status}); restarting in {}s",
                        backoff.as_secs()
                    );
                }
                () = stopping(&mut stop) => {
                    process.stop().await;
                    pid.send_replace(None);
                    return;
                }
            }
        }

        tokio::select! {
            () = tokio::time::sleep(backoff) => {}
            () = stopping(&mut stop) => return,
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);

        let ready = match Process::spawn(&launch) {
            Ok(process) => {
                tokio::select! {
                    ready = process.ready(READY_TIMEOUT) => ready,
                    () = stopping(&mut stop) => return,
                }
            }
            Err(e) => Err(format!("cannot start {}: {e}", launch.binary.display())),
        };
        match ready {
            Ok(process) => {
                eprintln!("[oc-server] LiveKit is back: pid {}", process.pid());
                pid.send_replace(Some(process.pid()));
                current = Some(process);
            }
            Err(e) => eprintln!("[oc-server] LiveKit restart failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oc-livekit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn keys_are_made_once_and_kept() {
        let dir = temp_dir("keys");
        let first = Keys::load_or_create(&dir).unwrap();
        assert!(first.api_key.starts_with("OC") && first.api_key.len() == 14);
        assert_eq!(first.api_secret.len(), 64, "32 random bytes, hex");
        let again = Keys::load_or_create(&dir).unwrap();
        assert_eq!(
            (again.api_key, again.api_secret),
            (first.api_key, first.api_secret)
        );
        let other = Keys::generate().unwrap();
        assert_ne!(
            other.api_secret,
            Keys::load_or_create(&dir).unwrap().api_secret
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn damaged_keys_are_replaced() {
        let dir = temp_dir("damaged");
        std::fs::write(dir.join(KEYS_FILE), b"{ nope").unwrap();
        let keys = Keys::load_or_create(&dir).unwrap();
        assert_eq!(keys.api_secret.len(), 64);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn config_keeps_signalling_on_loopback_and_media_on_fixed_ports() {
        let managed = ManagedLiveKit::new(PathBuf::from("livekit-server"));
        let yaml = config_body(
            50123,
            &managed,
            node_ip(&Host::Ipv4(Ipv4Addr::new(192, 168, 1, 2))),
        );
        assert_eq!(
            yaml,
            "port: 50123\nbind_addresses: [127.0.0.1]\n\
             rtc:\n  tcp_port: 7881\n  udp_port: 7882\n  use_external_ip: false\n  node_ip: 192.168.1.2\n\
             logging:\n  level: info\n"
        );
        let by_name = config_body(1, &managed, node_ip(&Host::Domain("camp.example".into())));
        assert!(!by_name.contains("node_ip"), "a domain is left to LiveKit");
    }
}
