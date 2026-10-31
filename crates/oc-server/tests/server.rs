//! The server end to end: real TCP, real TLS with the client's pinning verifier,
//! real HTTP. Honest flows go through `oc-camp-client` (the code the app will use);
//! attacks are hand-made requests the honest client cannot produce.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use oc_camp_client::{CampClient, ClientError, DeviceKey, Trust};
use oc_invite::{Fingerprint, Host, Invite, InviteKind};
use oc_proto::{ChannelKind, ErrorCode, InviteKindDto, Role, API_VERSION};
use oc_server::{start, Config, LiveKit, LiveKitMode, Running};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

mod support;
use support::{data_dir, raw};

const LIVEKIT_URL: &str = "ws://127.0.0.1:7880";

fn config(data_dir: PathBuf) -> Config {
    Config {
        data_dir,
        bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        public_host: Host::Domain("camp.example".into()),
        public_port: None,
        camp_name: "Вечерний кооп".into(),
        router: None,
        certificate: None,
        livekit: livekit(LIVEKIT_URL),
    }
}

/// An external LiveKit at `url` with the `--dev` keys.
fn livekit(url: &str) -> LiveKitMode {
    LiveKit {
        url: url.to_owned(),
        api_key: "devkey".into(),
        api_secret: "secret".into(),
    }
    .into()
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
            Trust::Pin(self.running.fingerprint),
        )
    }

    fn join(&self) -> String {
        self.running.links.join.token.as_str().to_owned()
    }

    async fn stop(self) {
        self.running.stop().await;
        let _ = std::fs::remove_dir_all(self.dir);
    }
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
    assert_eq!(info.channels[0].name, "Voice");
    assert_eq!(info.channels[0].id, "c0");
    camp.stop().await;
}

#[tokio::test]
async fn a_client_pinning_another_certificate_cannot_connect() {
    let camp = Camp::start().await;
    let impostor = CampClient::at(
        &Host::Ipv4(Ipv4Addr::LOCALHOST),
        camp.running.local_addr.port(),
        Trust::Pin(Fingerprint([7; 32])),
    );
    let refused = impostor.camp().await.unwrap_err();
    assert_eq!(
        refused.certificate_problem(),
        Some(oc_camp_client::CertificateProblem::NotPinned),
        "{refused:?}"
    );
    camp.stop().await;
}

#[tokio::test]
async fn links_carry_the_public_address_and_the_real_fingerprint() {
    let camp = Camp::start().await;
    let join: Invite = camp.running.links.join.to_string().parse().unwrap();
    assert_eq!(join.host, Host::Domain("camp.example".into()));
    assert_eq!(join.port, camp.running.local_addr.port());
    assert_eq!(join.kind, InviteKind::Join);
    assert_eq!(join.fingerprint, Some(camp.running.fingerprint));
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
    let signature = key.sign(&oc_proto::login_message(
        oc_proto::CampKey::Pinned(&signed_for),
        nonce,
    ));
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
    cfg.livekit = livekit(upstream);
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
    oc_camp_client::transport::trust(&authority, Trust::Pin(Fingerprint([7; 32])));
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
    cfg.livekit = livekit("wss://127.0.0.1:7880");
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
    assert_eq!(link.fingerprint, Some(camp.running.fingerprint));

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
    fake_twirp_logged(room).await.0
}

/// Twirp methods the camp called, in order (`DeleteRoom`, …).
type Calls = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

/// [`fake_twirp`] that also records which methods were called.
async fn fake_twirp_logged(room: Room) -> (String, Calls) {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let calls: Calls = std::sync::Arc::default();
    let log = calls.clone();

    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let room = room.clone();
            let log = log.clone();
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
                if let Some(method) = path.rsplit('/').next() {
                    log.lock().unwrap().push(method.to_owned());
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
    (format!("ws://{addr}"), calls)
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
        let oc_proto::CampEvent::State(state) = event else {
            continue;
        };
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
        .login(&host_key, "Дима", Some(&camp.join()))
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
        (dima.name.as_str(), dima.role),
        ("Дима", Role::Member),
        "names and roles travel with the snapshot"
    );
    let angelina = state
        .members
        .iter()
        .find(|m| m.user_id == friend.user_id)
        .unwrap();
    assert_eq!(angelina.role, Role::Member);

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
    let oc_proto::CampEvent::State(first) = first else {
        panic!("the first event is the snapshot, got {first:?}")
    };
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

// ---- step 6: what the server window reads ----

#[tokio::test]
async fn the_host_sees_the_camp_state_without_a_client() {
    let camp = Camp::start().await;
    let mut state = camp.running.camp_state();
    assert!(state.borrow().members.is_empty(), "a fresh camp");
    let session = camp
        .client()
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let seen = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.wait_for(|s| s.members.iter().any(|m| m.user_id == session.user_id)),
    )
    .await
    .expect("the next snapshot has her")
    .is_ok();
    assert!(seen);
    assert_eq!(
        camp.running.livekit_pid_changes().map(|_| ()),
        None,
        "external mode"
    );
    camp.stop().await;
}

// ---- leaving a camp ----

#[tokio::test]
async fn a_member_who_leaves_is_forgotten_and_needs_a_new_invite() {
    let camp = Camp::start().await;
    let client = camp.client();
    let key = DeviceKey::generate();
    let session = client
        .login(&key, "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let mut state = camp.running.camp_state();
    let seen = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.wait_for(|s| s.members.iter().any(|m| m.user_id == session.user_id)),
    )
    .await
    .expect("she shows up first")
    .is_ok();
    assert!(seen);

    client.leave(&session).await.unwrap();

    assert_eq!(
        api_error(client.events(&session).await.map(|_| ())),
        ErrorCode::Unauthorized,
        "the session ended with the membership"
    );
    assert_eq!(
        api_error(client.leave(&session).await),
        ErrorCode::Unauthorized,
        "leaving twice is not a thing"
    );
    let gone = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.wait_for(|s| s.members.iter().all(|m| m.user_id != session.user_id)),
    )
    .await
    .expect("the next snapshot drops her")
    .is_ok();
    assert!(gone);
    assert_eq!(
        api_error(client.login(&key, "Лена", None).await),
        ErrorCode::InviteRequired,
        "the same device comes back only with an invite"
    );
    client
        .login(&key, "Лена", Some(&camp.join()))
        .await
        .expect("and with one, it does");
    camp.stop().await;
}

// ---- ADR-0026: the owner manages the camp ----

/// The next snapshot that satisfies `ready`, within `within` — shorter than a poll round
/// proves the change was published at once.
async fn state_within(
    events: &mut oc_camp_client::CampEvents,
    within: std::time::Duration,
    ready: impl Fn(&oc_proto::CampState) -> bool,
) -> oc_proto::CampState {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let event = tokio::time::timeout_at(deadline, events.next())
            .await
            .expect("the change within the time")
            .expect("stream open")
            .expect("valid event");
        let oc_proto::CampEvent::State(state) = event else {
            continue;
        };
        if ready(&state) {
            return state;
        }
    }
}

#[tokio::test]
async fn the_owners_changes_reach_members_at_once() {
    let camp = camp_with_upstream(&fake_twirp(Room::default()).await).await;
    let client = camp.client();
    let lena = client
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let mut events = client.events(&lena).await.unwrap();
    state_where(&mut events, |s| s.members.len() == 1).await;
    let manager = camp.running.manager();
    let soon = std::time::Duration::from_millis(600);

    manager.rename_camp("  Кооп на выходных ").await.unwrap();
    state_within(&mut events, soon, |s| s.name == "Кооп на выходных").await;

    manager
        .set_role(&lena.user_id, Role::Moderator)
        .await
        .unwrap();
    state_within(&mut events, soon, |s| s.members[0].role == Role::Moderator).await;
    let again = client.login(&DeviceKey::generate(), "x", None).await;
    assert!(again.is_err(), "roles never come from logging in");

    let added = manager
        .create_channel("Катка", ChannelKind::Voice)
        .await
        .unwrap();
    let state = state_within(&mut events, soon, |s| {
        s.channels.iter().any(|c| c.id == added.id)
    })
    .await;
    let names: Vec<_> = state.channels.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["Voice", "Chat", "Катка", "AFK"],
        "new channels go before AFK"
    );

    manager.rename_channel(&added.id, "Катка 2").await.unwrap();
    state_within(&mut events, soon, |s| {
        s.channels.iter().any(|c| c.name == "Катка 2")
    })
    .await;

    assert!(matches!(
        manager.set_role("nobody", Role::Admin).await,
        Err(oc_server::ManageError::NotFound)
    ));
    assert!(matches!(
        manager.rename_camp("   ").await,
        Err(oc_server::ManageError::BadName(_))
    ));
    camp.stop().await;
}

#[tokio::test]
async fn removing_a_campfire_closes_its_room_and_afk_stays() {
    let (upstream, calls) = fake_twirp_logged(Room::default()).await;
    let camp = camp_with_upstream(&upstream).await;
    let manager = camp.running.manager();

    assert!(matches!(
        manager.delete_channel("afk").await,
        Err(oc_server::ManageError::AfkIsPermanent)
    ));
    manager.delete_channel("c1").await.unwrap(); // Chat: no room to close
    assert!(!calls.lock().unwrap().iter().any(|c| c == "DeleteRoom"));

    manager.delete_channel("c0").await.unwrap();
    assert!(
        calls.lock().unwrap().iter().any(|c| c == "DeleteRoom"),
        "the campfire's room is closed: whoever sat there is disconnected"
    );
    // The change is published by the presence loop right after, not before the call
    // returns: wait for that snapshot instead of reading the current one.
    let mut state = camp.running.camp_state();
    let only_afk = tokio::time::timeout(
        std::time::Duration::from_millis(600),
        state.wait_for(|s| s.channels.iter().map(|c| c.id.as_str()).eq(["afk"])),
    )
    .await
    .expect("only AFK is left, within a moment")
    .is_ok();
    assert!(only_afk);
    assert!(matches!(
        manager.delete_channel("c0").await,
        Err(oc_server::ManageError::NotFound)
    ));

    // A removed campfire hands out no more room tokens.
    let client = camp.client();
    let session = client
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    assert_eq!(
        api_error(client.room_token(&session, "c0").await),
        ErrorCode::NotFound
    );
    camp.stop().await;
}

// ---- ADR-0028: moderation ----

/// The next event that is not a snapshot (directives come between snapshots).
async fn next_directive(events: &mut oc_camp_client::CampEvents) -> Option<oc_proto::CampEvent> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let event = tokio::time::timeout_at(deadline, events.next())
            .await
            .expect("a directive within 5 s")?
            .expect("valid event");
        if !matches!(event, oc_proto::CampEvent::State(_)) {
            return Some(event);
        }
    }
}

fn can_publish(token: &str) -> Option<bool> {
    livekit_token::TokenVerifier::with_api_key("devkey", "secret")
        .verify(token)
        .unwrap()
        .video
        .can_publish
}

/// ADR-0035: a member sends an avatar; everyone's snapshot names it by hash and members
/// fetch the same bytes. Only PNG/WebP within the size limit is kept.
#[tokio::test]
async fn avatars_are_kept_by_the_camp_and_shown_to_members() {
    let room: Room = std::sync::Arc::default();
    let (upstream, _calls) = fake_twirp_logged(room).await;
    let camp = camp_with_upstream(&upstream).await;
    let client = camp.client();
    let lena = client
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let bob = client
        .login(&DeviceKey::generate(), "Боб", Some(&camp.join()))
        .await
        .unwrap();
    let mut bob_events = client.events(&bob).await.unwrap();

    let picture = b"RIFF\x1a\0\0\0WEBPVP8 \x0e\0\0\0lena's face".to_vec();
    client.set_avatar(&lena, picture.clone()).await.unwrap();
    let hash = oc_proto::avatar_hash(&picture);
    state_where(&mut bob_events, |s| {
        s.members
            .iter()
            .any(|m| m.user_id == lena.user_id && m.avatar.as_deref() == Some(hash.as_str()))
    })
    .await;
    assert_eq!(client.avatar(&bob, &lena.user_id).await.unwrap(), picture);
    assert_eq!(
        api_error(client.avatar(&bob, &bob.user_id).await),
        ErrorCode::NotFound,
        "Боб has none"
    );

    // The camp decides by the bytes, not the claim: an SVG dressed as a PNG is refused, as
    // is anything past the limit.
    assert_eq!(
        raw_put_avatar(&camp, &lena, "image/png", b"<svg onload=alert(1)>").await,
        400
    );
    let mut huge = b"\x89PNG\r\n\x1a\n".to_vec();
    huge.resize(oc_proto::MAX_AVATAR_BYTES + 1, 0);
    assert_eq!(raw_put_avatar(&camp, &lena, "image/png", &huge).await, 400);
    assert_eq!(
        client.avatar(&bob, &lena.user_id).await.unwrap(),
        picture,
        "kept"
    );

    client.clear_avatar(&lena).await.unwrap();
    state_where(&mut bob_events, |s| {
        s.members
            .iter()
            .any(|m| m.user_id == lena.user_id && m.avatar.is_none())
    })
    .await;
}

/// `PUT /api/v1/members/me/avatar` with whatever body: the status code.
async fn raw_put_avatar(
    camp: &Camp,
    session: &oc_camp_client::Session,
    mime: &str,
    body: &[u8],
) -> u16 {
    support::put_bytes(
        camp.running.local_addr,
        camp.running.fingerprint,
        "/api/v1/members/me/avatar",
        session.token(),
        mime,
        body,
    )
    .await
    .unwrap()
}

/// ADR-0033: from a client, moderators and admins kick and silence lower ranks only;
/// LiveKit enforces the silence, the kicked member is told.
#[tokio::test]
async fn moderators_kick_and_silence_lower_ranks_from_their_client() {
    let room: Room = std::sync::Arc::default();
    let (upstream, calls) = fake_twirp_logged(room.clone()).await;
    let camp = camp_with_upstream(&upstream).await;
    let client = camp.client();
    let lena = client
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let bob = client
        .login(&DeviceKey::generate(), "Боб", Some(&camp.join()))
        .await
        .unwrap();
    let mut lena_events = client.events(&lena).await.unwrap();
    room.lock().unwrap().push((lena.user_id.clone(), false)); // Лена в голосе, в c0
    let manager = camp.running.manager();
    let updates = || {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| *c == "UpdateParticipant")
            .count()
    };

    // A member governs nobody.
    assert_eq!(
        api_error(client.set_silenced(&bob, &lena.user_id, true).await),
        ErrorCode::Forbidden
    );
    assert_eq!(
        api_error(client.kick(&bob, &lena.user_id).await),
        ErrorCode::Forbidden
    );

    // A moderator silences a member: saved, seen by all, and LiveKit takes the mic down.
    manager
        .set_role(&bob.user_id, Role::Moderator)
        .await
        .unwrap();
    client
        .set_silenced(&bob, &lena.user_id, true)
        .await
        .unwrap();
    state_where(&mut lena_events, |s| {
        s.members
            .iter()
            .any(|m| m.user_id == lena.user_id && m.silenced)
    })
    .await;
    assert_eq!(updates(), 1);
    client
        .set_silenced(&bob, &lena.user_id, false)
        .await
        .unwrap();
    assert_eq!(updates(), 2, "lifted in the room too");

    // ...but not another moderator, nor himself.
    manager
        .set_role(&lena.user_id, Role::Moderator)
        .await
        .unwrap();
    assert_eq!(
        api_error(client.set_silenced(&bob, &lena.user_id, true).await),
        ErrorCode::Forbidden
    );
    assert_eq!(
        api_error(client.kick(&bob, &bob.user_id).await),
        ErrorCode::Forbidden
    );
    assert_eq!(
        api_error(client.kick(&bob, "nobody").await),
        ErrorCode::NotFound
    );

    // An admin kicks a moderator: she is told and gone.
    manager.set_role(&bob.user_id, Role::Admin).await.unwrap();
    client.kick(&bob, &lena.user_id).await.unwrap();
    assert_eq!(
        next_directive(&mut lena_events).await,
        Some(oc_proto::CampEvent::Kicked)
    );
    assert_eq!(
        api_error(client.events(&lena).await.map(|_| ())),
        ErrorCode::Unauthorized
    );
}

#[tokio::test]
async fn moderators_move_people_from_their_client_admins_only_by_admins() {
    let room: Room = std::sync::Arc::default();
    let (upstream, _calls) = fake_twirp_logged(room.clone()).await;
    let camp = camp_with_upstream(&upstream).await;
    let client = camp.client();
    let lena = client
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let bob = client
        .login(&DeviceKey::generate(), "Боб", Some(&camp.join()))
        .await
        .unwrap();
    let mut lena_events = client.events(&lena).await.unwrap();
    room.lock().unwrap().push((lena.user_id.clone(), false)); // Лена в голосе, в c0
    let manager = camp.running.manager();
    let katka = manager
        .create_channel("Катка", ChannelKind::Voice)
        .await
        .unwrap();

    // ADR-0029: a member may not move others; a moderator may, over
    // their own rank and below; nobody moves an admin but an admin.
    assert_eq!(
        api_error(client.move_member(&bob, &lena.user_id, "c0").await),
        ErrorCode::Forbidden
    );
    manager
        .set_role(&bob.user_id, oc_proto::Role::Moderator)
        .await
        .unwrap();
    client
        .move_member(&bob, &lena.user_id, &katka.id)
        .await
        .unwrap();
    assert_eq!(
        next_directive(&mut lena_events).await,
        Some(oc_proto::CampEvent::Moved {
            channel_id: katka.id.clone()
        })
    );
    assert_eq!(
        api_error(client.move_member(&bob, &bob.user_id, "c0").await),
        ErrorCode::NotInVoice
    );
    assert_eq!(
        api_error(client.move_member(&bob, &lena.user_id, "c1").await),
        ErrorCode::BadRequest,
        "a chat has no voice"
    );
    assert_eq!(
        api_error(client.move_member(&bob, "nobody", "c0").await),
        ErrorCode::NotFound
    );
    manager
        .set_role(&lena.user_id, oc_proto::Role::Admin)
        .await
        .unwrap();
    assert_eq!(
        api_error(client.move_member(&bob, &lena.user_id, "c0").await),
        ErrorCode::Forbidden,
        "a moderator does not move an admin"
    );
}

#[tokio::test]
async fn moderation_reaches_the_member_and_livekit_enforces_it() {
    let room: Room = std::sync::Arc::default();
    let (upstream, calls) = fake_twirp_logged(room.clone()).await;
    let camp = camp_with_upstream(&upstream).await;
    let client = camp.client();
    let lena = client
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let bob = client
        .login(&DeviceKey::generate(), "Боб", Some(&camp.join()))
        .await
        .unwrap();
    let mut lena_events = client.events(&lena).await.unwrap();
    let mut bob_events = client.events(&bob).await.unwrap();
    room.lock().unwrap().push((lena.user_id.clone(), false)); // Лена в голосе, в c0
    let manager = camp.running.manager();
    let calls_of = |method: &str| {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| *c == method)
            .count()
    };

    // Silenced: the snapshot says so, LiveKit takes her mic down, new tokens listen only.
    manager.set_silenced(&lena.user_id, true).await.unwrap();
    state_where(&mut lena_events, |s| {
        s.members
            .iter()
            .any(|m| m.user_id == lena.user_id && m.silenced)
    })
    .await;
    assert_eq!(calls_of("UpdateParticipant"), 1);
    let token = client.room_token(&lena, "c0").await.unwrap();
    assert_eq!(can_publish(&token.token), Some(false));
    manager.set_silenced(&lena.user_id, false).await.unwrap();
    assert_eq!(calls_of("UpdateParticipant"), 2, "lifted in the room too");
    let token = client.room_token(&lena, "c0").await.unwrap();
    assert_eq!(can_publish(&token.token), Some(true));

    // Moved: only she is told; chats and people outside voice are refused.
    let katka = manager
        .create_channel("Катка", ChannelKind::Voice)
        .await
        .unwrap();
    manager.move_member(&lena.user_id, &katka.id).await.unwrap();
    assert_eq!(
        next_directive(&mut lena_events).await,
        Some(oc_proto::CampEvent::Moved {
            channel_id: katka.id.clone()
        })
    );
    assert!(matches!(
        manager.move_member(&lena.user_id, "c1").await,
        Err(oc_server::ManageError::NotACampfire)
    ));
    assert!(matches!(
        manager.move_member(&bob.user_id, &katka.id).await,
        Err(oc_server::ManageError::NotInVoice)
    ));

    // Kicked: told, stream closed, out of the room, back only with an invite.
    manager.kick(&lena.user_id).await.unwrap();
    assert_eq!(
        next_directive(&mut lena_events).await,
        Some(oc_proto::CampEvent::Kicked)
    );
    let after = tokio::time::timeout(std::time::Duration::from_secs(2), lena_events.next())
        .await
        .expect("the stream ends");
    assert!(after.is_none(), "closed after the kick, got {after:?}");
    assert_eq!(calls_of("RemoveParticipant"), 1);
    assert_eq!(
        api_error(client.events(&lena).await.map(|_| ())),
        ErrorCode::Unauthorized
    );

    // Bob's stream goes on: he sees Лена gone, and none of what was meant for her.
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), bob_events.next())
            .await
            .expect("Bob's snapshots keep coming")
            .expect("stream open")
            .expect("valid event");
        let oc_proto::CampEvent::State(state) = event else {
            panic!("a directive for someone else reached Bob: {event:?}")
        };
        if state.members.len() == 1 && state.members[0].user_id == bob.user_id {
            break;
        }
    }
    camp.stop().await;
}

// ---- the owner's commands (ADR-0041) ----

/// The control's snapshot once `ready` holds (it follows the camp's once-a-second state).
async fn control_status_where(
    control: &oc_server::control::ControlClient,
    ready: impl Fn(&oc_proto::CampState) -> bool,
) -> oc_server::control::Status {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let status = control.status().await.unwrap();
        if ready(&status.state) {
            return status;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the expected state within 5 s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn the_owner_commands_the_running_camp_from_its_data_directory() {
    use oc_server::control::{CallError, Command, ControlClient, CONTROL_FILE};

    let free = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let upstream = format!("ws://{}", free.local_addr().unwrap());
    drop(free);
    let camp = camp_with_upstream(&upstream).await;
    let client = camp.client();
    let lena = DeviceKey::generate();
    client
        .login(&lena, "Лена", Some(&camp.join()))
        .await
        .unwrap();

    let control = ControlClient::find(&camp.dir).unwrap();
    let status = control_status_where(&control, |s| s.members.len() == 1).await;
    assert_eq!(status.link, camp.running.links.join.to_string());

    // A name in any case finds the person; the role is in the camp at once.
    let said = control
        .send(&Command::Role {
            who: "лена".into(),
            role: Role::Moderator,
        })
        .await
        .unwrap();
    assert!(said.contains("Лена"), "{said}");
    // The answer comes once the camp shows the change: the next status has it.
    let shown = control.status().await.unwrap().state;
    assert_eq!(shown.members[0].role, Role::Moderator);
    control
        .send(&Command::AddChannel {
            name: "Катка".into(),
            text: false,
        })
        .await
        .unwrap();
    let moved = control
        .send(&Command::Move {
            who: "Лена".into(),
            channel: "катка".into(),
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&moved, CallError::Refused(why) if why.contains("не в голосовом")),
        "the new channel is found at once; Лена is just not in voice: {moved:?}"
    );
    let again = client.login(&lena, "Лена", None).await.unwrap();
    assert_eq!(again.role, Role::Moderator);

    // Someone unknown is refused in words, not with a crash.
    let refused = control
        .send(&Command::Kick {
            who: "Боб".into()
        })
        .await
        .unwrap_err();
    assert!(matches!(refused, CallError::Refused(_)), "{refused:?}");

    // Open: a stranger comes in without an invite.
    control.send(&Command::Open { open: true }).await.unwrap();
    let stranger = client.login(&DeviceKey::generate(), "Боб", None).await;
    assert!(stranger.is_ok(), "{stranger:?}");

    // Only the key in the data directory opens the control.
    let file = camp.dir.join(CONTROL_FILE);
    let real = std::fs::read_to_string(&file).unwrap();
    let mut forged: Value = serde_json::from_str(&real).unwrap();
    forged["key"] = json!("not-the-key");
    std::fs::write(&file, forged.to_string()).unwrap();
    let forged = ControlClient::find(&camp.dir).unwrap().status().await;
    assert!(matches!(forged, Err(CallError::Refused(_))), "{forged:?}");
    std::fs::write(&file, real).unwrap();

    // A stopped camp leaves no control behind.
    let dir = camp.dir.clone();
    camp.running.stop().await;
    assert!(!dir.join(CONTROL_FILE).exists());
    assert!(matches!(
        ControlClient::find(&dir),
        Err(CallError::NotRunning)
    ));
    let _ = std::fs::remove_dir_all(dir);
}

// ---- a domain with a certificate from an authority (ADR-0042) ----

/// A LiveKit address nothing listens on: presence fails at once instead of timing out.
fn livekit_down() -> String {
    let free = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    format!("ws://{}", free.local_addr().unwrap())
}

/// Listens on `[::1]`: Windows resolves `localhost` there first and takes ~2 s to give up
/// on a refused address before it tries `127.0.0.1`.
fn on_localhost(dir: PathBuf) -> Config {
    let mut cfg = config(dir);
    cfg.bind = SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, 0));
    cfg
}

/// A camp at `localhost` (it resolves without DNS) with a certificate a made-up
/// authority issued for it; the client trusts that authority.
async fn camp_on_a_domain(upstream: &str) -> (Camp, Trust) {
    let dir = data_dir();
    let (roots, files) = support::issue_certificate(&dir.join("letsencrypt"), "localhost");
    let mut cfg = on_localhost(dir.clone());
    cfg.public_host = Host::Domain("localhost".into());
    cfg.certificate = Some(files);
    cfg.livekit = livekit(upstream);
    let camp = Camp {
        running: start(cfg).await.unwrap(),
        dir,
    };
    (camp, Trust::Authorities(roots))
}

#[tokio::test]
async fn on_a_domain_the_link_has_no_fingerprint_and_friends_come_by_the_name() {
    let (camp, trust) = camp_on_a_domain(&livekit_down()).await;
    let port = camp.running.local_addr.port();
    let link = camp.running.links.join.to_string();
    assert_eq!(
        link,
        format!("ourcamp://localhost:{port}/join/{}", camp.join())
    );
    assert_eq!(
        camp.running.links.open.to_string(),
        format!("ourcamp://localhost:{port}")
    );

    let host = Host::Domain("localhost".into());
    let client = CampClient::at(&host, port, trust);
    assert_eq!(client.camp().await.unwrap().name, "Вечерний кооп");
    let key = DeviceKey::generate();
    let session = client
        .login(&key, "Лена", Some(&camp.join()))
        .await
        .unwrap();
    assert_eq!(session.user_id, key.user_id());
    let mut events = client.events(&session).await.unwrap();
    state_where(&mut events, |s| s.members.len() == 1).await;
    let shared = client.current_invite(&session).await.unwrap();
    assert_eq!(
        shared.fingerprint, None,
        "members share the link without a pin"
    );

    // Old links with the pin keep working: the camp answers them with its own certificate.
    let pinned = CampClient::at(&host, port, Trust::Pin(camp.running.fingerprint));
    let again = pinned.login(&key, "Лена", None).await.unwrap();
    assert_eq!(again.user_id, key.user_id());
    camp.stop().await;
}

#[tokio::test]
async fn without_the_authority_a_domain_camp_is_refused() {
    let (camp, _) = camp_on_a_domain(&livekit_down()).await;
    let port = camp.running.local_addr.port();
    // This computer's roots do not include the made-up authority.
    let stranger = CampClient::at(&Host::Domain("localhost".into()), port, Trust::of(None));
    let refused = stranger.camp().await.unwrap_err();
    assert_eq!(
        refused.certificate_problem(),
        Some(oc_camp_client::CertificateProblem::UnknownIssuer),
        "{refused:?}"
    );

    // And a camp with only its own certificate is no domain camp.
    let dir = data_dir();
    let plain = Camp {
        running: start(on_localhost(dir.clone())).await.unwrap(),
        dir,
    };
    let (roots, _) = support::issue_certificate(&plain.dir.join("ca"), "localhost");
    let by_name = CampClient::at(
        &Host::Domain("localhost".into()),
        plain.running.local_addr.port(),
        Trust::Authorities(roots),
    );
    let refused = by_name.camp().await.unwrap_err();
    assert!(refused.certificate_problem().is_some(), "{refused:?}");
    plain.stop().await;
    camp.stop().await;
}

#[tokio::test]
async fn a_login_signed_for_another_port_of_the_domain_is_rejected() {
    let (camp, _) = camp_on_a_domain(&livekit_down()).await;
    let port = camp.running.local_addr.port();
    let key = ring_key();
    let signed = |nonce: String, port: u16| {
        let signature = key.sign(&oc_proto::login_message(
            oc_proto::CampKey::Named {
                host: "localhost",
                port,
            },
            &nonce,
        ));
        json!({
            "publicKey": oc_proto::encode_bytes(key.public_key().as_ref()),
            "name": "Лена",
            "nonce": nonce,
            "signature": oc_proto::encode_bytes(signature.as_ref()),
            "invite": camp.join(),
        })
    };
    let (addr, pin) = (camp.running.local_addr, camp.running.fingerprint);
    let send = |body: Value| async move {
        raw(addr, pin, "POST", "/api/v1/auth/login", Some(&body))
            .await
            .unwrap()
    };
    let (status, body) = send(signed(nonce(&camp).await, port + 1)).await;
    assert_eq!((status, &body["error"]), (401, &json!("bad_signature")));
    let (status, _) = send(signed(nonce(&camp).await, port)).await;
    assert_eq!(status, 200, "signed for this camp's name");
    camp.stop().await;
}

#[tokio::test]
async fn voice_goes_to_a_domain_camp_checked_by_its_authority() {
    let upstream = fake_livekit().await;
    let (camp, trust) = camp_on_a_domain(&upstream).await;
    let client = CampClient::at(
        &Host::Domain("localhost".into()),
        camp.running.local_addr.port(),
        trust,
    );
    let voice = client.voice_url(); // registers the camp's trust for the transport
    assert!(voice.starts_with("wss://localhost:"), "{voice}");
    let conn = signal(format!("{voice}/rtc/v1?access_token=t"))
        .await
        .unwrap();
    assert!(conn.recv().await.unwrap().unwrap().starts_with(b"/rtc/v1"));
    conn.close().await;
    camp.stop().await;
}

// ---- chats (M5, ADR-0044) ----

/// A camp (its LiveKit down: chats need none) with two members in it.
async fn camp_with_two() -> (
    Camp,
    CampClient,
    oc_camp_client::Session,
    oc_camp_client::Session,
) {
    let camp = camp_with_upstream(&livekit_down()).await;
    let client = camp.client();
    let lena = client
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let bob = client
        .login(&DeviceKey::generate(), "Боб", Some(&camp.join()))
        .await
        .unwrap();
    (camp, client, lena, bob)
}

fn text(s: &str) -> oc_proto::NewMessage {
    oc_proto::NewMessage {
        text: s.into(),
        photo: None,
    }
}

/// The next chat message on an event stream, skipping snapshots.
async fn next_message(events: &mut oc_camp_client::CampEvents) -> oc_proto::ChatMessage {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let event = tokio::time::timeout_at(deadline, events.next())
            .await
            .expect("a message within 5 s")
            .expect("stream open")
            .expect("valid event");
        if let oc_proto::CampEvent::Message(message) = event {
            return message;
        }
    }
}

#[tokio::test]
async fn a_message_reaches_everyone_and_stays_in_the_chat() {
    let (camp, client, lena, bob) = camp_with_two().await;
    let mut bobs = client.events(&bob).await.unwrap();

    let sent = client
        .post_message(&lena, "c1", &text("Привет, костёр  \n"))
        .await
        .unwrap();
    assert_eq!(
        (sent.seq, sent.text.as_str()),
        (1, "Привет, костёр"),
        "trailing space trimmed"
    );
    assert_eq!(
        (sent.author_id.as_str(), sent.author_name.as_str()),
        (lena.user_id.as_str(), "Лена")
    );
    assert_eq!(next_message(&mut bobs).await, sent, "Боб gets it live");

    let state = state_where(&mut bobs, |s| s.chats.get("c1") == Some(&1)).await;
    assert_eq!(
        state.chats.len(),
        1,
        "the snapshot names the chat's last message"
    );

    client
        .post_message(&bob, "c1", &text("И тебе"))
        .await
        .unwrap();
    let history = client.history(&lena, "c1", None, 50).await.unwrap();
    let texts: Vec<_> = history.iter().map(|m| (m.seq, m.text.as_str())).collect();
    assert_eq!(texts, [(1, "Привет, костёр"), (2, "И тебе")]);
    let older = client.history(&lena, "c1", Some(2), 50).await.unwrap();
    assert_eq!(older, history[..1]);
    camp.stop().await;
}

#[tokio::test]
async fn only_chats_take_messages_and_only_sensible_ones() {
    let (camp, client, lena, _) = camp_with_two().await;
    assert_eq!(
        api_error(client.post_message(&lena, "c0", &text("в голос")).await),
        ErrorCode::NotFound
    );
    assert_eq!(
        api_error(client.history(&lena, "c0", None, 50).await),
        ErrorCode::NotFound
    );
    assert_eq!(
        api_error(client.post_message(&lena, "c1", &text("  \n")).await),
        ErrorCode::BadRequest
    );
    let long = "я".repeat(oc_proto::MAX_MESSAGE_CHARS + 1);
    assert_eq!(
        api_error(client.post_message(&lena, "c1", &text(&long)).await),
        ErrorCode::BadRequest
    );
    let unknown_photo = oc_proto::NewMessage {
        text: String::new(),
        photo: Some(oc_proto::Photo {
            hash: "nobody-uploaded1".into(),
            width: 10,
            height: 10,
        }),
    };
    assert_eq!(
        api_error(client.post_message(&lena, "c1", &unknown_photo).await),
        ErrorCode::BadRequest
    );
    camp.stop().await;
}

#[tokio::test]
async fn whoever_may_not_speak_reads_but_does_not_write() {
    let (camp, client, lena, bob) = camp_with_two().await;
    client
        .post_message(&lena, "c1", &text("Боб, ты тут?"))
        .await
        .unwrap();
    camp.running
        .manager()
        .set_silenced(&bob.user_id, true)
        .await
        .unwrap();
    assert_eq!(
        api_error(client.post_message(&bob, "c1", &text("да")).await),
        ErrorCode::Silenced
    );
    let png = b"\x89PNG\r\n\x1a\nnot really".to_vec();
    assert_eq!(
        api_error(client.upload_photo(&bob, png).await),
        ErrorCode::Silenced
    );
    assert_eq!(
        client.history(&bob, "c1", None, 50).await.unwrap().len(),
        1,
        "still reads"
    );
    camp.stop().await;
}

#[tokio::test]
async fn a_photo_goes_up_first_then_into_a_message() {
    let (camp, client, lena, bob) = camp_with_two().await;
    let jpeg = [b"\xFF\xD8\xFF\xE0".as_slice(), &[7; 5000]].concat();
    let hash = client.upload_photo(&lena, jpeg.clone()).await.unwrap();
    let message = oc_proto::NewMessage {
        text: "Закат".into(),
        photo: Some(oc_proto::Photo {
            hash: hash.clone(),
            width: 1600,
            height: 900,
        }),
    };
    let sent = client.post_message(&lena, "c1", &message).await.unwrap();
    assert_eq!(sent.photo, message.photo);
    assert_eq!(
        client.photo(&bob, &hash).await.unwrap(),
        jpeg,
        "the same bytes for Боб"
    );

    let too_big = [
        b"\xFF\xD8\xFF\xE0".as_slice(),
        &vec![0; oc_proto::MAX_PHOTO_BYTES],
    ]
    .concat();
    assert!(
        client.upload_photo(&lena, too_big).await.is_err(),
        "over the limit"
    );
    let gif = b"GIF89a....".to_vec();
    assert!(matches!(
        client.upload_photo(&lena, gif).await,
        Err(ClientError::Protocol(_))
    ));
    camp.stop().await;
}

// ---- what the server window watches ----

/// The window follows `camp_state` and `ports` until they close, which it takes for the
/// camp stopping. Without a router `ports` closed at once, and the window froze on its
/// first snapshot: nobody joining, nobody in voice.
#[tokio::test]
async fn the_ports_watch_stays_open_until_the_camp_stops() {
    let camp = camp_with_upstream(&livekit_down()).await;
    let mut ports = camp.running.ports();
    let quiet = tokio::time::timeout(std::time::Duration::from_millis(200), ports.changed()).await;
    assert!(quiet.is_err(), "no change and not closed: {quiet:?}");

    let mut state = camp.running.camp_state();
    camp.client()
        .login(&DeviceKey::generate(), "Лена", Some(&camp.join()))
        .await
        .unwrap();
    let joined = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            state.changed().await.unwrap();
            if state.borrow_and_update().members.len() == 1 {
                return;
            }
        }
    });
    joined.await.expect("the snapshot shows who came in");

    camp.running.stop().await;
    assert!(
        ports.changed().await.is_err(),
        "closed once the camp stopped"
    );
    let _ = std::fs::remove_dir_all(camp.dir);
}
