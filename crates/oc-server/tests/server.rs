//! The server end to end: real TCP, real TLS with the client's pinning verifier,
//! real HTTP. Honest flows go through `oc-camp-client` (the code the app will use);
//! attacks are hand-made requests the honest client cannot produce.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use oc_camp_client::{CampClient, ClientError, DeviceKey};
use oc_invite::{Fingerprint, Host, Invite, InviteKind};
use oc_proto::{ErrorCode, InviteKindDto, Role, API_VERSION};
use oc_server::{start, Config, LiveKit, Running};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use rustls::pki_types::ServerName;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

const LIVEKIT_URL: &str = "ws://127.0.0.1:7880";

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
        livekit: LiveKit {
            url: LIVEKIT_URL.into(),
            api_key: "devkey".into(),
            api_secret: "secret".into(),
        },
    }
}

struct Camp {
    running: Running,
    dir: PathBuf,
}

impl Camp {
    async fn start() -> Self {
        let dir = data_dir();
        Self {
            running: start(config(dir.clone())).await.unwrap(),
            dir,
        }
    }

    /// The honest client, reaching the camp on its real (loopback) address.
    fn client(&self) -> CampClient {
        CampClient::at(
            &Host::Ipv4(Ipv4Addr::LOCALHOST),
            self.running.local_addr.port(),
            self.running.fingerprint,
        )
    }

    fn join(&self) -> String {
        self.running.links.join.token.as_str().to_owned()
    }

    fn claim(&self) -> String {
        self.running
            .links
            .claim
            .as_ref()
            .unwrap()
            .token
            .as_str()
            .to_owned()
    }

    async fn stop(self) {
        self.running.stop().await;
        let _ = std::fs::remove_dir_all(self.dir);
    }
}

/// One raw HTTP/1.1 request over pinned TLS. Returns (status, JSON body).
async fn raw(
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

fn api_error(result: Result<impl std::fmt::Debug, ClientError>) -> ErrorCode {
    match result {
        Err(ClientError::Api(code)) => code,
        other => panic!("expected an API error, got {other:?}"),
    }
}

// ---- step 1: camp, links, TLS ----

#[tokio::test]
async fn camp_is_served_over_pinned_tls() {
    let camp = Camp::start().await;
    let info = camp.client().camp().await.unwrap();
    assert_eq!(info.api_version, API_VERSION);
    assert_eq!(info.name, "Вечерний кооп");
    assert_eq!(info.channels[0].name, "Лобби");
    assert_eq!(info.channels[0].id, "c0");
    camp.stop().await;
}

#[tokio::test]
async fn a_client_pinning_another_certificate_cannot_connect() {
    let camp = Camp::start().await;
    let impostor = CampClient::at(
        &Host::Ipv4(Ipv4Addr::LOCALHOST),
        camp.running.local_addr.port(),
        Fingerprint([7; 32]),
    );
    match impostor.camp().await {
        Err(ClientError::Io(e)) => assert!(e.to_string().contains("certificate"), "{e}"),
        other => panic!("expected a certificate error, got {other:?}"),
    }
    camp.stop().await;
}

#[tokio::test]
async fn links_carry_the_public_address_and_the_real_fingerprint() {
    let camp = Camp::start().await;
    let join: Invite = camp.running.links.join.to_string().parse().unwrap();
    assert_eq!(join.host, Host::Domain("camp.example".into()));
    assert_eq!(join.port, camp.running.local_addr.port());
    assert_eq!(join.kind, InviteKind::Join);
    assert_eq!(join.fingerprint, camp.running.fingerprint);
    let claim = camp.running.links.claim.as_ref().unwrap();
    assert_eq!(claim.kind, InviteKind::ClaimAdmin);
    camp.stop().await;
}

#[tokio::test]
async fn invite_status_is_lenient_about_case_and_strict_about_existence() {
    let camp = Camp::start().await;
    let client = camp.client();
    let info = client
        .invite(&camp.join().to_ascii_uppercase())
        .await
        .unwrap();
    assert_eq!((info.kind, info.usable), (InviteKindDto::Join, true));
    let info = client.invite(&camp.claim()).await.unwrap();
    assert_eq!((info.kind, info.usable), (InviteKindDto::Claim, true));
    assert_eq!(
        api_error(client.invite(&"0".repeat(16)).await),
        ErrorCode::NotFound
    );
    assert_eq!(api_error(client.invite("hello").await), ErrorCode::NotFound);
    camp.stop().await;
}

#[tokio::test]
async fn restart_keeps_fingerprint_and_links() {
    let dir = data_dir();
    let first = start(config(dir.clone())).await.unwrap();
    let (fp, join) = (first.fingerprint, first.links.join.token.clone());
    first.stop().await;
    let second = start(config(dir.clone())).await.unwrap();
    assert_eq!(
        second.fingerprint, fp,
        "links already sent must keep working"
    );
    assert_eq!(second.links.join.token, join);
    second.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}

// ---- step 2: login ----

#[tokio::test]
async fn friend_joins_with_the_invite_and_comes_back_without_it() {
    let camp = Camp::start().await;
    let client = camp.client();
    let key = DeviceKey::generate();
    let session = client
        .login(&key, "Лена", Some(&camp.join()))
        .await
        .unwrap();
    assert_eq!(session.role, Role::Member);
    assert_eq!(session.user_id, key.user_id());
    let again = client.login(&key, "Лена", None).await.unwrap();
    assert_eq!((again.user_id, again.role), (key.user_id(), Role::Member));
    camp.stop().await;
}

#[tokio::test]
async fn admin_key_makes_exactly_one_admin() {
    let camp = Camp::start().await;
    let client = camp.client();
    let host = client
        .login(&DeviceKey::generate(), "Дима", Some(&camp.claim()))
        .await
        .unwrap();
    assert_eq!(host.role, Role::Admin);
    let intruder = client
        .login(&DeviceKey::generate(), "Не Дима", Some(&camp.claim()))
        .await;
    assert_eq!(api_error(intruder), ErrorCode::ClaimUsed);
    assert!(!client.invite(&camp.claim()).await.unwrap().usable);
    camp.stop().await;
}

#[tokio::test]
async fn strangers_without_an_invite_are_turned_away() {
    let camp = Camp::start().await;
    let client = camp.client();
    let key = DeviceKey::generate();
    assert_eq!(
        api_error(client.login(&key, "Кто-то", None).await),
        ErrorCode::InviteRequired
    );
    assert_eq!(
        api_error(client.login(&key, "Кто-то", Some(&"0".repeat(16))).await),
        ErrorCode::InviteInvalid
    );
    assert_eq!(
        api_error(client.login(&key, "   ", Some(&camp.join())).await),
        ErrorCode::BadRequest,
        "blank names are refused"
    );
    camp.stop().await;
}

fn ring_key() -> Ed25519KeyPair {
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
}

/// A hand-made login: `signed_for` is the fingerprint put into the signed message.
async fn hand_login(
    camp: &Camp,
    key: &Ed25519KeyPair,
    nonce: &str,
    signed_for: Fingerprint,
) -> (u16, Value) {
    let (addr, pin) = (camp.running.local_addr, camp.running.fingerprint);
    let signature = key.sign(&oc_proto::login_message(&signed_for, nonce));
    let body = json!({
        "publicKey": oc_proto::encode_bytes(key.public_key().as_ref()),
        "name": "Атакующий",
        "nonce": nonce,
        "signature": oc_proto::encode_bytes(signature.as_ref()),
        "invite": camp.join(),
    });
    raw(addr, pin, "POST", "/api/v1/auth/login", Some(&body))
        .await
        .unwrap()
}

async fn nonce(camp: &Camp) -> String {
    let (addr, pin) = (camp.running.local_addr, camp.running.fingerprint);
    let (status, body) = raw(addr, pin, "POST", "/api/v1/auth/challenge", None)
        .await
        .unwrap();
    assert_eq!(status, 200);
    body["nonce"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn a_signature_made_for_another_camp_is_rejected() {
    let camp = Camp::start().await;
    let key = ring_key();
    let n = nonce(&camp).await;
    let (status, body) = hand_login(&camp, &key, &n, Fingerprint([9; 32])).await;
    assert_eq!((status, &body["error"]), (401, &json!("bad_signature")));
    camp.stop().await;
}

#[tokio::test]
async fn nonces_are_single_use_and_must_be_issued() {
    let camp = Camp::start().await;
    let key = ring_key();
    let fp = camp.running.fingerprint;
    let n = nonce(&camp).await;
    let (status, _) = hand_login(&camp, &key, &n, fp).await;
    assert_eq!(status, 200, "honest hand-made login works");
    let (status, body) = hand_login(&camp, &key, &n, fp).await;
    assert_eq!(
        (status, &body["error"]),
        (401, &json!("nonce_invalid")),
        "replay"
    );
    let (status, body) = hand_login(&camp, &key, "made-up", fp).await;
    assert_eq!((status, &body["error"]), (401, &json!("nonce_invalid")));
    camp.stop().await;
}

#[tokio::test]
async fn malformed_requests_get_the_uniform_error_shape() {
    let camp = Camp::start().await;
    let (addr, pin) = (camp.running.local_addr, camp.running.fingerprint);
    let (status, body) = raw(
        addr,
        pin,
        "POST",
        "/api/v1/auth/login",
        Some(&json!({ "x": 1 })),
    )
    .await
    .unwrap();
    assert_eq!((status, &body["error"]), (400, &json!("bad_request")));
    camp.stop().await;
}

// ---- step 2: room tokens ----

#[tokio::test]
async fn members_get_a_livekit_token_for_one_channel() {
    let camp = Camp::start().await;
    let client = camp.client();
    let key = DeviceKey::generate();
    let session = client
        .login(&key, "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let room = client.room_token(&session, "c0").await.unwrap();
    assert_eq!(room.url, LIVEKIT_URL);

    let claims = livekit_token::TokenVerifier::with_api_key("devkey", "secret")
        .verify(&room.token)
        .expect("signed with the camp's LiveKit secret");
    assert_eq!(claims.sub, key.user_id(), "identity = user id");
    assert_eq!(claims.name, "Лена");
    assert_eq!(claims.video.room, "c0", "only this channel");
    assert!(claims.video.room_join);
    assert_eq!(claims.video.can_publish, Some(true));

    assert_eq!(
        api_error(client.room_token(&session, "lobby").await),
        ErrorCode::NotFound
    );
    camp.stop().await;
}

#[tokio::test]
async fn room_tokens_need_a_valid_session() {
    let camp = Camp::start().await;
    let (addr, pin) = (camp.running.local_addr, camp.running.fingerprint);
    let (status, body) = raw(addr, pin, "POST", "/api/v1/channels/c0/token", None)
        .await
        .unwrap();
    assert_eq!((status, &body["error"]), (401, &json!("unauthorized")));
    camp.stop().await;
}
