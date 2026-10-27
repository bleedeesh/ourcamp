//! TLS accept loop: TCP → rustls → hyper (HTTP/1.1, HTTP/2 and connection upgrades)
//! → the axum router.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use rustls::ServerConfig;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

/// A client that connects but never finishes the handshake must not hold a task forever.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Serves `app` over TLS until `shutdown` turns true.
pub async fn serve(
    listener: TcpListener,
    tls: Arc<ServerConfig>,
    app: Router,
    mut shutdown: watch::Receiver<bool>,
) {
    let acceptor = TlsAcceptor::from(tls);
    loop {
        let (tcp, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(conn) => conn,
                Err(e) => {
                    // Usually EMFILE or a reset during accept; keep serving others.
                    eprintln!("[oc-server] accept: {e}");
                    continue;
                }
            },
            _ = shutdown.changed() => break,
        };
        let acceptor = acceptor.clone();
        let app = app.clone();
        tokio::spawn(async move {
            // Wrong pin on the client side, port scanners, timeouts: not our problem.
            let Ok(Ok(tls)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await
            else {
                return;
            };
            let service = TowerToHyperService::new(app);
            if let Err(e) = auto::Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(TokioIo::new(tls), service)
                .await
            {
                eprintln!("[oc-server] connection {peer}: {e}");
            }
        });
    }
}
