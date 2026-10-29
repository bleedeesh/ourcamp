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

#[allow(
    dead_code,
    reason = "tests/server.rs uses it, tests/managed.rs does not"
)]
/// A raw `PUT` of bytes with a bearer session over pinned TLS; the status. For bodies the
/// honest client refuses to send (an SVG dressed as a PNG, an oversized picture).
pub async fn put_bytes(
    addr: SocketAddr,
    pin: Fingerprint,
    path: &str,
    bearer: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<u16> {
    let tcp = TcpStream::connect(addr).await?;
    let connector = TlsConnector::from(oc_tls::client_config(pin));
    let mut tls = connector
        .connect(ServerName::try_from("ourcamp").unwrap(), tcp)
        .await?;
    let head = format!(
        "PUT {path} HTTP/1.1\r\nHost: ourcamp\r\nConnection: close\r\n\
         Authorization: Bearer {bearer}\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\n\r\n",
        body.len()
    );
    tls.write_all(head.as_bytes()).await?;
    tls.write_all(body).await?;
    let mut response = Vec::new();
    tls.read_to_end(&mut response).await?;
    let text = String::from_utf8_lossy(&response);
    Ok(text.split(' ').nth(1).unwrap().parse().unwrap())
}

#[allow(
    dead_code,
    reason = "tests/server.rs uses it, tests/managed.rs does not"
)]
/// A made-up certificate authority, and the full chain it issued for `domain`, written
/// into `dir` as certbot writes it (ADR-0042). The roots go to the client under test.
pub fn issue_certificate(
    dir: &std::path::Path,
    domain: &str,
) -> (
    std::sync::Arc<rustls::RootCertStore>,
    oc_server::CertificateFiles,
) {
    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
    let ca = || {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
    };
    let root = CertifiedIssuer::self_signed(ca(), KeyPair::generate().unwrap()).unwrap();
    let intermediate =
        CertifiedIssuer::signed_by(ca(), KeyPair::generate().unwrap(), &root).unwrap();
    let key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec![domain.to_owned()])
        .unwrap()
        .signed_by(&key, &intermediate)
        .unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let files = oc_server::CertificateFiles {
        chain: dir.join("fullchain.pem"),
        key: dir.join("privkey.pem"),
    };
    std::fs::write(&files.chain, leaf.pem() + &intermediate.pem()).unwrap();
    std::fs::write(&files.key, key.serialize_pem()).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.der().clone()).unwrap();
    (std::sync::Arc::new(roots), files)
}
