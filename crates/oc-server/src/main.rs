//! `oc-server` — the camp server without a window (`--headless` mode of ADR-0016).
//!
//! ```text
//! oc-server [--data DIR] [--bind ADDR] [--host HOST] [--port PORT] [--name NAME]
//!           [--livekit-bin PATH] [--rtc-tcp-port N] [--rtc-udp-port N]      managed
//!           [--livekit-url URL [--livekit-key KEY] [--livekit-secret S]]    external
//! ```

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use oc_invite::{Host, DEFAULT_PORT};
use oc_server::network::{self, AddressKind};
use oc_server::{
    find_livekit, lan_address, start, Config, LiveKit, LiveKitMode, ManagedLiveKit,
    DEFAULT_RTC_TCP_PORT, DEFAULT_RTC_UDP_PORT,
};

const HELP: &str = "\
OurCamp Server (headless)

  --data DIR              camp data: database + TLS identity   [./camp-data]
  --bind ADDR             listen address                        [0.0.0.0:7780]
  --host HOST             address friends use, put into links   [this PC's LAN IPv4]
                          a domain or an IP, or a network found here:
                          lan, public, radmin, hamachi, tailscale (ADR-0037)
  --port PORT             port put into links                   [the bound port]
  --name NAME             name of a new camp                    [Мой сервер]

Voice — the camp's own LiveKit (managed, the default):
  --livekit-bin PATH      livekit-server to run   [next to oc-server, dev/bin, PATH]
  --rtc-tcp-port N        media over TCP                        [7881]
  --rtc-udp-port N        media over UDP                        [7882]

Voice — a LiveKit already running (external, e.g. `pnpm livekit` in Docker):
  --livekit-url URL       ws://host:port of that LiveKit
  --livekit-key KEY       its API key                           [devkey]
  --livekit-secret SECRET its API secret                        [secret]

  -h, --help              this text
";

/// Which LiveKit, as the flags say. The binary is looked up only after parsing, so
/// parsing stays a pure function.
#[derive(Debug)]
enum Voice {
    Managed {
        binary: Option<PathBuf>,
        rtc_tcp_port: u16,
        rtc_udp_port: u16,
    },
    External(LiveKit),
}

/// `--host`: an address, or a network to take this PC's address in (ADR-0037).
#[derive(Debug, PartialEq, Eq)]
enum HostArg {
    Address(Host),
    Network(AddressKind),
}

impl HostArg {
    fn parse(value: &str) -> Result<Self, String> {
        if let Some(kind) = AddressKind::parse(value) {
            return Ok(Self::Network(kind));
        }
        value
            .parse()
            .map(Self::Address)
            .map_err(|e| format!("--host: {e}"))
    }

    /// The address for links and voice; a network must be there now.
    fn resolve(self) -> Result<Host, String> {
        match self {
            Self::Address(host) => Ok(host),
            Self::Network(kind) => network::address_options()
                .into_iter()
                .find(|o| o.kind == kind)
                .map(|o| Host::Ipv4(o.ip))
                .ok_or_else(|| {
                    format!(
                        "--host {}: no such network on this PC (is the VPN on?)",
                        kind.as_str()
                    )
                }),
        }
    }
}

#[derive(Debug)]
struct Args {
    data_dir: PathBuf,
    bind: SocketAddr,
    host: Option<HostArg>,
    port: Option<u16>,
    name: String,
    voice: Voice,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut data_dir = PathBuf::from("camp-data");
    let mut bind = SocketAddr::from((Ipv4Addr::UNSPECIFIED, DEFAULT_PORT));
    let mut host: Option<HostArg> = None;
    let mut port: Option<u16> = None;
    let mut name = String::from("Мой сервер");
    let mut binary: Option<PathBuf> = None;
    let mut rtc_tcp_port: Option<u16> = None;
    let mut rtc_udp_port: Option<u16> = None;
    let mut url: Option<String> = None;
    // The defaults of `livekit-server --dev` (`pnpm livekit`).
    let mut key: Option<String> = None;
    let mut secret: Option<String> = None;

    while let Some(flag) = args.next() {
        if flag == "-h" || flag == "--help" {
            return Ok(None);
        }
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        let port_value = |v: String| v.parse::<u16>().map_err(|e| format!("{flag}: {e}"));
        match flag.as_str() {
            "--data" => data_dir = PathBuf::from(value()?),
            "--bind" => bind = value()?.parse().map_err(|e| format!("--bind: {e}"))?,
            "--host" => host = Some(HostArg::parse(&value()?)?),
            "--port" => port = Some(value()?.parse().map_err(|e| format!("--port: {e}"))?),
            "--name" => name = value()?,
            "--livekit-bin" => binary = Some(PathBuf::from(value()?)),
            "--rtc-tcp-port" => rtc_tcp_port = Some(port_value(value()?)?),
            "--rtc-udp-port" => rtc_udp_port = Some(port_value(value()?)?),
            "--livekit-url" => url = Some(value()?),
            "--livekit-key" => key = Some(value()?),
            "--livekit-secret" => secret = Some(value()?),
            other => return Err(format!("unknown argument {other} (see --help)")),
        }
    }

    let voice = if let Some(url) = url {
        if binary.is_some() || rtc_tcp_port.is_some() || rtc_udp_port.is_some() {
            return Err(
                "--livekit-url runs no LiveKit of its own: drop --livekit-bin and \
                        --rtc-*-port (they belong to the camp's own LiveKit)"
                    .into(),
            );
        }
        Voice::External(LiveKit {
            url,
            api_key: key.unwrap_or_else(|| "devkey".into()),
            api_secret: secret.unwrap_or_else(|| "secret".into()),
        })
    } else {
        if key.is_some() || secret.is_some() {
            return Err(
                "--livekit-key/--livekit-secret are for --livekit-url; the camp's \
                        own LiveKit gets a generated key"
                    .into(),
            );
        }
        Voice::Managed {
            binary,
            rtc_tcp_port: rtc_tcp_port.unwrap_or(DEFAULT_RTC_TCP_PORT),
            rtc_udp_port: rtc_udp_port.unwrap_or(DEFAULT_RTC_UDP_PORT),
        }
    };

    Ok(Some(Args {
        data_dir,
        bind,
        host,
        port,
        name,
        voice,
    }))
}

/// Warns early if an external LiveKit is not listening; the camp works without it,
/// only voice does not.
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
        eprintln!("[oc-server] LiveKit (external): {url}");
    } else {
        eprintln!("[oc-server] LiveKit is not reachable at {url} (start it with `pnpm livekit`)");
    }
}

/// The flags' voice choice, with the binary found.
fn livekit_mode(voice: Voice) -> Result<LiveKitMode, String> {
    match voice {
        Voice::External(livekit) => Ok(LiveKitMode::External(livekit)),
        Voice::Managed {
            binary,
            rtc_tcp_port,
            rtc_udp_port,
        } => {
            let binary = binary.or_else(find_livekit).ok_or(
                "livekit-server not found (next to oc-server, in dev/bin, on PATH). \
                 Get it with `pnpm livekit:fetch`, point to it with --livekit-bin PATH, \
                 or use a running one: --livekit-url ws://127.0.0.1:7880 (`pnpm livekit`)",
            )?;
            Ok(LiveKitMode::Managed(ManagedLiveKit {
                binary,
                rtc_tcp_port,
                rtc_udp_port,
            }))
        }
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
    let external_url = match &args.voice {
        Voice::External(livekit) => Some(livekit.url.clone()),
        Voice::Managed { .. } => None,
    };
    let livekit = match livekit_mode(args.voice) {
        Ok(mode) => mode,
        Err(e) => {
            eprintln!("oc-server: {e}");
            return ExitCode::FAILURE;
        }
    };
    let public_host = match args.host.map(HostArg::resolve).transpose() {
        Ok(host) => host.unwrap_or_else(lan_address),
        Err(e) => {
            eprintln!("oc-server: {e}");
            return ExitCode::FAILURE;
        }
    };
    let data_dir = std::path::absolute(&args.data_dir).unwrap_or_else(|_| args.data_dir.clone());
    let config = Config {
        data_dir: args.data_dir,
        bind: args.bind,
        public_host,
        public_port: args.port,
        camp_name: args.name,
        livekit,
    };

    let running = match start(config).await {
        Ok(running) => running,
        Err(e) => {
            eprintln!("oc-server: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(url) = external_url {
        check_livekit(&url).await;
    }

    eprintln!("[oc-server] data: {}", data_dir.display());
    eprintln!("[oc-server] listening on {} (TLS)", running.local_addr);
    eprintln!("[oc-server] fingerprint: {}", running.fingerprint);
    // The link goes to stdout so it can be piped; everything else is a log line.
    println!("Для друзей: {}", running.links.join);

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
    fn defaults_are_a_real_camp_on_7780_with_its_own_livekit() {
        let args = parse(&[]).unwrap().unwrap();
        assert_eq!(args.bind.port(), DEFAULT_PORT);
        assert_eq!(args.port, None);
        match args.voice {
            Voice::Managed {
                binary,
                rtc_tcp_port,
                rtc_udp_port,
            } => {
                assert_eq!(binary, None, "looked up later");
                assert_eq!((rtc_tcp_port, rtc_udp_port), (7881, 7882));
            }
            Voice::External(_) => panic!("managed is the default"),
        }
    }

    #[test]
    fn a_livekit_url_means_external_with_dev_keys() {
        let args = parse(&["--livekit-url", "ws://127.0.0.1:7880"])
            .unwrap()
            .unwrap();
        let Voice::External(livekit) = args.voice else {
            panic!("external")
        };
        assert_eq!(livekit.url, "ws://127.0.0.1:7880");
        assert_eq!(
            (livekit.api_key.as_str(), livekit.api_secret.as_str()),
            ("devkey", "secret")
        );
    }

    #[test]
    fn flags_of_the_two_modes_do_not_mix() {
        let err = parse(&["--livekit-url", "ws://x:1", "--rtc-udp-port", "9"]).unwrap_err();
        assert!(err.contains("--livekit-url runs no LiveKit"), "{err}");
        let err = parse(&["--livekit-key", "k"]).unwrap_err();
        assert!(err.contains("generated key"), "{err}");
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
            "--livekit-bin",
            "/opt/livekit-server",
            "--rtc-tcp-port",
            "17881",
            "--rtc-udp-port",
            "17882",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            args.host,
            Some(HostArg::Address(Host::Domain("camp.example".into())))
        );
        assert_eq!(args.port, Some(443));
        assert_eq!(args.bind, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(args.name, "Кооп");
        let Voice::Managed {
            binary,
            rtc_tcp_port,
            rtc_udp_port,
        } = args.voice
        else {
            panic!("managed")
        };
        assert_eq!(binary, Some(PathBuf::from("/opt/livekit-server")));
        assert_eq!((rtc_tcp_port, rtc_udp_port), (17881, 17882));
    }

    #[test]
    fn bad_input_is_reported() {
        assert!(parse(&["--port"]).unwrap_err().contains("needs a value"));
        assert!(parse(&["--host", "999.1.1.1"])
            .unwrap_err()
            .starts_with("--host"));
        assert!(parse(&["--rtc-udp-port", "70000"])
            .unwrap_err()
            .starts_with("--rtc-udp-port"));
        assert!(parse(&["--wat"]).unwrap_err().contains("unknown argument"));
        assert!(parse(&["--help"]).unwrap().is_none());
    }

    #[test]
    fn a_network_name_takes_this_pcs_address_there() {
        let args = parse(&["--host", "radmin"]).unwrap().unwrap();
        assert_eq!(args.host, Some(HostArg::Network(AddressKind::Radmin)));
        // A VPN absent here is said, not silently replaced.
        if !network::address_options()
            .iter()
            .any(|o| o.kind == AddressKind::Hamachi)
        {
            assert!(HostArg::Network(AddressKind::Hamachi)
                .resolve()
                .unwrap_err()
                .contains("is the VPN on?"));
        }
    }
}
