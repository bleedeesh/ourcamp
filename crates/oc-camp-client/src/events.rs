//! `GET /api/v1/events`: the camp's live state over a pinned WebSocket (ADR-0021).

use futures_util::{SinkExt, StreamExt};
use oc_proto::{CampEvent, ErrorCode};
use oc_tls::Trust;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::WebSocketStream;

use crate::ClientError;

/// An open event stream. Dropping it closes the connection.
pub struct CampEvents {
    ws: WebSocketStream<TlsStream<TcpStream>>,
}

impl CampEvents {
    pub(crate) async fn connect(
        authority: &str,
        trust: &Trust,
        session: &str,
    ) -> Result<Self, ClientError> {
        let protocol = |e: WsError| ClientError::Protocol(e.to_string());
        let mut request = format!("wss://{authority}/api/v1/events")
            .into_client_request()
            .map_err(protocol)?;
        let bearer = HeaderValue::from_str(&format!("Bearer {session}"))
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        request.headers_mut().insert("authorization", bearer);

        let tls = crate::http::connect(authority, trust).await?;
        match tokio_tungstenite::client_async(request, tls).await {
            Ok((ws, _)) => Ok(Self { ws }),
            // Refused before the upgrade: the server's status says why.
            Err(WsError::Http(response)) if response.status() == 401 => {
                Err(ClientError::Api(ErrorCode::Unauthorized))
            }
            Err(WsError::Io(e)) => Err(ClientError::Io(e)),
            Err(e) => Err(protocol(e)),
        }
    }

    /// The next event; `None` when the camp closed the stream.
    pub async fn next(&mut self) -> Option<Result<CampEvent, ClientError>> {
        loop {
            match self.ws.next().await? {
                Ok(Message::Text(text)) => {
                    return Some(
                        serde_json::from_str(&text)
                            .map_err(|e| ClientError::Protocol(e.to_string())),
                    );
                }
                Ok(Message::Ping(payload)) => {
                    // tungstenite queues the pong; flushing sends it.
                    let _ = self.ws.send(Message::Pong(payload)).await;
                }
                Ok(Message::Close(_)) | Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => {
                    return None;
                }
                Ok(_) => {}
                Err(WsError::Io(e)) => return Some(Err(ClientError::Io(e))),
                Err(e) => return Some(Err(ClientError::Protocol(e.to_string()))),
            }
        }
    }
}
