//! The camp's TLS identity: a self-signed certificate created on first start.
//!
//! Its fingerprint goes into every invite link, so it must never change by accident:
//! files are written atomically (temp file + rename) and reused on every start.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oc_invite::Fingerprint;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;

const CERT_FILE: &str = "cert.der";
const KEY_FILE: &str = "key.der";
/// Subject name. Clients pin the fingerprint and ignore names (ADR-0014).
const SUBJECT: &str = "ourcamp";

/// Certificate, key and the fingerprint links carry.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
    /// SHA-256 of `cert`.
    pub fingerprint: Fingerprint,
}

impl Identity {
    /// Loads the identity from `dir`, creating it on first start.
    ///
    /// # Errors
    /// If the files cannot be read or written, or a previous identity is half there
    /// (refusing beats silently changing the fingerprint of every link).
    pub fn load_or_create(dir: &Path) -> io::Result<Self> {
        let cert_path = dir.join(CERT_FILE);
        let key_path = dir.join(KEY_FILE);
        match (cert_path.exists(), key_path.exists()) {
            (true, true) => {
                let cert = CertificateDer::from(fs::read(&cert_path)?);
                let key = PrivatePkcs8KeyDer::from(fs::read(&key_path)?);
                Ok(Self::new(cert, key))
            }
            (false, false) => {
                fs::create_dir_all(dir)?;
                let generated = rcgen::generate_simple_self_signed(vec![SUBJECT.into()])
                    .map_err(io::Error::other)?;
                let cert = generated.cert.der().clone();
                let key = PrivatePkcs8KeyDer::from(generated.signing_key.serialize_der());
                // Key first: a crash between the two leaves "key only", which the
                // half-present check above reports instead of regenerating.
                write_atomically(&key_path, key.secret_pkcs8_der())?;
                write_atomically(&cert_path, cert.as_ref())?;
                Ok(Self::new(cert, key))
            }
            _ => Err(io::Error::other(format!(
                "{} has only one of {CERT_FILE}/{KEY_FILE}; restore the missing file \
                 (deleting both creates a new identity and invalidates every link)",
                dir.display()
            ))),
        }
    }

    fn new(cert: CertificateDer<'static>, key: PrivatePkcs8KeyDer<'static>) -> Self {
        let fingerprint = oc_tls::fingerprint(&cert);
        Self {
            cert,
            key,
            fingerprint,
        }
    }

    /// TLS server configuration for this identity.
    ///
    /// # Errors
    /// If the key does not match the certificate (a corrupted identity directory).
    pub fn server_config(&self) -> Result<Arc<ServerConfig>, rustls::Error> {
        let mut config = ServerConfig::builder_with_provider(oc_tls::provider())
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(
                vec![self.cert.clone()],
                PrivateKeyDer::Pkcs8(self.key.clone_key()),
            )?;
        // HTTP/2 for API calls, HTTP/1.1 for the WebSocket upgrades (signalling proxy).
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp: PathBuf = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    restrict_to_owner(&tmp)?;
    fs::rename(&tmp, path)
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
#[allow(
    clippy::unnecessary_wraps,
    reason = "same signature as the unix version"
)]
fn restrict_to_owner(_path: &Path) -> io::Result<()> {
    // Windows: files under the user's profile are private to the user by default ACLs.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oc-tls-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn identity_is_created_once_and_reused() {
        let dir = temp_dir("reuse");
        let first = Identity::load_or_create(&dir).unwrap();
        let second = Identity::load_or_create(&dir).unwrap();
        assert_eq!(
            first.fingerprint, second.fingerprint,
            "links survive restarts"
        );
        assert!(second.server_config().is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn half_present_identity_is_refused_not_regenerated() {
        let dir = temp_dir("half");
        Identity::load_or_create(&dir).unwrap();
        fs::remove_file(dir.join(CERT_FILE)).unwrap();
        let err = Identity::load_or_create(&dir).err().expect("must refuse");
        assert!(err.to_string().contains("only one of"), "{err}");
        assert!(!dir.join(CERT_FILE).exists(), "nothing was regenerated");
        let _ = fs::remove_dir_all(&dir);
    }
}
