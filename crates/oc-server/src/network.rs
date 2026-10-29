//! Addresses friends may reach this PC at (ADR-0037): the local network, a VPN that joins
//! friends into one network (Radmin VPN, Hamachi, Tailscale), a public address on the
//! network card (a hosting server).
//!
//! No interface listing: the OS is asked which of our addresses it would send from
//! towards each network (a connected UDP socket sends nothing). A VPN is there when that
//! address lies inside the VPN's own range; otherwise the route went the default way.

use std::net::{Ipv4Addr, UdpSocket};

/// What kind of address an option is; decides the hint the owner sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressKind {
    /// A home or office network: friends on the same router only.
    Lan,
    /// A public address right on the network card (a hosting server).
    Public,
    /// Radmin VPN: 26.0.0.0/8.
    Radmin,
    /// Hamachi (by LogMeIn): 25.0.0.0/8.
    Hamachi,
    /// Tailscale: 100.64.0.0/10.
    Tailscale,
    /// The internet through the home router: its public address, ports opened over
    /// UPnP (ADR-0039). Found by asking the router, not by a route.
    Router,
}

impl AddressKind {
    /// Its name as configs and the window store it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lan => "lan",
            Self::Public => "public",
            Self::Radmin => "radmin",
            Self::Hamachi => "hamachi",
            Self::Tailscale => "tailscale",
            Self::Router => "router",
        }
    }

    /// The kind by that name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        [
            Self::Lan,
            Self::Public,
            Self::Radmin,
            Self::Hamachi,
            Self::Tailscale,
            Self::Router,
        ]
        .into_iter()
        .find(|k| k.as_str() == name)
    }
}

/// One address friends could use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddressOption {
    /// Local network, public, or which VPN.
    pub kind: AddressKind,
    /// This PC's address there.
    pub ip: Ipv4Addr,
}

/// A VPN: its kind, an address inside its range to route towards, and the range.
struct Vpn {
    kind: AddressKind,
    probe: Ipv4Addr,
    network: Ipv4Addr,
    prefix: u8,
}

const VPNS: [Vpn; 3] = [
    Vpn {
        kind: AddressKind::Radmin,
        probe: Ipv4Addr::new(26, 0, 0, 1),
        network: Ipv4Addr::new(26, 0, 0, 0),
        prefix: 8,
    },
    Vpn {
        kind: AddressKind::Hamachi,
        probe: Ipv4Addr::new(25, 0, 0, 1),
        network: Ipv4Addr::new(25, 0, 0, 0),
        prefix: 8,
    },
    Vpn {
        kind: AddressKind::Tailscale,
        // Tailscale's own resolver: routed into the tailnet wherever Tailscale runs.
        probe: Ipv4Addr::new(100, 100, 100, 100),
        network: Ipv4Addr::new(100, 64, 0, 0),
        prefix: 10,
    },
];

/// TEST-NET-1 (RFC 5737): never routed anywhere, so it takes the default route.
const DEFAULT_ROUTE_PROBE: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);

fn in_network(ip: Ipv4Addr, network: Ipv4Addr, prefix: u8) -> bool {
    let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
    u32::from(ip) & mask == u32::from(network) & mask
}

/// Whether friends on the internet can reach `ip`: not a private, carrier-grade NAT
/// (a "grey" address: the provider shares one public address between customers),
/// link-local or loopback one.
#[must_use]
pub fn is_public(ip: Ipv4Addr) -> bool {
    !(ip.is_private()
        || in_network(ip, Ipv4Addr::new(100, 64, 0, 0), 10)
        || ip.is_link_local()
        || ip.is_loopback()
        || ip.is_unspecified())
}

/// The kind of the default route's address: private ranges are a local network.
fn default_kind(ip: Ipv4Addr) -> AddressKind {
    // 100.64/10 is carrier-grade NAT: on a card it is a local address, not a public one.
    if is_public(ip) {
        AddressKind::Public
    } else {
        AddressKind::Lan
    }
}

/// The options from what each probe was routed from: `(default route, [per VPN])`.
fn options_from(default: Option<Ipv4Addr>, vpns: &[Option<Ipv4Addr>]) -> Vec<AddressOption> {
    let mut out = Vec::new();
    if let Some(ip) = default.filter(|ip| !ip.is_unspecified() && !ip.is_loopback()) {
        out.push(AddressOption {
            kind: default_kind(ip),
            ip,
        });
    }
    for (vpn, source) in VPNS.iter().zip(vpns) {
        if let Some(ip) = source.filter(|ip| in_network(*ip, vpn.network, vpn.prefix)) {
            if !out.iter().any(|o| o.ip == ip) {
                out.push(AddressOption { kind: vpn.kind, ip });
            }
        }
    }
    out
}

/// Which of our addresses the OS would send from towards `target`.
fn route_source(target: Ipv4Addr) -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((target, 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) => Some(ip),
        std::net::IpAddr::V6(_) => None,
    }
}

/// The default route's address (the network card the internet goes through), if any.
#[must_use]
pub fn default_address() -> Option<Ipv4Addr> {
    route_source(DEFAULT_ROUTE_PROBE).filter(|ip| !ip.is_unspecified())
}

/// The addresses friends could use, best first: the default route's, then each VPN
/// found. Empty without any network.
#[must_use]
pub fn address_options() -> Vec<AddressOption> {
    let vpns: Vec<_> = VPNS.iter().map(|v| route_source(v.probe)).collect();
    options_from(route_source(DEFAULT_ROUTE_PROBE), &vpns)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAN: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 2);

    #[test]
    fn a_vpn_counts_only_when_its_route_leaves_from_inside_its_range() {
        // Radmin installed; no Hamachi (its probe went the default way); Tailscale on.
        let options = options_from(
            Some(LAN),
            &[
                Some(Ipv4Addr::new(26, 12, 34, 56)),
                Some(LAN),
                Some(Ipv4Addr::new(100, 101, 2, 3)),
            ],
        );
        assert_eq!(
            options,
            [
                AddressOption {
                    kind: AddressKind::Lan,
                    ip: LAN
                },
                AddressOption {
                    kind: AddressKind::Radmin,
                    ip: Ipv4Addr::new(26, 12, 34, 56)
                },
                AddressOption {
                    kind: AddressKind::Tailscale,
                    ip: Ipv4Addr::new(100, 101, 2, 3)
                },
            ]
        );
    }

    #[test]
    fn a_hosting_server_has_its_public_address_on_the_card() {
        let public = Ipv4Addr::new(85, 1, 2, 3);
        assert_eq!(
            options_from(Some(public), &[Some(public), Some(public), Some(public)]),
            [AddressOption {
                kind: AddressKind::Public,
                ip: public
            }]
        );
        assert_eq!(default_kind(Ipv4Addr::new(10, 0, 0, 5)), AddressKind::Lan);
        assert_eq!(default_kind(Ipv4Addr::new(172, 20, 0, 1)), AddressKind::Lan);
        assert_eq!(
            default_kind(Ipv4Addr::new(100, 70, 1, 1)),
            AddressKind::Lan,
            "carrier-grade NAT is no public address"
        );
    }

    #[test]
    fn grey_addresses_are_no_way_in() {
        assert!(is_public(Ipv4Addr::new(85, 143, 20, 11)));
        assert!(
            !is_public(Ipv4Addr::new(100, 91, 68, 10)),
            "carrier-grade NAT"
        );
        assert!(!is_public(Ipv4Addr::new(192, 168, 1, 2)));
        assert!(!is_public(Ipv4Addr::new(10, 0, 0, 1)));
        assert!(!is_public(Ipv4Addr::new(169, 254, 1, 1)));
    }

    #[test]
    fn no_network_no_options() {
        assert_eq!(options_from(None, &[None, None, None]), []);
        assert_eq!(
            options_from(Some(Ipv4Addr::LOCALHOST), &[None, None, None]),
            []
        );
    }

    #[test]
    fn kinds_round_trip_by_name() {
        for kind in [
            AddressKind::Lan,
            AddressKind::Public,
            AddressKind::Radmin,
            AddressKind::Hamachi,
            AddressKind::Tailscale,
            AddressKind::Router,
        ] {
            assert_eq!(AddressKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(AddressKind::parse("zerotier"), None);
    }

    #[test]
    fn this_machine_has_some_answer() {
        // Whatever the network here, probing must not fail or hang.
        let _ = address_options();
    }
}
