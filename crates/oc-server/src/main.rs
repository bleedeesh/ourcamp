//! `oc-server` — the camp server without a window (`--headless` mode of ADR-0016).
//!
//! ```text
//! oc-server [--data DIR] [--bind ADDR] [--host HOST] [--port PORT] [--name NAME]
//!           [--livekit-url URL]
//! ```

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use oc_invite::{Host, DEFAULT_PORT};
use oc_server::{start, Config};

const HELP: &str = "\
OurCamp Server (headless)

  --data DIR         camp data: database + TLS identity   [./camp-data]
  --bind ADDR        listen address                        [0.0.0.0:7780]
  --host HOST        address friends use, put into links   [this PC's LAN IPv4]
  --port PORT        port put into links                   [the bound port]
  --name NAME        name of a new camp                    [Мой лагерь]
  --livekit-url URL  LiveKit to use (dev: `pnpm livekit`)   [ws://127.0.0.1:7880]
  -h, --help         this text
";

#[derive(Debug)]
struct Args {
    config: Config,
    livekit_url: String,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut data_dir = PathBuf::from("camp-data");
    let mut bind = SocketAddr::from((Ipv4Addr::UNSPECIFIED, DEFAULT_PORT));
    let mut host: Option<Host> = None;
    let mut port: Option<u16> = None;
    let mut name = String::from("Мой лагерь");
    let mut livekit_url = String::from("ws://127.0.0.1:7880");

    while let Some(flag) = args.next() {
        if flag == "-h" || flag == "--help" {
            return Ok(None);
        }
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--data" => data_dir = PathBuf::from(value()?),
            "--bind" => bind = value()?.parse().map_err(|e| format!("--bind: {e}"))?,
            "--host" => host = Some(value()?.parse().map_err(|e| format!("--host: {e}"))?),
            "--port" => port = Some(value()?.parse().map_err(|e| format!("--port: {e}"))?),
            "--name" => name = value()?,
            "--livekit-url" => livekit_url = value()?,
            other => return Err(format!("unknown argument {other} (see --help)")),
        }
    }

    Ok(Some(Args {
        config: Config {
            data_dir,
            bind,
            public_host: host.unwrap_or_else(lan_address),
            public_port: port,
            camp_name: name,
        },
        livekit_url,
    }))
}

/// This machine's LAN address: the source address the OS would pick to reach the
/// internet. `connect` on UDP only selects a route — no packet is sent.
fn lan_address() -> Host {
    let ip = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|s| {
            s.connect((Ipv4Addr::new(192, 0, 2, 1), 9))?; // TEST-NET-1, never routed
            s.local_addr()
        })
        .map(|a| a.ip());
    match ip {
        Ok(IpAddr::V4(v4)) if !v4.is_unspecified() => Host::Ipv4(v4),
        _ => Host::Ipv4(Ipv4Addr::LOCALHOST),
    }
}

/// Warns early if the LiveKit the camp will hand out is not listening. Voice rooms
/// arrive in step 2 of M3; the camp itself works without them.
async fn check_livekit(url: &str) {
    let authority = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    let reachable = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect(authority.as_str()),
    )
    .await
    .is_ok_and(|r| r.is_ok());
    if reachable {
        eprintln!("[oc-server] LiveKit: {url}");
    } else {
        eprintln!("[oc-server] LiveKit is not reachable at {url} (start it with `pnpm livekit`)");
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("oc-server: {e}");
            return ExitCode::from(2);
        }
    };

    let data_dir =
        std::path::absolute(&args.config.data_dir).unwrap_or_else(|_| args.config.data_dir.clone());
    let running = match start(args.config).await {
        Ok(running) => running,
        Err(e) => {
            eprintln!("oc-server: {e}");
            return ExitCode::FAILURE;
        }
    };
    check_livekit(&args.livekit_url).await;

    eprintln!("[oc-server] data: {}", data_dir.display());
    eprintln!("[oc-server] listening on {} (TLS)", running.local_addr);
    eprintln!("[oc-server] fingerprint: {}", running.fingerprint);
    // Links go to stdout so they can be piped; everything else is a log line.
    println!("Для друзей: {}", running.links.join);
    match &running.links.claim {
        Some(claim) => println!("Для вас (админ, одноразовая): {claim}"),
        None => eprintln!("[oc-server] admin key already redeemed"),
    }

    if let Err(e) = tokio::signal::ctrl_c().await {
        eprintln!("oc-server: cannot wait for Ctrl+C: {e}");
    }
    eprintln!("[oc-server] stopping");
    running.stop().await;
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Args>, String> {
        parse_args(args.iter().map(ToString::to_string))
    }

    #[test]
    fn defaults_are_a_real_camp_on_7780() {
        let args = parse(&[]).unwrap().unwrap();
        assert_eq!(args.config.bind.port(), DEFAULT_PORT);
        assert_eq!(args.config.public_port, None);
        assert_eq!(args.livekit_url, "ws://127.0.0.1:7880");
    }

    #[test]
    fn flags_override_defaults() {
        let args = parse(&[
            "--host",
            "camp.example",
            "--port",
            "443",
            "--bind",
            "127.0.0.1:9000",
            "--name",
            "Кооп",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(args.config.public_host, Host::Domain("camp.example".into()));
        assert_eq!(args.config.public_port, Some(443));
        assert_eq!(args.config.bind, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(args.config.camp_name, "Кооп");
    }

    #[test]
    fn bad_input_is_reported() {
        assert!(parse(&["--port"]).unwrap_err().contains("needs a value"));
        assert!(parse(&["--host", "999.1.1.1"])
            .unwrap_err()
            .starts_with("--host"));
        assert!(parse(&["--wat"]).unwrap_err().contains("unknown argument"));
        assert!(parse(&["--help"]).unwrap().is_none());
    }
}
