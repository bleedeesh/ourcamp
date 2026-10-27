//! LiveKit signalling over the camp's pinned TLS (ADR-0014, ADR-0019).
//!
//! The LiveKit SDK opens its signalling WebSocket (and an occasional `validate` HTTP
//! probe) through a process-wide transport from `livekit-net`. [`install`] replaces it
//! with [`PinnedTransport`]: every `wss://`/`https://` connection must go to an address
//! whose certificate fingerprint was registered with [`pin`] — anything else is refused
//! (fail closed). Media keys travel in that signalling, so pinning it protects the audio.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full};
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Method, Request, Uri};
use hyper_util::rt::TokioIo;
use livekit_net::{
    Header, HttpClient, HttpMethod, HttpResponse, TransportError, WsClient, WsConnectResult,
    WsConnection,
};
use oc_invite::Fingerprint;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::WebSocketStream;

/// `host:port` (lower-case, as in the URL) → certificate fingerprint.
static PINS: LazyLock<RwLock<HashMap<String, Fingerprint>>> = LazyLock::new(RwLock::default);

/// Trusts `fingerprint` for `authority` (`host:port` as it appears in URLs, IPv6 in
/// brackets). Re-pinning an address replaces its fingerprint.
pub fn pin(authority: &str, fingerprint: Fingerprint) {
    PINS.write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(authority.to_ascii_lowercase(), fingerprint);
}

fn pinned(authority: &str) -> Option<Fingerprint> {
    PINS.read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&authority.to_ascii_lowercase())
        .copied()
}

/// Whether unencrypted `ws://`/`http://` to loopback is allowed: the local LiveKit of
/// `pnpm livekit` during development. Never to any other host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlainLoopback {
    /// Debug builds: allow `ws://127.0.0.1:…`, `ws://[::1]:…`, `ws://localhost:…`.
    Allow,
    /// Release builds: TLS with a pin, always.
    Deny,
}

/// Makes the LiveKit SDK use [`PinnedTransport`] for the whole process. Call once at
/// startup, before the first room connect: the SDK keeps the first registration.
pub fn install(plain_loopback: PlainLoopback) {
    let transport = Arc::new(PinnedTransport { plain_loopback });
    livekit_net::set_ws_client(transport.clone());
    livekit_net::set_http_client(transport);
}

/// The transport [`install`] registers.
#[derive(Debug)]
pub struct PinnedTransport {
    plain_loopback: PlainLoopback,
}

/// Any byte stream tungstenite and hyper can run over (TLS or, on loopback, plain TCP).
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

fn refused(message: impl Into<String>) -> TransportError {
    TransportError::Connection(message.into())
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl PinnedTransport {
    /// Opens the byte stream for `uri`, enforcing the pin/loopback policy.
    async fn open(&self, uri: &Uri) -> Result<Box<dyn Io>, TransportError> {
        let host = uri.host().ok_or_else(|| refused("URL without a host"))?;
        let (secure, default_port) = match uri.scheme_str() {
            Some("wss" | "https") => (true, 443),
            Some("ws" | "http") => (false, 80),
            other => return Err(refused(format!("unsupported scheme {other:?}"))),
        };
        let authority = format!("{host}:{}", uri.port_u16().unwrap_or(default_port));
        let io_error = |e: io::Error| refused(e.to_string());

        if secure {
            let fingerprint = pinned(&authority).ok_or_else(|| {
                refused(format!(
                    "{authority} is not a known camp; refusing to connect"
                ))
            })?;
            let tls = crate::http::connect_pinned(&authority, fingerprint)
                .await
                .map_err(io_error)?;
            Ok(Box::new(tls))
        } else if self.plain_loopback == PlainLoopback::Allow && is_loopback(host) {
            let tcp = TcpStream::connect(authority.as_str())
                .await
                .map_err(io_error)?;
            tcp.set_nodelay(true).map_err(io_error)?;
            Ok(Box::new(tcp))
        } else {
            Err(refused(format!(
                "unencrypted connection to {authority} refused"
            )))
        }
    }
}

fn parse(url: &str) -> Result<Uri, TransportError> {
    url.parse::<Uri>()
        .map_err(|e| refused(format!("bad URL: {e}")))
}

fn header_pair(h: &Header) -> Result<(HeaderName, HeaderValue), TransportError> {
    let name = h
        .name
        .parse::<HeaderName>()
        .map_err(|_| TransportError::Other("bad header name".into()))?;
    let value = HeaderValue::from_str(&h.value)
        .map_err(|_| TransportError::Other("bad header value".into()))?;
    Ok((name, value))
}

#[async_trait::async_trait]
impl WsClient for PinnedTransport {
    async fn connect(
        &self,
        url: String,
        headers: Vec<Header>,
        timeout_ms: u64,
    ) -> Result<WsConnectResult, TransportError> {
        let uri = parse(&url)?;
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| refused(e.to_string()))?;
        for h in &headers {
            let (name, value) = header_pair(h)?;
            request.headers_mut().insert(name, value);
        }
        let connect = async {
            let stream = self.open(&uri).await?;
            let (ws, _) = tokio_tungstenite::client_async(request, stream)
                .await
                .map_err(|e| refused(e.to_string()))?;
            Ok::<_, TransportError>(ws)
        };
        let ws = tokio::time::timeout(Duration::from_millis(timeout_ms), connect)
            .await
            .map_err(|_| TransportError::Timeout)??;
        let (writer, reader) = ws.split();
        Ok(WsConnectResult {
            connection: Arc::new(Connection {
                writer: Mutex::new(writer),
                reader: Mutex::new(reader),
            }),
        })
    }
}

#[async_trait::async_trait]
impl HttpClient for PinnedTransport {
    async fn request(
        &self,
        method: HttpMethod,
        url: String,
        headers: Vec<Header>,
        body: Option<Vec<u8>>,
    ) -> Result<HttpResponse, TransportError> {
        let uri = parse(&url)?;
        let path = uri
            .path_and_query()
            .map_or("/", hyper::http::uri::PathAndQuery::as_str);
        let mut builder = Request::builder()
            .method(match method {
                HttpMethod::Get => Method::GET,
                HttpMethod::Post => Method::POST,
            })
            .uri(path)
            .header(
                hyper::header::HOST,
                uri.authority().map_or("", |a| a.as_str()),
            );
        for h in &headers {
            let (name, value) = header_pair(h)?;
            builder = builder.header(name, value);
        }
        let request = builder
            .body(Full::new(Bytes::from(body.unwrap_or_default())))
            .map_err(|e| TransportError::Other(e.to_string()))?;

        let stream = self.open(&uri).await?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| refused(e.to_string()))?;
        tokio::spawn(connection);
        let response = sender
            .send_request(request)
            .await
            .map_err(|e| refused(e.to_string()))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(n, v)| {
                Some(Header {
                    name: n.as_str().to_owned(),
                    value: v.to_str().ok()?.to_owned(),
                })
            })
            .collect();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|e| refused(e.to_string()))?
            .to_bytes()
            .to_vec();
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

type WebSocket = WebSocketStream<Box<dyn Io>>;

/// One signalling WebSocket. Mirrors `livekit-net`'s native connection: binary frames
/// cross the trait, control frames are handled here.
struct Connection {
    writer: Mutex<SplitSink<WebSocket, Message>>,
    reader: Mutex<SplitStream<WebSocket>>,
}

#[async_trait::async_trait]
impl WsConnection for Connection {
    async fn send(&self, frame: Vec<u8>) -> Result<(), TransportError> {
        self.writer
            .lock()
            .await
            .send(Message::Binary(frame.into()))
            .await
            .map_err(|e| refused(e.to_string()))
    }

    async fn recv(&self) -> Result<Option<Vec<u8>>, TransportError> {
        let mut reader = self.reader.lock().await;
        loop {
            match reader.next().await {
                Some(Ok(Message::Binary(data))) => return Ok(Some(data.to_vec())),
                Some(Ok(Message::Ping(payload))) => {
                    let _ = self.writer.lock().await.send(Message::Pong(payload)).await;
                }
                // Signalling never sends text; pongs and raw frames need no action.
                Some(Ok(Message::Pong(_) | Message::Frame(_) | Message::Text(_))) => {}
                None
                | Some(
                    Ok(Message::Close(_))
                    | Err(
                        WsError::ConnectionClosed
                        | WsError::AlreadyClosed
                        | WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake),
                    ),
                ) => return Ok(None),
                // A peer that drops TCP/TLS without a close frame is still a close.
                Some(Err(WsError::Io(e))) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Ok(None)
                }
                Some(Err(e)) => return Err(refused(e.to_string())),
            }
        }
    }

    async fn close(&self) {
        let _ = self.writer.lock().await.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport(plain: PlainLoopback) -> PinnedTransport {
        PinnedTransport {
            plain_loopback: plain,
        }
    }

    #[test]
    fn loopback_detection() {
        for host in ["127.0.0.1", "[::1]", "localhost", "LOCALHOST", "127.5.6.7"] {
            assert!(is_loopback(host), "{host}");
        }
        for host in ["192.168.1.2", "example.com", "[2001:db8::1]"] {
            assert!(!is_loopback(host), "{host}");
        }
    }

    async fn open_error(t: &PinnedTransport, url: &str) -> String {
        match t.open(&parse(url).unwrap()).await {
            Err(TransportError::Connection(m)) => m,
            Err(e) => panic!("unexpected error {e:?}"),
            Ok(_) => panic!("{url} must be refused"),
        }
    }

    #[tokio::test]
    async fn unpinned_tls_hosts_are_refused_before_any_network_io() {
        let t = transport(PlainLoopback::Allow);
        let msg = open_error(&t, "wss://203.0.113.9:7780/rtc").await;
        assert!(msg.contains("not a known camp"), "{msg}");
    }

    #[tokio::test]
    async fn plain_connections_only_to_loopback_and_only_when_allowed() {
        let msg = open_error(
            &transport(PlainLoopback::Allow),
            "ws://192.168.1.2:7880/rtc",
        )
        .await;
        assert!(msg.contains("unencrypted"), "{msg}");
        let msg = open_error(&transport(PlainLoopback::Deny), "ws://127.0.0.1:7880/rtc").await;
        assert!(msg.contains("unencrypted"), "{msg}");
        let msg = open_error(&transport(PlainLoopback::Allow), "ftp://127.0.0.1/").await;
        assert!(msg.contains("unsupported scheme"), "{msg}");
    }

    #[test]
    fn pins_are_case_insensitive_and_replaceable() {
        pin("Camp.Example:7780", Fingerprint([1; 32]));
        assert_eq!(pinned("camp.example:7780"), Some(Fingerprint([1; 32])));
        pin("camp.example:7780", Fingerprint([2; 32]));
        assert_eq!(pinned("CAMP.EXAMPLE:7780"), Some(Fingerprint([2; 32])));
        assert_eq!(pinned("camp.example:7781"), None, "port is part of the pin");
    }
}
