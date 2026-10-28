//! Helpers shared by the integration tests (`mod support;` in each).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use oc_invite::Fingerprint;
use rustls::pki_types::ServerName;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// A fresh data directory per test (tests run in parallel).
pub fn data_dir() -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "oc-server-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// One raw HTTP/1.1 request over pinned TLS. Returns (status, JSON body).
pub async fn raw(
    addr: SocketAddr,
    pin: Fingerprint,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> std::io::Result<(u16, Value)> {
    let tcp = TcpStream::connect(addr).await?;
    let connector = TlsConnector::from(oc_tls::client_config(pin));
    let mut tls = connector
        .connect(ServerName::try_from("ourcamp").unwrap(), tcp)
        .await?;
    let body = body.map(Value::to_string).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: ourcamp\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    tls.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    tls.read_to_end(&mut response).await?;
    let text = String::from_utf8(response).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").expect("HTTP response");
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    Ok((status, serde_json::from_str(body).unwrap_or(Value::Null)))
}
