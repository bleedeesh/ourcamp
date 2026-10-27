//! The server end to end: real TCP, real TLS with the client's pinning verifier
//! (`oc-tls`), real HTTP.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use oc_invite::{Fingerprint, Host, Invite, InviteKind};
use oc_server::{start, Config, Running, API_VERSION};
use rustls::pki_types::ServerName;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// A fresh data directory per test (tests run in parallel).
fn data_dir() -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "oc-server-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn config(data_dir: PathBuf) -> Config {
    Config {
        data_dir,
        bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        public_host: Host::Domain("camp.example".into()),
        public_port: None,
        camp_name: "Вечерний кооп".into(),
    }
}

/// One HTTP/1.1 GET over pinned TLS. Returns (status, JSON body).
async fn get(addr: SocketAddr, pin: Fingerprint, path: &str) -> std::io::Result<(u16, Value)> {
    let tcp = TcpStream::connect(addr).await?;
    let connector = TlsConnector::from(oc_tls::client_config(pin));
    // Any name: the pin replaces name checks.
    let name = ServerName::try_from("ourcamp").unwrap();
    let mut tls = connector.connect(name, tcp).await?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: ourcamp\r\nConnection: close\r\n\r\n");
    tls.write_all(request.as_bytes()).await?;
    let mut raw = Vec::new();
    tls.read_to_end(&mut raw).await?;
    let text = String::from_utf8(raw).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").expect("HTTP response");
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    Ok((status, serde_json::from_str(body).unwrap_or(Value::Null)))
}

async fn running() -> (Running, PathBuf) {
    let dir = data_dir();
    (start(config(dir.clone())).await.unwrap(), dir)
}

#[tokio::test]
async fn camp_is_served_over_pinned_tls() {
    let (camp, dir) = running().await;
    let (status, body) = get(camp.local_addr, camp.fingerprint, "/api/v1/camp")
        .await
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(body["apiVersion"], API_VERSION);
    assert_eq!(body["name"], "Вечерний кооп");
    assert_eq!(body["channels"][0]["name"], "Лобби");
    assert_eq!(body["channels"][0]["id"], "c0");
    camp.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_client_pinning_another_certificate_cannot_connect() {
    let (camp, dir) = running().await;
    let wrong = Fingerprint([7; 32]);
    let err = get(camp.local_addr, wrong, "/api/v1/camp")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("certificate"),
        "expected a certificate error, got {err}"
    );
    camp.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn links_carry_the_public_address_and_the_real_fingerprint() {
    let (camp, dir) = running().await;
    let join: Invite = camp.links.join.to_string().parse().unwrap();
    assert_eq!(join.host, Host::Domain("camp.example".into()));
    assert_eq!(
        join.port,
        camp.local_addr.port(),
        "public port defaults to the bound one"
    );
    assert_eq!(join.kind, InviteKind::Join);
    assert_eq!(join.fingerprint, camp.fingerprint);
    let claim = camp
        .links
        .claim
        .as_ref()
        .expect("fresh camp has a claim key");
    assert_eq!(claim.kind, InviteKind::ClaimAdmin);
    camp.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn invite_status_is_lenient_about_case_and_strict_about_existence() {
    let (camp, dir) = running().await;
    let (addr, pin) = (camp.local_addr, camp.fingerprint);
    let join = camp.links.join.token.as_str().to_ascii_uppercase();
    let claim = camp.links.claim.as_ref().unwrap().token.as_str().to_owned();

    let (status, body) = get(addr, pin, &format!("/api/v1/invites/{join}"))
        .await
        .unwrap();
    assert_eq!(
        (status, &body["kind"], &body["usable"]),
        (200, &"join".into(), &true.into())
    );
    let (status, body) = get(addr, pin, &format!("/api/v1/invites/{claim}"))
        .await
        .unwrap();
    assert_eq!((status, &body["kind"]), (200, &"claim".into()));

    // Well-formed but never issued, and not a token at all.
    let unknown = "0".repeat(16);
    let (status, body) = get(addr, pin, &format!("/api/v1/invites/{unknown}"))
        .await
        .unwrap();
    assert_eq!((status, &body["error"]), (404, &"not_found".into()));
    let (status, _) = get(addr, pin, "/api/v1/invites/hello").await.unwrap();
    assert_eq!(status, 404);
    camp.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn restart_keeps_fingerprint_and_links() {
    let dir = data_dir();
    let first = start(config(dir.clone())).await.unwrap();
    let (fp, join, claim) = (
        first.fingerprint,
        first.links.join.token.clone(),
        first.links.claim.as_ref().map(|c| c.token.clone()),
    );
    first.stop().await;

    let second = start(config(dir.clone())).await.unwrap();
    assert_eq!(
        second.fingerprint, fp,
        "links already sent must keep working"
    );
    assert_eq!(second.links.join.token, join);
    assert_eq!(second.links.claim.as_ref().map(|c| c.token.clone()), claim);
    second.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}
