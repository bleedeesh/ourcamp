//! The camp's own LiveKit (`managed`, ADR-0022) with the real livekit-server:
//! `pnpm livekit:fetch` first (or `OC_LIVEKIT_BIN`). Each test takes free media ports,
//! so they run in parallel and next to a dev LiveKit in Docker.

mod support;

use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use oc_camp_client::{CampClient, DeviceKey};
use oc_invite::Host;
use oc_server::{find_livekit, start, Config, LiveKitMode, ManagedLiveKit, Running};
use support::{data_dir, raw};
use tokio::io::{AsyncBufReadExt, BufReader};

fn binary() -> PathBuf {
    std::env::var_os("OC_LIVEKIT_BIN")
        .map(PathBuf::from)
        .or_else(find_livekit)
        .expect("livekit-server: run `pnpm livekit:fetch` or set OC_LIVEKIT_BIN")
}

/// Media ports nobody uses right now.
fn free_media_ports() -> (u16, u16) {
    let tcp = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
    let udp = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
    (
        tcp.local_addr().unwrap().port(),
        udp.local_addr().unwrap().port(),
    )
}

fn config(data_dir: PathBuf, (tcp, udp): (u16, u16)) -> Config {
    Config {
        data_dir,
        bind: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        public_host: Host::Ipv4(Ipv4Addr::LOCALHOST),
        public_port: None,
        camp_name: "Свой LiveKit".into(),
        router: None,
        livekit: LiveKitMode::Managed(ManagedLiveKit {
            binary: binary(),
            rtc_tcp_port: tcp,
            rtc_udp_port: udp,
        }),
    }
}

fn client(running: &Running) -> CampClient {
    CampClient::at(
        &Host::Ipv4(Ipv4Addr::LOCALHOST),
        running.local_addr.port(),
        running.fingerprint,
    )
}

/// A room token for c0 and LiveKit's verdict on it, asked through the camp's `/rtc`.
/// 200 means LiveKit runs, is reachable through the camp, and trusts the camp's key.
async fn validate(running: &Running, session: &oc_camp_client::Session) -> u16 {
    let token = client(running).room_token(session, "c0").await.unwrap();
    let path = format!("/rtc/validate?access_token={}", token.token);
    let (status, _) = raw(running.local_addr, running.fingerprint, "GET", &path, None)
        .await
        .unwrap();
    status
}

/// Whether LiveKit answers on its media TCP port. Asked by connecting, not by binding:
/// LiveKit listens on `[::]` (both stacks), and Windows lets `0.0.0.0` be bound beside
/// it, so a successful bind proves nothing there.
async fn listening(tcp: u16) -> bool {
    let connect = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, tcp));
    matches!(
        tokio::time::timeout(Duration::from_secs(3), connect).await,
        Ok(Ok(_))
    )
}

/// LiveKit is gone once its media port refuses connections.
async fn gone(tcp: u16, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if !listening(tcp).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn kill_hard(pid: u32) {
    let pid = pid.to_string();
    let status = if cfg!(windows) {
        std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid])
            .stdout(Stdio::null())
            .status()
    } else {
        std::process::Command::new("kill")
            .args(["-KILL", &pid])
            .status()
    };
    assert!(status.unwrap().success(), "kill {pid}");
}

#[tokio::test]
#[ignore = "needs livekit-server: `pnpm livekit:fetch`"]
async fn the_camp_runs_its_own_livekit_with_a_key_of_its_own() {
    let dir = data_dir();
    let ports = free_media_ports();
    let running = start(config(dir.clone(), ports)).await.unwrap();
    assert!(running.livekit_pid().is_some());
    assert!(listening(ports.0).await, "media on the configured port");
    let join = running.links.join.token.as_str().to_owned();
    let session = client(&running)
        .login(&DeviceKey::generate(), "Дима", Some(&join))
        .await
        .unwrap();
    assert_eq!(validate(&running, &session).await, 200);

    let keys = std::fs::read_to_string(dir.join("livekit-keys.json")).unwrap();
    assert!(!keys.contains("devkey"), "a generated key, not the dev one");
    running.stop().await;
    assert!(
        gone(ports.0, Duration::from_secs(10)).await,
        "stop takes LiveKit down"
    );

    // Restarted: same keys, so the session's tokens still validate.
    let running = start(config(dir.clone(), ports)).await.unwrap();
    assert_eq!(validate(&running, &session).await, 200);
    assert_eq!(
        std::fs::read_to_string(dir.join("livekit-keys.json")).unwrap(),
        keys
    );
    running.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
#[ignore = "needs livekit-server: `pnpm livekit:fetch`"]
async fn a_crashed_livekit_comes_back() {
    let dir = data_dir();
    let running = start(config(dir.clone(), free_media_ports()))
        .await
        .unwrap();
    let join = running.links.join.token.as_str().to_owned();
    let session = client(&running)
        .login(&DeviceKey::generate(), "Лена", Some(&join))
        .await
        .unwrap();
    let first = running.livekit_pid().unwrap();

    kill_hard(first);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let second = loop {
        match running.livekit_pid() {
            Some(pid) if pid != first => break pid,
            _ if tokio::time::Instant::now() > deadline => panic!("not restarted in 15 s"),
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    };
    assert_ne!(second, first);
    assert_eq!(validate(&running, &session).await, 200, "voice works again");
    running.stop().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
#[ignore = "needs livekit-server: `pnpm livekit:fetch`"]
async fn livekit_does_not_outlive_a_killed_camp() {
    let dir = data_dir();
    let (tcp, udp) = free_media_ports();
    let mut server = tokio::process::Command::new(env!("CARGO_BIN_EXE_oc-server"))
        .arg("--data")
        .arg(&dir)
        .args(["--bind", "127.0.0.1:0", "--host", "127.0.0.1"])
        .arg("--livekit-bin")
        .arg(binary())
        .args(["--rtc-tcp-port", &tcp.to_string()])
        .args(["--rtc-udp-port", &udp.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    // Up when it says it listens; LiveKit is started before that.
    let mut log = BufReader::new(server.stderr.take().unwrap()).lines();
    let started = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(line) = log.next_line().await.unwrap() {
            if line.starts_with("[oc-server] listening on") {
                return true;
            }
        }
        false
    })
    .await
    .expect("the camp starts within 30 s");
    assert!(started, "the camp exited before listening");
    assert!(listening(tcp).await, "LiveKit is up on its media port");
    // Keep reading so the camp never blocks on a full stderr pipe.
    tokio::spawn(async move { while let Ok(Some(_)) = log.next_line().await {} });

    // No cleanup chance at all: TerminateProcess / SIGKILL.
    server.kill().await.unwrap();
    assert!(
        gone(tcp, Duration::from_secs(15)).await,
        "LiveKit went down with the camp"
    );
    let _ = std::fs::remove_dir_all(dir);
}
