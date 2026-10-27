//! Minimal JSON-over-HTTPS client: one pinned TLS connection per request (logins and
//! token requests are rare), HTTP/1.1 via `hyper`, bodies via `serde_json`.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HOST};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use oc_invite::Fingerprint;
use oc_proto::ErrorBody;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::{rustls, TlsConnector};

use crate::ClientError;

/// Connect + TLS + request + response must finish within this.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Responses are small JSON; refuse anything that is not.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

pub(crate) struct Client {
    authority: String,
    tls: TlsConnector,
}

impl Client {
    pub(crate) fn new(authority: String, fingerprint: Fingerprint) -> Self {
        let config: Arc<rustls::ClientConfig> = oc_tls::client_config(fingerprint);
        Self {
            authority,
            tls: TlsConnector::from(config),
        }
    }

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        self.request(Method::GET, path, None, Bytes::new()).await
    }

    pub(crate) async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        bearer: Option<&str>,
        body: &impl Serialize,
    ) -> Result<T, ClientError> {
        let body = serde_json::to_vec(body).map_err(|e| ClientError::Protocol(e.to_string()))?;
        self.request(Method::POST, path, bearer, Bytes::from(body))
            .await
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        bearer: Option<&str>,
        body: Bytes,
    ) -> Result<T, ClientError> {
        tokio::time::timeout(REQUEST_TIMEOUT, self.exchange(method, path, bearer, body))
            .await
            .map_err(|_| {
                ClientError::Io(io::Error::new(io::ErrorKind::TimedOut, "camp timed out"))
            })?
    }

    async fn exchange<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        bearer: Option<&str>,
        body: Bytes,
    ) -> Result<T, ClientError> {
        let tcp = TcpStream::connect(self.authority.as_str()).await?;
        // The name is only sent as SNI; the pin replaces name checks (ADR-0014).
        let name = ServerName::try_from("ourcamp").expect("static name");
        let tls = self.tls.connect(name, tcp).await?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .map_err(io::Error::other)?;
        // Drives the connection; ends when the response is read and the request dropped.
        tokio::spawn(connection);

        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, "ourcamp")
            .header(CONTENT_TYPE, "application/json");
        if let Some(token) = bearer {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = request
            .body(Full::new(body))
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        let response = sender
            .send_request(request)
            .await
            .map_err(io::Error::other)?;
        let status = response.status();
        let bytes = http_body_util::Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
            .collect()
            .await
            .map_err(|e| ClientError::Protocol(e.to_string()))?
            .to_bytes();

        if status.is_success() {
            serde_json::from_slice(&bytes).map_err(|e| ClientError::Protocol(e.to_string()))
        } else {
            match serde_json::from_slice::<ErrorBody>(&bytes) {
                Ok(body) => Err(ClientError::Api(body.error)),
                Err(_) => Err(ClientError::Protocol(format!("HTTP {status}"))),
            }
        }
    }
}
