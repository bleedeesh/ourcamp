//! # oc-tls
//!
//! A camp server has no domain and no certificate from an authority; its invite link
//! carries the SHA-256 of its self-signed certificate instead (ADR-0005). This crate
//! makes TLS clients trust **exactly** that certificate (ADR-0014):
//!
//! * the end-entity certificate must hash to the expected [`Fingerprint`];
//! * the handshake signature must verify against that certificate's key. The
//!   certificate itself is public — anyone can replay it — so the signature is what
//!   proves the peer holds the private key. Skipping it would make pinning worthless.
//!
//! Host names, validity dates and chains are deliberately ignored: the pin replaces
//! all of them.
//!
//! A camp on a **domain** may instead have a certificate from an authority (ADR-0042);
//! its link carries no pin, and [`Trust::Authorities`] checks the certificate the usual
//! way: chain to a root this computer trusts, the domain name, the validity dates.
//!
//! ```no_run
//! # let fingerprint: oc_invite::Fingerprint = "A".repeat(43).parse().unwrap();
//! let config = oc_tls::client_config(fingerprint);
//! // tokio_rustls::TlsConnector::from(config).connect(...)
//! ```

use std::sync::{Arc, LazyLock};

use oc_invite::Fingerprint;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, Error, RootCertStore, SignatureScheme,
};

/// The name pinned connections send as SNI. A camp answers it with its self-signed
/// certificate; its domain, with the certificate from an authority (ADR-0042).
pub const PINNED_NAME: &str = "ourcamp";

/// SHA-256 of a DER certificate, as written in invite links.
#[must_use]
pub fn fingerprint(cert: &CertificateDer<'_>) -> Fingerprint {
    let digest = ring::digest::digest(&ring::digest::SHA256, cert.as_ref());
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(digest.as_ref());
    Fingerprint(bytes)
}

/// The crypto provider every OurCamp TLS endpoint uses.
#[must_use]
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Accepts the server only if its certificate matches `expected` and it proves
/// possession of that certificate's key.
#[derive(Debug)]
pub struct PinnedVerifier {
    expected: Fingerprint,
    provider: Arc<CryptoProvider>,
}

impl PinnedVerifier {
    /// Pins `expected` (from the invite link).
    #[must_use]
    pub fn new(expected: Fingerprint) -> Self {
        Self {
            expected,
            provider: provider(),
        }
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        // A plain comparison is fine: the expected value is public (it is in the link),
        // so timing reveals nothing.
        if fingerprint(end_entity) == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Whom a client trusts to be one camp.
#[derive(Clone, Debug)]
pub enum Trust {
    /// Exactly the certificate with this fingerprint (a link with `fp`).
    Pin(Fingerprint),
    /// A certificate for the camp's name, issued by one of these authorities.
    Authorities(Arc<RootCertStore>),
}

impl Trust {
    /// What a link asks for: its pin, or else the authorities this computer trusts.
    #[must_use]
    pub fn of(pin: Option<Fingerprint>) -> Self {
        pin.map_or_else(|| Self::Authorities(system_roots()), Self::Pin)
    }

    /// The pin, if this is one.
    #[must_use]
    pub const fn pin(&self) -> Option<Fingerprint> {
        match self {
            Self::Pin(fp) => Some(*fp),
            Self::Authorities(_) => None,
        }
    }

    /// The name to connect to `host` with: checked against the certificate for an
    /// authority's, only sent as SNI for a pin.
    ///
    /// # Errors
    /// `host` is not a valid DNS name or IP address.
    pub fn server_name(&self, host: &str) -> Result<ServerName<'static>, Error> {
        let name = match self {
            Self::Pin(_) => PINNED_NAME,
            // An IPv6 host comes in brackets, as in URLs.
            Self::Authorities(_) => host.trim_start_matches('[').trim_end_matches(']'),
        };
        ServerName::try_from(name.to_owned())
            .map_err(|_| Error::General(format!("{host} is not a server name")))
    }

    /// The client configuration for this trust.
    ///
    /// # Panics
    /// Never in practice: the `ring` provider supports the default protocol versions.
    #[must_use]
    pub fn client_config(&self) -> Arc<ClientConfig> {
        match self {
            Self::Pin(fp) => client_config(*fp),
            Self::Authorities(roots) => {
                let verifier =
                    WebPkiServerVerifier::builder_with_provider(Arc::clone(roots), provider())
                        .build()
                        .expect("a root store with the ring provider");
                let config = ClientConfig::builder_with_provider(provider())
                    .with_safe_default_protocol_versions()
                    .expect("ring supports the default TLS versions")
                    .with_webpki_verifier(verifier)
                    .with_no_client_auth();
                Arc::new(config)
            }
        }
    }
}

/// The root certificates of this computer's trust store (Windows, macOS, or the usual
/// files on Linux), read once. Certificates it cannot parse are skipped.
#[must_use]
pub fn system_roots() -> Arc<RootCertStore> {
    static ROOTS: LazyLock<Arc<RootCertStore>> = LazyLock::new(|| {
        let mut roots = RootCertStore::empty();
        let found = rustls_native_certs::load_native_certs();
        let (added, _skipped) = roots.add_parsable_certificates(found.certs);
        if added == 0 {
            // Every domain camp will then fail its handshake; say why once.
            eprintln!("[oc-tls] no trusted root certificates found on this computer");
        }
        Arc::new(roots)
    });
    Arc::clone(&ROOTS)
}

/// Client configuration that trusts only the camp with this fingerprint.
///
/// # Panics
/// Never in practice: the `ring` provider supports the default protocol versions.
#[must_use]
pub fn client_config(expected: Fingerprint) -> Arc<ClientConfig> {
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .dangerous() // "dangerous" only in that it replaces CA validation — with a pin
        .with_custom_certificate_verifier(Arc::new(PinnedVerifier::new(expected)))
        .with_no_client_auth();
    Arc::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use rustls::server::{ClientHello, ResolvesServerCert};
    use rustls::sign::CertifiedKey;
    use rustls::{ClientConnection, ServerConfig, ServerConnection};

    struct Identity {
        cert: CertificateDer<'static>,
        key: PrivateKeyDer<'static>,
    }

    fn identity() -> Identity {
        let generated = rcgen::generate_simple_self_signed(vec!["ourcamp".into()]).unwrap();
        Identity {
            cert: generated.cert.der().clone(),
            key: PrivatePkcs8KeyDer::from(generated.signing_key.serialize_der()).into(),
        }
    }

    /// Serves a fixed certificate chain with an arbitrary key — including one that
    /// does not match the certificate, which normal config builders refuse.
    #[derive(Debug)]
    struct Fixed(Arc<CertifiedKey>);

    impl ResolvesServerCert for Fixed {
        fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
            Some(Arc::clone(&self.0))
        }
    }

    fn server(cert: &CertificateDer<'static>, key: &PrivateKeyDer<'static>) -> ServerConnection {
        server_chain(vec![cert.clone()], key)
    }

    fn server_chain(
        chain: Vec<CertificateDer<'static>>,
        key: &PrivateKeyDer<'static>,
    ) -> ServerConnection {
        let provider = provider();
        let signing = provider
            .key_provider
            .load_private_key(key.clone_key())
            .unwrap();
        let config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(Fixed(Arc::new(CertifiedKey::new(chain, signing)))));
        ServerConnection::new(Arc::new(config)).unwrap()
    }

    /// Runs a handshake in memory, shuttling bytes until both sides are done or one fails.
    fn handshake(expected: Fingerprint, server: ServerConnection) -> Result<(), Error> {
        handshake_as(&Trust::Pin(expected), "camp.test", server)
    }

    fn handshake_as(trust: &Trust, host: &str, mut server: ServerConnection) -> Result<(), Error> {
        let name = trust.server_name(host)?;
        let mut client = ClientConnection::new(trust.client_config(), name).unwrap();
        let mut buf = Vec::new();
        for _ in 0..20 {
            buf.clear();
            client.write_tls(&mut buf).unwrap();
            if !buf.is_empty() {
                server.read_tls(&mut buf.as_slice()).unwrap();
                server.process_new_packets()?;
            }
            buf.clear();
            server.write_tls(&mut buf).unwrap();
            if !buf.is_empty() {
                client.read_tls(&mut buf.as_slice()).unwrap();
                client.process_new_packets()?;
            }
            if !client.is_handshaking() && !server.is_handshaking() {
                return Ok(());
            }
        }
        panic!("handshake did not finish");
    }

    /// A certificate authority and a certificate it issued for `names`.
    struct Authority {
        roots: Arc<RootCertStore>,
        chain: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    }

    fn authority(names: &[&str]) -> Authority {
        let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = CertifiedIssuer::self_signed(ca, KeyPair::generate().unwrap()).unwrap();
        let leaf_key = KeyPair::generate().unwrap();
        let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        let leaf = CertificateParams::new(names)
            .unwrap()
            .signed_by(&leaf_key, &ca)
            .unwrap();
        let mut roots = RootCertStore::empty();
        roots.add(ca.der().clone()).unwrap();
        Authority {
            roots: Arc::new(roots),
            chain: vec![leaf.der().clone()],
            key: PrivatePkcs8KeyDer::from(leaf_key.serialize_der()).into(),
        }
    }

    #[test]
    fn a_certificate_from_an_authority_is_checked_by_name() {
        let ca = authority(&["camp.test"]);
        let trust = Trust::Authorities(Arc::clone(&ca.roots));
        assert_eq!(
            handshake_as(&trust, "camp.test", server_chain(ca.chain.clone(), &ca.key)),
            Ok(())
        );
        assert_eq!(
            handshake_as(&trust, "other.test", server_chain(ca.chain, &ca.key)),
            Err(Error::InvalidCertificate(
                CertificateError::NotValidForNameContext {
                    expected: ServerName::try_from("other.test").unwrap(),
                    presented: vec!["DnsName(\"camp.test\")".into()],
                }
            )),
            "another domain's certificate is not this camp's"
        );
    }

    #[test]
    fn without_a_pin_a_self_signed_certificate_is_not_enough() {
        let ca = authority(&["camp.test"]);
        let own = identity();
        let result = handshake_as(
            &Trust::Authorities(ca.roots),
            "camp.test",
            server(&own.cert, &own.key),
        );
        assert!(
            matches!(result, Err(Error::InvalidCertificate(_))),
            "{result:?}"
        );
    }

    #[test]
    fn a_link_without_a_pin_trusts_the_system() {
        assert!(matches!(Trust::of(None), Trust::Authorities(_)));
        let fp = fingerprint(&identity().cert);
        assert_eq!(Trust::of(Some(fp)).pin(), Some(fp));
        assert_eq!(
            Trust::Pin(fp).server_name("192.0.2.15").unwrap(),
            ServerName::try_from(PINNED_NAME).unwrap(),
            "a pinned camp is asked for its own certificate"
        );
        assert_eq!(
            Trust::of(None).server_name("[2001:db8::7]").unwrap(),
            ServerName::try_from("2001:db8::7").unwrap()
        );
    }

    #[test]
    fn fingerprint_is_sha256_of_der() {
        let id = identity();
        let fp = fingerprint(&id.cert);
        assert_eq!(fp.to_string().len(), 43, "unpadded base64url of 32 bytes");
        assert_eq!(fp, fingerprint(&id.cert.clone()));
        assert_ne!(fp, fingerprint(&identity().cert));
    }

    #[test]
    fn pinned_certificate_is_accepted() {
        let id = identity();
        assert_eq!(
            handshake(fingerprint(&id.cert), server(&id.cert, &id.key)),
            Ok(())
        );
    }

    #[test]
    fn any_other_certificate_is_rejected() {
        let pinned = identity();
        let impostor = identity();
        let result = handshake(
            fingerprint(&pinned.cert),
            server(&impostor.cert, &impostor.key),
        );
        assert_eq!(
            result,
            Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure
            ))
        );
    }

    #[test]
    fn replaying_the_pinned_certificate_without_its_key_is_rejected() {
        // The attacker copies the camp's public certificate but can only sign with
        // their own key: the fingerprint matches, the handshake signature must not.
        let camp = identity();
        let attacker = identity();
        let result = handshake(fingerprint(&camp.cert), server(&camp.cert, &attacker.key));
        assert_eq!(
            result,
            Err(Error::InvalidCertificate(CertificateError::BadSignature)),
            "the handshake signature must be checked against the pinned certificate"
        );
    }
}
