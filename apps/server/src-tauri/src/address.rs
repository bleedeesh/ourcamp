//! «Адрес для друзей» (ADR-0037): which address goes into the friends link and is
//! announced for voice — the local network, a VPN (Radmin, Hamachi, Tailscale), the
//! public address of a hosting server, or one the owner types (a domain, a static IP).

use std::path::Path;

use oc_invite::Host;
use oc_server::network::{AddressKind, AddressOption};
use serde::{Deserialize, Serialize};

/// The window's own settings, next to the camp's data.
const SETTINGS_FILE: &str = "window.json";

/// What the owner chose.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AddressChoice {
    /// The network the internet goes through (as before ADR-0037).
    #[default]
    Auto,
    /// A detected network by its kind (`radmin`, `lan`, …): followed when its IP changes.
    Detected { which: String },
    /// Typed by the owner: a domain or an IP.
    Custom { host: String },
}

/// One detected address, for the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressOptionDto {
    pub kind: &'static str,
    pub ip: String,
}

/// Everything the window shows about the address.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reach {
    pub choice: AddressChoice,
    pub options: Vec<AddressOptionDto>,
    /// Why the link is not at the chosen address right now (the VPN is off).
    pub note: Option<String>,
}

impl Reach {
    pub fn new(choice: AddressChoice, options: &[AddressOption], note: Option<String>) -> Self {
        Self {
            choice,
            options: options
                .iter()
                .map(|o| AddressOptionDto {
                    kind: o.kind.as_str(),
                    ip: o.ip.to_string(),
                })
                .collect(),
            note,
        }
    }
}

/// How the owner reads a kind.
fn title(kind: AddressKind) -> &'static str {
    match kind {
        AddressKind::Lan => "Локальная сеть",
        AddressKind::Public => "Внешний адрес",
        AddressKind::Radmin => "Radmin VPN",
        AddressKind::Hamachi => "Hamachi",
        AddressKind::Tailscale => "Tailscale",
    }
}

/// The address for links and voice from the choice and what is detected now, with a
/// note when the choice cannot be honoured (it falls back to the default route).
pub fn resolve(choice: &AddressChoice, options: &[AddressOption]) -> (Host, Option<String>) {
    let default = || {
        options
            .first()
            .map_or(Host::Ipv4(std::net::Ipv4Addr::LOCALHOST), |o| {
                Host::Ipv4(o.ip)
            })
    };
    match choice {
        AddressChoice::Auto => (default(), None),
        AddressChoice::Detected { which } => {
            let kind = AddressKind::parse(which);
            if let Some(found) = kind.and_then(|k| options.iter().find(|o| o.kind == k)) {
                return (Host::Ipv4(found.ip), None);
            }
            let name: &str = kind.map_or(which, |k| title(k));
            (
                default(),
                Some(format!(
                    "{name} сейчас не найден — ссылка ведёт по основной сети. \
                     Включите его и выберите снова"
                )),
            )
        }
        AddressChoice::Custom { host } => match host.trim().parse::<Host>() {
            Ok(host) => (host, None),
            Err(_) => (
                default(),
                Some(format!(
                    "«{host}» — не адрес; ссылка ведёт по основной сети"
                )),
            ),
        },
    }
}

/// Checks a typed address before it is kept: a domain or an IP, nothing else.
///
/// # Errors
/// A sentence for the window.
pub fn check_custom(host: &str) -> Result<(), String> {
    host.trim()
        .parse::<Host>()
        .map(drop)
        .map_err(|_| "Введите домен (mycamp.duckdns.org) или IP (85.12.34.56)".into())
}

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    address: AddressChoice,
}

/// The kept choice; a missing or damaged file means the default.
pub fn load(dir: &Path) -> AddressChoice {
    std::fs::read(dir.join(SETTINGS_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
        .unwrap_or_default()
        .address
}

/// Keeps the choice.
///
/// # Errors
/// The file cannot be written.
pub fn save(dir: &Path, address: &AddressChoice) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(&Settings {
        address: address.clone(),
    })
    .map_err(std::io::Error::other)?;
    let path = dir.join(SETTINGS_FILE);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    const LAN: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 2);
    const RADMIN: Ipv4Addr = Ipv4Addr::new(26, 12, 34, 56);

    fn options() -> Vec<AddressOption> {
        vec![
            AddressOption {
                kind: AddressKind::Lan,
                ip: LAN,
            },
            AddressOption {
                kind: AddressKind::Radmin,
                ip: RADMIN,
            },
        ]
    }

    fn detected(which: &str) -> AddressChoice {
        AddressChoice::Detected {
            which: which.into(),
        }
    }

    #[test]
    fn the_chosen_network_is_followed_or_its_absence_said() {
        assert_eq!(
            resolve(&AddressChoice::Auto, &options()),
            (Host::Ipv4(LAN), None)
        );
        assert_eq!(
            resolve(&detected("radmin"), &options()),
            (Host::Ipv4(RADMIN), None)
        );
        let (host, note) = resolve(&detected("hamachi"), &options());
        assert_eq!(host, Host::Ipv4(LAN), "falls back to the main network");
        assert!(note.unwrap().starts_with("Hamachi сейчас не найден"));
        assert_eq!(
            resolve(&AddressChoice::Auto, &[]).0,
            Host::Ipv4(Ipv4Addr::LOCALHOST),
            "no network at all"
        );
    }

    #[test]
    fn a_typed_address_is_a_domain_or_an_ip() {
        assert_eq!(
            resolve(
                &AddressChoice::Custom {
                    host: " Mycamp.DuckDNS.org ".into()
                },
                &options()
            ),
            (Host::Domain("mycamp.duckdns.org".into()), None)
        );
        assert!(check_custom("85.12.34.56").is_ok());
        assert!(check_custom("ourcamp.kz").is_ok());
        assert!(check_custom("http://x/y").is_err());
        assert!(check_custom("").is_err());
    }

    #[test]
    fn the_choice_is_kept_across_runs() {
        let dir = std::env::temp_dir().join(format!("oc-window-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir), AddressChoice::Auto, "nothing kept yet");
        save(&dir, &detected("radmin")).unwrap();
        assert_eq!(load(&dir), detected("radmin"));
        std::fs::write(dir.join(SETTINGS_FILE), "garbage").unwrap();
        assert_eq!(
            load(&dir),
            AddressChoice::Auto,
            "a damaged file is the default"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
