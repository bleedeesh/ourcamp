//! # oc-invite
//!
//! Links that bring people into a self-hosted camp.
//!
//! ```text
//! ourcamp://192.0.2.15/join/7k2q9m4x8v1c3b5n?fp=<43 chars>    friends
//! ourcamp://192.0.2.15/claim/<26 chars>?fp=<43 chars>           owner, one-time admin
//! ourcamp://[2001:db8::7]:7781/join/…                            IPv6, custom port
//! ```
//!
//! * **host[:port]** — where the camp server listens; the port is omitted when it is the
//!   default [`DEFAULT_PORT`].
//! * **join / claim** — an ordinary invite, or the one-time key the server prints on
//!   first start that makes whoever redeems it the camp admin.
//! * **token** — random bytes encoded in lowercase Crockford base32 (no look-alike
//!   characters, safe to read aloud).
//! * **fp** — SHA-256 fingerprint of the server's self-signed TLS certificate in
//!   unpadded base64url. The client pins it, so a link cannot be redirected to an
//!   impostor even though there is no certificate authority involved.
//!
//! Parsing is lenient where humans are involved (surrounding whitespace, upper-case
//! scheme/host/token, a trailing slash) and strict everywhere else.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

mod base32;
mod base64url;

/// URL scheme registered by the desktop client.
pub const SCHEME: &str = "ourcamp";
/// Port the camp server listens on unless configured otherwise.
pub const DEFAULT_PORT: u16 = 7780;
/// Bytes of entropy in a friends invite token (80 bits → 16 characters).
pub const JOIN_TOKEN_BYTES: usize = 10;
/// Bytes of entropy in an admin claim key (128 bits → 26 characters).
pub const CLAIM_TOKEN_BYTES: usize = 16;

/// What redeeming the link does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InviteKind {
    /// Join the camp as a regular member.
    Join,
    /// Join and become admin. The server accepts each claim key exactly once.
    ClaimAdmin,
}

impl InviteKind {
    fn path(self) -> &'static str {
        match self {
            Self::Join => "join",
            Self::ClaimAdmin => "claim",
        }
    }

    fn token_bytes(self) -> usize {
        match self {
            Self::Join => JOIN_TOKEN_BYTES,
            Self::ClaimAdmin => CLAIM_TOKEN_BYTES,
        }
    }
}

/// Where the camp server can be reached.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Host {
    /// IPv4 address.
    Ipv4(Ipv4Addr),
    /// IPv6 address (written in brackets in links).
    Ipv6(Ipv6Addr),
    /// DNS name, stored lower-case.
    Domain(String),
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ipv4(ip) => write!(f, "{ip}"),
            Self::Ipv6(ip) => write!(f, "[{ip}]"),
            Self::Domain(d) => f.write_str(d),
        }
    }
}

/// Parses a host as written in a link: `192.0.2.15`, `[2001:db8::7]`, `camp.example`.
/// The camp server uses it for its configured public address.
impl FromStr for Host {
    type Err = InviteError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s.trim())
    }
}

impl Host {
    fn parse(s: &str) -> Result<Self, InviteError> {
        if let Some(inner) = s.strip_prefix('[') {
            let inner = inner.strip_suffix(']').ok_or(InviteError::InvalidHost)?;
            return inner
                .parse::<Ipv6Addr>()
                .map(Self::Ipv6)
                .map_err(|_| InviteError::InvalidHost);
        }
        if let Ok(ip) = s.parse::<Ipv4Addr>() {
            return Ok(Self::Ipv4(ip));
        }
        let domain = s.to_ascii_lowercase();
        if is_valid_domain(&domain) {
            Ok(Self::Domain(domain))
        } else {
            Err(InviteError::InvalidHost)
        }
    }
}

/// RFC 1123 host name: dot-separated labels of 1–63 alphanumerics/hyphens, no
/// leading/trailing hyphen, at most 253 characters, not all-numeric.
fn is_valid_domain(s: &str) -> bool {
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    let labels_ok = s.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    });
    // "999.1.1.1" is a malformed IP, not a domain.
    let all_numeric = s.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    labels_ok && !all_numeric
}

/// Random secret identifying an invite or claim, as lowercase Crockford base32.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Token(String);

impl Token {
    /// Encodes random bytes produced by the server's CSPRNG.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(base32::encode(bytes))
    }

    /// The encoded token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses a token of the given kind, as typed or pasted by a person: case and the
    /// Crockford look-alikes (`O`→`0`, `I`/`L`→`1`) are normalised, length is checked.
    ///
    /// # Errors
    /// [`InviteError::InvalidToken`] for bad characters or the wrong length.
    pub fn parse(s: &str, kind: InviteKind) -> Result<Self, InviteError> {
        let bytes = base32::decode(s).ok_or(InviteError::InvalidToken)?;
        if bytes.len() != kind.token_bytes() {
            return Err(InviteError::InvalidToken);
        }
        // Re-encode to get the canonical form (lower-case, look-alikes mapped).
        Ok(Self::from_bytes(&bytes))
    }
}

// Tokens are secrets: keep them out of logs and panic messages.
impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(…)")
    }
}

/// SHA-256 fingerprint of the camp server's TLS certificate.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&base64url::encode(&self.0))
    }
}

impl FromStr for Fingerprint {
    type Err = InviteError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = base64url::decode(s).ok_or(InviteError::InvalidFingerprint)?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| InviteError::InvalidFingerprint)?;
        Ok(Self(arr))
    }
}

/// A parsed `ourcamp://` link.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Invite {
    /// Server address.
    pub host: Host,
    /// Server port.
    pub port: u16,
    /// Join or admin claim.
    pub kind: InviteKind,
    /// Secret to present to the server.
    pub token: Token,
    /// Certificate pin.
    pub fingerprint: Fingerprint,
}

impl fmt::Display for Invite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{SCHEME}://{}", self.host)?;
        if self.port != DEFAULT_PORT {
            write!(f, ":{}", self.port)?;
        }
        write!(
            f,
            "/{}/{}?fp={}",
            self.kind.path(),
            self.token.as_str(),
            self.fingerprint
        )
    }
}

impl FromStr for Invite {
    type Err = InviteError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let s = input.trim();
        let (scheme, rest) = s.split_once("://").ok_or(InviteError::NotAnInvite)?;
        if !scheme.eq_ignore_ascii_case(SCHEME) {
            return Err(InviteError::NotAnInvite);
        }

        let (path_part, query) = rest
            .split_once('?')
            .ok_or(InviteError::MissingFingerprint)?;
        let (authority, path) = path_part.split_once('/').ok_or(InviteError::InvalidPath)?;
        let (host, port) = split_host_port(authority)?;

        let path = path.strip_suffix('/').unwrap_or(path);
        let (kind_str, token_str) = path.split_once('/').ok_or(InviteError::InvalidPath)?;
        let kind = match kind_str.to_ascii_lowercase().as_str() {
            "join" => InviteKind::Join,
            "claim" => InviteKind::ClaimAdmin,
            _ => return Err(InviteError::InvalidPath),
        };
        if token_str.contains('/') {
            return Err(InviteError::InvalidPath);
        }
        let token = Token::parse(token_str, kind)?;

        let mut fingerprint = None;
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            if let Some(value) = pair.strip_prefix("fp=") {
                if fingerprint.is_some() {
                    return Err(InviteError::InvalidFingerprint);
                }
                fingerprint = Some(value.parse::<Fingerprint>()?);
            }
            // Unknown parameters are ignored so newer servers can add fields.
        }
        let fingerprint = fingerprint.ok_or(InviteError::MissingFingerprint)?;

        Ok(Self {
            host,
            port,
            kind,
            token,
            fingerprint,
        })
    }
}

fn split_host_port(authority: &str) -> Result<(Host, u16), InviteError> {
    if authority.is_empty() {
        return Err(InviteError::InvalidHost);
    }
    let (host_str, port_str) = if authority.starts_with('[') {
        match authority.find(']') {
            Some(end) => {
                let (h, rest) = authority.split_at(end + 1);
                (h, rest.strip_prefix(':'))
            }
            None => return Err(InviteError::InvalidHost),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    let port = match port_str {
        None => DEFAULT_PORT,
        Some(p) => match p.parse::<u16>() {
            Ok(0) | Err(_) => return Err(InviteError::InvalidPort),
            Ok(port) => port,
        },
    };
    Ok((Host::parse(host_str)?, port))
}

/// Why a string is not a valid invite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteError {
    /// Not an `ourcamp://` link at all.
    NotAnInvite,
    /// Host is not an IPv4/IPv6 address or a valid domain.
    InvalidHost,
    /// Port is not in 1–65535.
    InvalidPort,
    /// Path is not `/join/<token>` or `/claim/<token>`.
    InvalidPath,
    /// Token has invalid characters or the wrong length.
    InvalidToken,
    /// `fp` parameter missing.
    MissingFingerprint,
    /// `fp` is not 32 bytes of base64url, or appears twice.
    InvalidFingerprint,
}

impl fmt::Display for InviteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotAnInvite => "this is not an OurCamp link",
            Self::InvalidHost => "the server address in the link is invalid",
            Self::InvalidPort => "the port in the link is invalid",
            Self::InvalidPath => "the link must point to /join/… or /claim/…",
            Self::InvalidToken => "the invite code in the link is damaged",
            Self::MissingFingerprint => "the link has no server fingerprint",
            Self::InvalidFingerprint => "the server fingerprint in the link is damaged",
        })
    }
}

impl std::error::Error for InviteError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp() -> Fingerprint {
        let mut b = [0u8; 32];
        for (i, v) in b.iter_mut().enumerate() {
            *v = (i * 7 + 3) as u8;
        }
        Fingerprint(b)
    }

    fn join(host: Host, port: u16) -> Invite {
        Invite {
            host,
            port,
            kind: InviteKind::Join,
            token: Token::from_bytes(&[0xA5; JOIN_TOKEN_BYTES]),
            fingerprint: fp(),
        }
    }

    #[test]
    fn host_parses_like_in_links() {
        assert_eq!(
            " 192.0.2.15 ".parse::<Host>(),
            Ok(Host::Ipv4(Ipv4Addr::new(192, 0, 2, 15)))
        );
        assert_eq!(
            "[2001:db8::7]".parse::<Host>(),
            Ok(Host::Ipv6("2001:db8::7".parse().unwrap()))
        );
        assert_eq!(
            "Camp.Example".parse::<Host>(),
            Ok(Host::Domain("camp.example".into()))
        );
        assert_eq!("999.1.1.1".parse::<Host>(), Err(InviteError::InvalidHost));
    }

    #[test]
    fn round_trips_ipv4_with_default_port_omitted() {
        let inv = join(Host::Ipv4(Ipv4Addr::new(192, 0, 2, 15)), DEFAULT_PORT);
        let s = inv.to_string();
        assert!(s.starts_with("ourcamp://192.0.2.15/join/"), "{s}");
        assert_eq!(s.parse::<Invite>().unwrap(), inv);
    }

    #[test]
    fn round_trips_ipv6_with_custom_port() {
        let inv = join(Host::Ipv6("2001:db8::7".parse().unwrap()), 7781);
        let s = inv.to_string();
        assert!(s.starts_with("ourcamp://[2001:db8::7]:7781/join/"), "{s}");
        assert_eq!(s.parse::<Invite>().unwrap(), inv);
    }

    #[test]
    fn round_trips_domain_and_admin_claim() {
        let inv = Invite {
            kind: InviteKind::ClaimAdmin,
            token: Token::from_bytes(&[0x3C; CLAIM_TOKEN_BYTES]),
            ..join(Host::Domain("camp.example.org".into()), DEFAULT_PORT)
        };
        let s = inv.to_string();
        assert!(s.contains("/claim/"));
        assert_eq!(inv.token.as_str().len(), 26);
        assert_eq!(s.parse::<Invite>().unwrap(), inv);
    }

    #[test]
    fn tolerates_human_formatting() {
        let canonical = join(Host::Domain("camp.example.org".into()), DEFAULT_PORT);
        let messy = format!(
            "  OurCamp://Camp.Example.ORG/JOIN/{}/?fp={}&utm=x \n",
            canonical.token.as_str().to_ascii_uppercase(),
            canonical.fingerprint
        );
        assert_eq!(messy.parse::<Invite>().unwrap(), canonical);
    }

    #[test]
    fn crockford_look_alikes_are_accepted() {
        let inv = Invite {
            token: Token::from_bytes(&[0x00, 0x42, 0x10, 0x84, 0x21, 0x08, 0x42, 0x10, 0x84, 0x21]),
            ..join(Host::Ipv4(Ipv4Addr::LOCALHOST), DEFAULT_PORT)
        };
        let token = inv.token.as_str();
        assert!(token.contains('0') && token.contains('1'), "{token}");
        let typed_token = token.replace('0', "O").replace('1', "l");
        let typed = inv.to_string().replace(token, &typed_token);
        assert_eq!(typed.parse::<Invite>().unwrap(), inv);
    }

    #[test]
    fn rejects_malformed_links() {
        let good = join(Host::Ipv4(Ipv4Addr::LOCALHOST), DEFAULT_PORT).to_string();
        let token = good.split('/').nth(4).unwrap().split('?').next().unwrap();
        let fp = fp().to_string();
        let cases = [
            ("https://example.org/join/x?fp=y", InviteError::NotAnInvite),
            ("just text", InviteError::NotAnInvite),
            (
                &format!("ourcamp://999.1.1.1/join/{token}?fp={fp}"),
                InviteError::InvalidHost,
            ),
            (
                &format!("ourcamp://-bad.org/join/{token}?fp={fp}"),
                InviteError::InvalidHost,
            ),
            (
                &format!("ourcamp://[::1/join/{token}?fp={fp}"),
                InviteError::InvalidHost,
            ),
            (
                &format!("ourcamp://h.org:0/join/{token}?fp={fp}"),
                InviteError::InvalidPort,
            ),
            (
                &format!("ourcamp://h.org:70000/join/{token}?fp={fp}"),
                InviteError::InvalidPort,
            ),
            (
                &format!("ourcamp://h.org/kick/{token}?fp={fp}"),
                InviteError::InvalidPath,
            ),
            (
                &format!("ourcamp://h.org/join/{token}/x?fp={fp}"),
                InviteError::InvalidPath,
            ),
            (
                &format!("ourcamp://h.org/join/{token}u!?fp={fp}"),
                InviteError::InvalidToken,
            ),
            (
                &format!("ourcamp://h.org/claim/{token}?fp={fp}"),
                InviteError::InvalidToken,
            ),
            (
                &format!("ourcamp://h.org/join/{token}"),
                InviteError::MissingFingerprint,
            ),
            (
                &format!("ourcamp://h.org/join/{token}?x=1"),
                InviteError::MissingFingerprint,
            ),
            (
                &format!("ourcamp://h.org/join/{token}?fp=abc"),
                InviteError::InvalidFingerprint,
            ),
            (
                &format!("ourcamp://h.org/join/{token}?fp={fp}&fp={fp}"),
                InviteError::InvalidFingerprint,
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(input.parse::<Invite>().err(), Some(expected), "{input}");
        }
    }

    #[test]
    fn token_debug_does_not_leak_secret() {
        let t = Token::from_bytes(&[1; JOIN_TOKEN_BYTES]);
        assert_eq!(format!("{t:?}"), "Token(…)");
    }

    #[test]
    fn fingerprint_text_round_trip() {
        let s = fp().to_string();
        assert_eq!(s.len(), 43);
        assert_eq!(s.parse::<Fingerprint>().unwrap(), fp());
    }
}
