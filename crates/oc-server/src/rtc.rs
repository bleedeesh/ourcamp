//! `/rtc*` → LiveKit: the signalling WebSocket (and its `…/validate` probe) on the camp's
//! own TLS port (ADR-0016, ADR-0019). Clients pin one certificate for everything; LiveKit
//! listens on loopback and is never exposed with its full HTTP API.
//!
//! ```text
//! client ──wss://camp:7780/rtc/v1?access_token=…──► rtc::proxy ──ws://127.0.0.1:7880/rtc/v1…──► LiveKit
//!          (HTTP/1.1 Upgrade)                         101 both ways, then raw byte copy
//! ```

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::api::AppState;

/// Hop-by-hop headers (RFC 9110 §7.6.1) are about one connection and are not
/// forwarded — except `Connection`/`Upgrade` on an upgrade, which must reach LiveKit.
const HOP_BY_HOP: [header::HeaderName; 5] = [
    header::PROXY_AUTHENTICATE,
    header::PROXY_AUTHORIZATION,
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
];

/// Forwards one request under `/rtc` to LiveKit.
pub async fn proxy(State(state): State<AppState>, mut request: Request) -> Response {
    let upstream = state.livekit_authority().to_owned();
    let is_upgrade = request.headers().contains_key(header::UPGRADE);
    // Must be taken before the request is moved: it resolves once our 101 is sent.
    let client_upgrade = is_upgrade.then(|| hyper::upgrade::on(&mut request));

    let path = request
        .uri()
        .path_and_query()
        .map_or("/rtc", |p| p.as_str())
        .to_owned();
    let (mut parts, body) = request.into_parts();
    parts.uri = match path.parse::<Uri>() {
        Ok(uri) => uri,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    for name in &HOP_BY_HOP {
        parts.headers.remove(name);
    }
    if let Ok(host) = HeaderValue::from_str(&upstream) {
        parts.headers.insert(header::HOST, host);
    }
    let forwarded = axum::http::Request::from_parts(parts, body);

    let mut response = match send(&upstream, forwarded).await {
        Ok(response) => response,
        Err(e) => {
            eprintln!("[oc-server] LiveKit at {upstream} unreachable: {e}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };

    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let Some(client_upgrade) = client_upgrade else {
            return StatusCode::BAD_GATEWAY.into_response();
        };
        let upstream_upgrade = hyper::upgrade::on(&mut response);
        tokio::spawn(async move {
            let (Ok(client), Ok(livekit)) = (client_upgrade.await, upstream_upgrade.await) else {
                return;
            };
            let (mut client, mut livekit) = (TokioIo::new(client), TokioIo::new(livekit));
            // Ends when either side closes; errors here are just disconnects.
            let _ = tokio::io::copy_bidirectional(&mut client, &mut livekit).await;
        });
    }
    response.map(Body::new)
}

async fn send(
    upstream: &str,
    request: axum::http::Request<Body>,
) -> Result<axum::http::Response<hyper::body::Incoming>, Box<dyn std::error::Error + Send + Sync>> {
    let tcp = TcpStream::connect(upstream).await?;
    tcp.set_nodelay(true)?; // signalling is small, latency-sensitive messages
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tcp)).await?;
    // `with_upgrades` keeps the connection alive past a 101 so it can be taken over.
    tokio::spawn(connection.with_upgrades());
    Ok(sender.send_request(request).await?)
}
