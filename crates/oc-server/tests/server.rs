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
    assert_eq!(
        client.voice_url(),
        format!("wss://127.0.0.1:{}", camp.running.local_addr.port()),
        "voice goes through the camp itself"
    );

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

// ---- step 3: /rtc proxy and the pinned LiveKit transport ----

/// Stands in for LiveKit: accepts one WebSocket, first sends back the request path and
/// `Host` it saw, then echoes every binary frame.
async fn fake_livekit() -> String {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
    use tokio_tungstenite::tungstenite::Message;

    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut seen = String::new();
                #[allow(
                    clippy::result_large_err,
                    reason = "the error type is fixed by tungstenite's handshake callback"
                )]
                let callback = |req: &Request, resp: Response| {
                    let host = req
                        .headers()
                        .get("host")
                        .and_then(|h| h.to_str().ok())
                        .unwrap_or("");
                    seen = format!("{} host={host}", req.uri());
                    Ok(resp)
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, callback).await else {
                    return;
                };
                let _ = ws.send(Message::Binary(seen.into_bytes().into())).await;
                while let Some(Ok(msg)) = ws.next().await {
                    if msg.is_binary() && ws.send(msg).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    format!("ws://{addr}")
}

async fn camp_with_upstream(upstream: &str) -> Camp {
    let dir = data_dir();
    let mut cfg = config(dir.clone());
    cfg.livekit.url = upstream.to_owned();
    Camp {
        running: start(cfg).await.unwrap(),
        dir,
    }
}

async fn signal(
    url: String,
) -> Result<std::sync::Arc<dyn livekit_net::WsConnection>, livekit_net::TransportError> {
    oc_camp_client::transport::install(oc_camp_client::transport::PlainLoopback::Deny);
    let client = livekit_net::ws_client().expect("transport registered");
    Ok(client.connect(url, Vec::new(), 5_000).await?.connection)
}

#[tokio::test]
async fn signalling_reaches_livekit_through_the_pinned_camp_port() {
    let upstream = fake_livekit().await;
    let camp = camp_with_upstream(&upstream).await;
    let voice = camp.client().voice_url(); // pins the camp for the transport

    let conn = signal(format!("{voice}/rtc/v1?access_token=t&sdk=rust"))
        .await
        .unwrap();
    let first = String::from_utf8(conn.recv().await.unwrap().unwrap()).unwrap();
    let upstream_authority = upstream.trim_start_matches("ws://");
    assert_eq!(
        first,
        format!("/rtc/v1?access_token=t&sdk=rust host={upstream_authority}"),
        "path and query preserved, Host rewritten to LiveKit"
    );
    conn.send(b"signal".to_vec()).await.unwrap();
    assert_eq!(
        conn.recv().await.unwrap().unwrap(),
        b"signal",
        "frames flow both ways"
    );
    conn.close().await;
    camp.stop().await;
}

#[tokio::test]
async fn the_transport_refuses_a_camp_pinned_to_another_certificate() {
    let upstream = fake_livekit().await;
    let camp = camp_with_upstream(&upstream).await;
    let authority = format!("127.0.0.1:{}", camp.running.local_addr.port());
    oc_camp_client::transport::pin(&authority, Fingerprint([7; 32]));
    let err = signal(format!("wss://{authority}/rtc"))
        .await
        .err()
        .expect("must refuse");
    assert!(format!("{err:?}").contains("certificate"), "{err:?}");
    camp.stop().await;
}

#[tokio::test]
async fn only_signalling_is_exposed_not_the_rest_of_livekit() {
    let upstream = fake_livekit().await;
    let camp = camp_with_upstream(&upstream).await;
    let (addr, pin) = (camp.running.local_addr, camp.running.fingerprint);
    let (status, _) = raw(
        addr,
        pin,
        "POST",
        "/twirp/livekit.RoomService/ListRooms",
        None,
    )
    .await
    .unwrap();
    assert_eq!(status, 404, "LiveKit's admin API stays on loopback");
    camp.stop().await;
}

#[tokio::test]
async fn livekit_down_is_a_bad_gateway_not_a_hang() {
    // A port nothing listens on.
    let free = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let upstream = format!("ws://{}", free.local_addr().unwrap());
    drop(free);
    let camp = camp_with_upstream(&upstream).await;
    let (addr, pin) = (camp.running.local_addr, camp.running.fingerprint);
    let (status, _) = raw(addr, pin, "GET", "/rtc/validate", None).await.unwrap();
    assert_eq!(status, 502);
    camp.stop().await;
}

#[tokio::test]
async fn a_livekit_url_with_tls_or_without_port_is_rejected_at_startup() {
    let mut cfg = config(data_dir());
    cfg.livekit.url = "wss://127.0.0.1:7880".into();
    let err = start(cfg).await.err().expect("must refuse");
    assert!(err.to_string().contains("ws://host:port"), "{err}");
}

// ---- step 4a: inviting from the app ----

#[tokio::test]
async fn members_can_copy_the_friends_link_and_it_works() {
    let camp = Camp::start().await;
    let client = camp.client();
    let session = client
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let link = client.current_invite(&session).await.unwrap();
    assert_eq!(link.kind, InviteKind::Join);
    assert_eq!(link.token.as_str(), camp.join());
    assert_eq!(link.fingerprint, camp.running.fingerprint);

    // A new friend joins with exactly that link.
    let friend = CampClient::new(&link);
    let joined = friend
        .login(&DeviceKey::generate(), "Женя", Some(link.token.as_str()))
        .await
        .unwrap();
    assert_eq!(joined.role, Role::Member);

    let stranger = oc_camp_client::Session::restore("AAAA".into(), "x".into(), Role::Member);
    assert_eq!(
        api_error(client.current_invite(&stranger).await),
        ErrorCode::Unauthorized
    );
    camp.stop().await;
}

#[tokio::test]
async fn a_saved_session_keeps_working() {
    let camp = Camp::start().await;
    let client = camp.client();
    let session = client
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let restored = oc_camp_client::Session::restore(
        session.token().to_owned(),
        session.user_id.clone(),
        session.role,
    );
    assert!(client.room_token(&restored, "c0").await.is_ok());
    camp.stop().await;
}

// ---- step 4b: live camp state ----

type Room = std::sync::Arc<std::sync::Mutex<Vec<(String, bool)>>>;

/// Stands in for LiveKit's Twirp API: one room, `c0`, holding whoever the test puts in
/// `room` as (identity, mic muted). Answers in LiveKit's own `snake_case` JSON shape.
async fn fake_twirp(room: Room) -> String {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let room = room.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(tcp);
                let mut line = String::new();
                let mut path = String::new();
                let mut length = 0usize;
                let mut authorized = false;
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let lower = line.to_ascii_lowercase();
                    if path.is_empty() {
                        path = line.split(' ').nth(1).unwrap_or("").to_owned();
                    } else if let Some(v) = lower.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    } else if lower.starts_with("authorization: bearer ") {
                        authorized = true;
                    } else if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0u8; length];
                let _ = tokio::io::AsyncReadExt::read_exact(&mut reader, &mut body).await;
                let json = if !authorized {
                    String::from(r#"{"code":"unauthenticated"}"#)
                } else if path.ends_with("/ListRooms") {
                    String::from(r#"{"rooms":[{"sid":"RM_1","name":"c0","empty_timeout":300}]}"#)
                } else {
                    let people: Vec<String> = room
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|(id, muted)| {
                            format!(
                                r#"{{"sid":"PA","identity":"{id}","state":"ACTIVE","tracks":[{{"type":"AUDIO","source":"MICROPHONE","muted":{muted}}}]}}"#
                            )
                        })
                        .collect();
                    format!(r#"{{"participants":[{}]}}"#, people.join(","))
                };
                let status = if authorized {
                    "200 OK"
                } else {
                    "401 Unauthorized"
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                );
                let _ = reader.get_mut().write_all(response.as_bytes()).await;
            });
        }
    });
    format!("ws://{addr}")
}

/// Waits for a snapshot that satisfies `ready` (snapshots are published once a second,
/// so a change shows up in the next one or two).
async fn state_where(
    events: &mut oc_camp_client::CampEvents,
    ready: impl Fn(&oc_proto::CampState) -> bool,
) -> oc_proto::CampState {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let event = tokio::time::timeout_at(deadline, events.next())
            .await
            .expect("the expected state within 5 s")
            .expect("stream open")
            .expect("valid event");
        let oc_proto::CampEvent::State(state) = event;
        if ready(&state) {
            return state;
        }
    }
}

#[tokio::test]
async fn everyone_sees_who_sits_at_which_campfire_with_roles() {
    let room: Room = std::sync::Arc::default();
    let camp = camp_with_upstream(&fake_twirp(room.clone()).await).await;
    let client = camp.client();
    let host_key = DeviceKey::generate();
    let host = client
        .login(&host_key, "Дима", Some(&camp.claim()))
        .await
        .unwrap();
    let friend_key = DeviceKey::generate();
    let friend = client
        .login(&friend_key, "Ангелина", Some(&camp.join()))
        .await
        .unwrap();

    // The friend opens the camp without joining voice: nobody at any campfire yet.
    let mut events = client.events(&friend).await.unwrap();
    let state = state_where(&mut events, |s| s.members.len() == 2).await;
    assert_eq!(state.presence, []);
    let dima = state
        .members
        .iter()
        .find(|m| m.user_id == host.user_id)
        .unwrap();
    assert_eq!(
        (dima.name.as_str(), dima.is_admin),
        ("Дима", true),
        "the crown is visible"
    );
    let angelina = state
        .members
        .iter()
        .find(|m| m.user_id == friend.user_id)
        .unwrap();
    assert!(!angelina.is_admin);

    // Dima sits down at the Lobby with his mic off: the friend sees it without joining.
    room.lock().unwrap().push((host.user_id.clone(), true));
    let state = state_where(&mut events, |s| !s.presence.is_empty()).await;
    assert_eq!(state.presence.len(), 1);
    assert_eq!(state.presence[0].channel_id, "c0");
    assert_eq!(state.presence[0].members[0].user_id, host.user_id);
    assert!(state.presence[0].members[0].mic_muted);

    // He unmutes; then leaves.
    room.lock().unwrap()[0].1 = false;
    state_where(&mut events, |s| {
        s.presence.first().is_some_and(|c| !c.members[0].mic_muted)
    })
    .await;
    room.lock().unwrap().clear();
    state_where(&mut events, |s| s.presence.is_empty()).await;
    camp.stop().await;
}

#[tokio::test]
async fn the_event_stream_is_for_members_only() {
    let camp = camp_with_upstream(&fake_twirp(Room::default()).await).await;
    let stranger = oc_camp_client::Session::restore("AAAA".into(), "x".into(), Role::Member);
    match camp.client().events(&stranger).await {
        Err(ClientError::Api(ErrorCode::Unauthorized)) => {}
        Err(other) => panic!("expected unauthorized, got {other:?}"),
        Ok(_) => panic!("expected unauthorized, got a stream"),
    }
    camp.stop().await;
}

#[tokio::test]
async fn with_livekit_down_the_camp_still_streams_its_members() {
    let free = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let upstream = format!("ws://{}", free.local_addr().unwrap());
    drop(free);
    let camp = camp_with_upstream(&upstream).await;
    let client = camp.client();
    let session = client
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let mut events = client.events(&session).await.unwrap();
    let state = state_where(&mut events, |s| s.members.len() == 1).await;
    assert_eq!(state.presence, []);
    camp.stop().await;
}

#[tokio::test]
async fn stopping_the_camp_closes_event_streams_at_once() {
    // LiveKit that accepts and never answers: a poll round hangs until its timeout, and
    // stopping must not wait for it (on Windows a refused connect alone takes ~2 s).
    let hung = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let camp = camp_with_upstream(&format!("ws://{}", hung.local_addr().unwrap())).await;
    let client = camp.client();
    let session = client
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let mut events = client.events(&session).await.unwrap();
    let first = events.next().await.unwrap().unwrap();
    let oc_proto::CampEvent::State(first) = first;
    assert_eq!(
        first.presence,
        [],
        "the first snapshot comes without LiveKit"
    );

    camp.stop().await;
    let closed = tokio::time::timeout(std::time::Duration::from_millis(500), events.next())
        .await
        .expect("closed promptly, not after a poll round");
    assert!(closed.is_none(), "a clean close, got {closed:?}");
    drop(hung);
}
