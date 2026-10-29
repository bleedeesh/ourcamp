//! Minimal HTTPS: one TLS connection per request (logins, token requests and
//! LiveKit's `validate` probe are rare), HTTP/1.1 via `hyper`.

use std::io;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HOST};
use hyper::{HeaderMap, Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use oc_proto::ErrorBody;
use oc_tls::Trust;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

/// What every API request but an avatar upload carries.
const JSON: &str = "application/json";
use crate::ClientError;

/// Connect + TLS + request + response must finish within this.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The largest JSON answer: a page of chat history (100 messages of up to 2000
/// characters) is the biggest. Anything larger is refused.
const MAX_JSON_BYTES: usize = 1024 * 1024;
/// The largest picture: a photo (avatars are smaller).
const MAX_PICTURE_BYTES: usize = oc_proto::MAX_PHOTO_BYTES;

/// TCP + TLS to `authority` (`host:port`), trusting only what `trust` allows: the pinned
/// certificate (ADR-0014), or an authority's for the host's name (ADR-0042).
pub(crate) async fn connect(authority: &str, trust: &Trust) -> io::Result<TlsStream<TcpStream>> {
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host);
    let name = trust
        .server_name(host)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let tcp = TcpStream::connect(authority).await?;
    tcp.set_nodelay(true)?;
    TlsConnector::from(trust.client_config())
        .connect(name, tcp)
        .await
}

/// One request over a fresh connection; returns status, headers and body (at most
/// `limit` bytes).
pub(crate) async fn send(
    authority: &str,
    trust: &Trust,
    request: Request<Full<Bytes>>,
    limit: usize,
) -> io::Result<(StatusCode, HeaderMap, Bytes)> {
    let exchange = async {
        let tls = connect(authority, trust).await?;
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
        let bytes = http_body_util::Limited::new(body, limit)
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
    pub(crate) trust: Trust,
}

impl Client {
    pub(crate) fn new(authority: String, trust: Trust) -> Self {
        Self { authority, trust }
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
        self.exchange(
            Method::POST,
            path,
            Some(bearer),
            Bytes::from(body),
            JSON,
            MAX_JSON_BYTES,
        )
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
        self.exchange(
            Method::PUT,
            path,
            Some(bearer),
            Bytes::from(body),
            JSON,
            MAX_JSON_BYTES,
        )
        .await
        .map(drop)
    }

    /// `DELETE` with a session; success has no body (`204`).
    pub(crate) async fn delete_authorized(
        &self,
        path: &str,
        bearer: &str,
    ) -> Result<(), ClientError> {
        self.exchange(
            Method::DELETE,
            path,
            Some(bearer),
            Bytes::new(),
            JSON,
            MAX_JSON_BYTES,
        )
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
        let bytes = self
            .exchange(method, path, bearer, body, JSON, MAX_JSON_BYTES)
            .await?;
        serde_json::from_slice(&bytes).map_err(|e| ClientError::Protocol(e.to_string()))
    }

    /// `PUT` of raw bytes (an avatar) with a session; success has no body (`204`).
    pub(crate) async fn put_bytes(
        &self,
        path: &str,
        bearer: &str,
        content_type: &'static str,
        body: Vec<u8>,
    ) -> Result<(), ClientError> {
        self.exchange(
            Method::PUT,
            path,
            Some(bearer),
            Bytes::from(body),
            content_type,
            MAX_JSON_BYTES,
        )
        .await
        .map(drop)
    }

    /// `POST` of raw bytes (a photo) with a session; a JSON answer.
    pub(crate) async fn post_bytes<T: DeserializeOwned>(
        &self,
        path: &str,
        bearer: &str,
        content_type: &'static str,
        body: Vec<u8>,
    ) -> Result<T, ClientError> {
        let bytes = self
            .exchange(
                Method::POST,
                path,
                Some(bearer),
                Bytes::from(body),
                content_type,
                MAX_JSON_BYTES,
            )
            .await?;
        serde_json::from_slice(&bytes).map_err(|e| ClientError::Protocol(e.to_string()))
    }

    /// `GET` of raw bytes (an avatar, a photo) with a session.
    pub(crate) async fn get_bytes(&self, path: &str, bearer: &str) -> Result<Bytes, ClientError> {
        self.exchange(
            Method::GET,
            path,
            Some(bearer),
            Bytes::new(),
            JSON,
            MAX_PICTURE_BYTES,
        )
        .await
    }

    /// One request; the body of a success (at most `limit` bytes), or the API error.
    async fn exchange(
        &self,
        method: Method,
        path: &str,
        bearer: Option<&str>,
        body: Bytes,
        content_type: &'static str,
        limit: usize,
    ) -> Result<Bytes, ClientError> {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, "ourcamp")
            .header(CONTENT_TYPE, content_type);
        if let Some(token) = bearer {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = request
            .body(Full::new(body))
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        let (status, _, bytes) = send(&self.authority, &self.trust, request, limit).await?;

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
