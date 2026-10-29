//! Minimal HTTPS: one pinned TLS connection per request (logins, token requests and
//! LiveKit's `validate` probe are rare), HTTP/1.1 via `hyper`.

use std::io;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HOST};
use hyper::{HeaderMap, Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use oc_invite::Fingerprint;
use oc_proto::ErrorBody;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;

use crate::ClientError;

/// Connect + TLS + request + response must finish within this.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Responses are small; refuse anything that is not.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// TCP + TLS to `authority`, trusting only the certificate with `fingerprint`.
pub(crate) async fn connect_pinned(
    authority: &str,
    fingerprint: Fingerprint,
) -> io::Result<TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(authority).await?;
    tcp.set_nodelay(true)?;
    // The name is only sent as SNI; the pin replaces name checks (ADR-0014).
    let name = ServerName::try_from("ourcamp").expect("static name");
    TlsConnector::from(oc_tls::client_config(fingerprint))
        .connect(name, tcp)
        .await
}

/// One request over a fresh pinned connection; returns status, headers and body.
pub(crate) async fn send(
    authority: &str,
    fingerprint: Fingerprint,
    request: Request<Full<Bytes>>,
) -> io::Result<(StatusCode, HeaderMap, Bytes)> {
    let exchange = async {
        let tls = connect_pinned(authority, fingerprint).await?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .map_err(io::Error::other)?;
        // Drives the connection; ends when the response is read and the request dropped.
        tokio::spawn(connection);
        let response = sender
            .send_request(request)
            .await
            .map_err(io::Error::other)?;
        let (parts, body) = response.into_parts();
        let bytes = http_body_util::Limited::new(body, MAX_RESPONSE_BYTES)
            .collect()
            .await
            .map_err(io::Error::other)?
            .to_bytes();
        Ok((parts.status, parts.headers, bytes))
    };
    tokio::time::timeout(REQUEST_TIMEOUT, exchange)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "camp timed out"))?
}

/// JSON API client for one camp.
pub(crate) struct Client {
    pub(crate) authority: String,
    fingerprint: Fingerprint,
}

impl Client {
    pub(crate) fn new(authority: String, fingerprint: Fingerprint) -> Self {
        Self {
            authority,
            fingerprint,
        }
    }

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        self.request(Method::GET, path, None, Bytes::new()).await
    }

    pub(crate) async fn get_authorized<T: DeserializeOwned>(
        &self,
        path: &str,
        bearer: &str,
    ) -> Result<T, ClientError> {
        self.request(Method::GET, path, Some(bearer), Bytes::new())
            .await
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

    /// `POST` with a session whose success has no body (`204`).
    pub(crate) async fn post_no_content(
        &self,
        path: &str,
        bearer: &str,
        body: &impl Serialize,
    ) -> Result<(), ClientError> {
        let body = serde_json::to_vec(body).map_err(|e| ClientError::Protocol(e.to_string()))?;
        self.exchange(Method::POST, path, Some(bearer), Bytes::from(body))
            .await
            .map(drop)
    }

    /// `PUT` with a session whose success has no body (`204`).
    pub(crate) async fn put_no_content(
        &self,
        path: &str,
        bearer: &str,
        body: &impl Serialize,
    ) -> Result<(), ClientError> {
        let body = serde_json::to_vec(body).map_err(|e| ClientError::Protocol(e.to_string()))?;
        self.exchange(Method::PUT, path, Some(bearer), Bytes::from(body))
            .await
            .map(drop)
    }

    /// `DELETE` with a session; success has no body (`204`).
    pub(crate) async fn delete_authorized(
        &self,
        path: &str,
        bearer: &str,
    ) -> Result<(), ClientError> {
        self.exchange(Method::DELETE, path, Some(bearer), Bytes::new())
            .await
            .map(drop)
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        bearer: Option<&str>,
        body: Bytes,
    ) -> Result<T, ClientError> {
        let bytes = self.exchange(method, path, bearer, body).await?;
        serde_json::from_slice(&bytes).map_err(|e| ClientError::Protocol(e.to_string()))
    }

    /// One request; the body of a success, or the API error.
    async fn exchange(
        &self,
        method: Method,
        path: &str,
        bearer: Option<&str>,
        body: Bytes,
    ) -> Result<Bytes, ClientError> {
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
        let (status, _, bytes) = send(&self.authority, self.fingerprint, request).await?;

        if status.is_success() {
            Ok(bytes)
        } else {
            match serde_json::from_slice::<ErrorBody>(&bytes) {
                Ok(body) => Err(ClientError::Api(body.error)),
                Err(_) => Err(ClientError::Protocol(format!("HTTP {status}"))),
            }
        }
    }
}
