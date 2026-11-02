//! Which addresses a preview may reach (ADR-0077): the open internet only. A page must not be
//! able to send the app to a router's admin page and have the answer posted in a chat.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether `ip` is on the open internet.
#[must_use]
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => is_public_v6(ip),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !matches!(
        (a, b, c),
        // "This" network, private, loopback; 25 and 26 are Hamachi and Radmin VPN: friends'
        // computers, not the internet (ADR-0037); multicast, reserved, broadcast.
        (0 | 10 | 25 | 26 | 127 | 224..=255, _, _)
        // Carrier-grade NAT.
        | (100, 64..=127, _)
        // Link-local, private.
        | (169, 254, _)
        | (172, 16..=31, _)
        | (192, 168, _)
        // Protocol assignments, documentation, benchmarking.
        | (192, 0, 0 | 2)
        | (198, 18 | 19, _)
        | (198, 51, 100)
        | (203, 0, 113)
    )
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    let embedded_v4 = |high: u16, low: u16| {
        let [a, b] = high.to_be_bytes();
        let [c, d] = low.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    match segments {
        // Unspecified, loopback, documentation.
        [0, 0, 0, 0, 0, 0, 0, 0 | 1] | [0x2001, 0xDB8, ..] => false,
        // IPv4 in IPv6 clothes is judged as IPv4: mapped, translated (NAT64), 6to4.
        [0, 0, 0, 0, 0, 0xFFFF, high, low] | [0x64, 0xFF9B, 0, 0, 0, 0, high, low] => {
            is_public_v4(embedded_v4(high, low))
        }
        [0x2002, high, low, ..] => is_public_v4(embedded_v4(high, low)),
        // Only global unicast is the internet: this leaves out unique-local, link-local,
        // multicast and the rest of what is not assigned.
        [first, ..] => first & 0xE000 == 0x2000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public(ip: &str) -> bool {
        is_public(ip.parse().unwrap())
    }

    #[test]
    fn the_internet_is_public() {
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "172.32.0.1",
            "198.20.0.1",
            "100.128.0.1",
            "2606:4700::1111",
            "2a00:1450:4001::1",
            "::ffff:1.1.1.1",
            "64:ff9b::808:808",
            "2002:808:808::1",
        ] {
            assert!(public(ip), "{ip}");
        }
    }

    #[test]
    fn nothing_private_is() {
        for ip in [
            "0.0.0.0",
            "10.0.0.1",
            "25.1.2.3",
            "26.1.2.3",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "127.0.0.2",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.1",
            "192.0.2.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:192.168.1.1",
            "64:ff9b::a00:1",
            "2002:a00:1::1",
            "2001:db8::1",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "ff02::1",
        ] {
            assert!(!public(ip), "{ip}");
        }
    }
}
