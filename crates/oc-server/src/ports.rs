//! The camp's ports on the home router (ADR-0039): opened over UPnP when the owner asks
//! for the internet address, kept open while the camp runs, closed when it stops.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::upnp::{Gateway, Protocol};

/// How long the router keeps a mapping we asked for; renewed well before.
const LEASE: Duration = Duration::from_secs(60 * 60);
const RENEW_EVERY: Duration = Duration::from_secs(30 * 60);
/// After a refusal or an unreachable router, the next try.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// The self-check from inside: a router that loops back answers at once.
const LOOPBACK_CHECK: Duration = Duration::from_secs(2);

/// One port the camp needs open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Port {
    /// TCP or UDP.
    pub protocol: Protocol,
    /// The same number outside and in.
    pub number: u16,
}

/// Where the camp's ports stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortsState {
    /// The owner did not ask for the internet address: nothing touched.
    Off,
    /// Asking the router.
    Opening,
    /// Open on the router to this PC.
    Open {
        /// The router's public address (where friends come from).
        external_ip: Ipv4Addr,
        /// The camp's own port answered at the public address from inside: `Some(true)`.
        /// `None`: the router does not loop back, so only a friend can tell.
        answers: Option<bool>,
    },
    /// The router refused or is gone; tried again shortly.
    Failed(String),
}

/// Keeps `ports` open on `gateway` until `shutdown`, then closes them.
pub(crate) fn spawn(
    gateway: Gateway,
    ports: Vec<Port>,
    mut shutdown: watch::Receiver<bool>,
) -> (watch::Receiver<PortsState>, JoinHandle<()>) {
    let (state, rx) = watch::channel(PortsState::Opening);
    let task = tokio::spawn(async move {
        let mut opened = false;
        loop {
            let wait = match open_all(&gateway, &ports).await {
                Ok(()) => {
                    opened = true;
                    let external_ip = gateway.external_ip().await.ok();
                    let answers = match (external_ip, first_tcp(&ports)) {
                        (Some(ip), Some(port)) => answers_at(SocketAddr::from((ip, port))).await,
                        _ => None,
                    };
                    state.send_replace(match external_ip {
                        Some(external_ip) => PortsState::Open {
                            external_ip,
                            answers,
                        },
                        None => PortsState::Failed("роутер не назвал внешний адрес".into()),
                    });
                    RENEW_EVERY
                }
                Err(why) => {
                    eprintln!("[oc-server] ports: {why}");
                    state.send_replace(PortsState::Failed(why));
                    RETRY_AFTER
                }
            };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = shutdown.wait_for(|stop| *stop) => break,
            }
        }
        if opened {
            for port in &ports {
                if let Err(e) = gateway.remove_port(port.protocol, port.number).await {
                    eprintln!("[oc-server] closing port {}: {e}", port.number);
                }
            }
        }
    });
    (rx, task)
}

/// Opens every port; the first refusal is said in words for the window.
async fn open_all(gateway: &Gateway, ports: &[Port]) -> Result<(), String> {
    for port in ports {
        gateway
            .add_port(port.protocol, port.number, LEASE)
            .await
            .map_err(|e| match e {
                crate::upnp::UpnpError::Refused { code: 718, .. } => format!(
                    "порт {} на роутере уже занят другим устройством",
                    port.number
                ),
                other => format!("роутер не открыл порт {}: {other}", port.number),
            })?;
    }
    Ok(())
}

fn first_tcp(ports: &[Port]) -> Option<u16> {
    ports
        .iter()
        .find(|p| p.protocol == Protocol::Tcp)
        .map(|p| p.number)
}

/// Whether our port answers at the public address from inside. Only a yes means
/// something: many routers do not loop back, and then a friend outside is the check.
async fn answers_at(public: SocketAddr) -> Option<bool> {
    matches!(
        tokio::time::timeout(LOOPBACK_CHECK, TcpStream::connect(public)).await,
        Ok(Ok(_))
    )
    .then_some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_tcp_port_is_the_one_checked() {
        let ports = [
            Port {
                protocol: Protocol::Udp,
                number: 7882,
            },
            Port {
                protocol: Protocol::Tcp,
                number: 7780,
            },
        ];
        assert_eq!(first_tcp(&ports), Some(7780));
        assert_eq!(first_tcp(&ports[..1]), None);
    }

    #[tokio::test]
    async fn only_an_answer_counts_from_inside() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        assert_eq!(answers_at(listener.local_addr().unwrap()).await, Some(true));
        let closed = listener.local_addr().unwrap();
        drop(listener);
        assert_eq!(
            answers_at(closed).await,
            None,
            "no answer is no proof of a closed port"
        );
    }
}
