//! A certificate from an authority for the camp's domain (ADR-0042): the files certbot
//! (or a hosting panel) keeps up to date, read at start and again whenever they change.
//!
//! Links to the domain carry no fingerprint; clients check this certificate by the
//! domain name. The camp's own self-signed identity stays: links with `fp` keep working,
//! and the [`Resolver`] tells the two apart by the name the client asks for (SNI).

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, SystemTime};

use rustls::client::danger::ServerCertVerifier;
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::RootCertStore;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// How often the files are looked at. Certbot renews a month before expiry, so this is
/// about picking up a hand-made replacement quickly; two `stat` calls cost nothing.
const RECHECK: Duration = Duration::from_secs(60);

/// Where the certificate is: the full chain (`fullchain.pem`) and its key (`privkey.pem`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateFiles {
    /// PEM: the certificate, then the intermediates.
    pub chain: PathBuf,
    /// PEM: the private key (PKCS#8, PKCS#1 or SEC1).
    pub key: PathBuf,
}

/// Why the certificate cannot be served.
#[derive(Debug)]
pub enum CertificateError {
    /// A file is missing, unreadable or not PEM.
    Read(PathBuf, String),
    /// Only one certificate: a self-signed one, or `cert.pem` instead of `fullchain.pem`.
    NoChain,
    /// The key is not the certificate's.
    KeyMismatch(String),
    /// Not for this domain, expired, or not issued by the next certificate in the chain.
    Invalid(String),
}

impl fmt::Display for CertificateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(path, why) => write!(f, "{}: {why}", path.display()),
            Self::NoChain => f.write_str(
                "the chain has a single certificate; give the full chain (fullchain.pem: \
                 the certificate and its intermediates), from an authority",
            ),
            Self::KeyMismatch(why) => write!(f, "the key does not match the certificate: {why}"),
            Self::Invalid(why) => write!(f, "the certificate is not valid: {why}"),
        }
    }
}

impl std::error::Error for CertificateError {}

/// Reads and checks the certificate for `domain`.
///
/// The authority's root is not on hand (the server may have no trust store at all), so
/// the chain is checked up to its last certificate: the name, the dates, and that each
/// certificate is signed by the next. That catches every mistake an owner can make with
/// the files; whether the authority is trusted, clients decide.
///
/// # Errors
/// See [`CertificateError`].
pub fn load(files: &CertificateFiles, domain: &str) -> Result<Arc<CertifiedKey>, CertificateError> {
    let read =
        |path: &Path, e: &dyn fmt::Display| CertificateError::Read(path.into(), e.to_string());
    let chain = CertificateDer::pem_file_iter(&files.chain)
        .and_then(Iterator::collect::<Result<Vec<_>, _>>)
        .map_err(|e| read(&files.chain, &e))?;
    if chain.is_empty() {
        return Err(read(&files.chain, &"no certificate in the file"));
    }
    let key = PrivateKeyDer::from_pem_file(&files.key).map_err(|e| read(&files.key, &e))?;
    check_chain(&chain, domain)?;
    let certified = CertifiedKey::from_der(chain, key, &oc_tls::provider())
        .map_err(|e| CertificateError::KeyMismatch(e.to_string()))?;
    Ok(Arc::new(certified))
}

fn check_chain(chain: &[CertificateDer<'static>], domain: &str) -> Result<(), CertificateError> {
    let invalid = |e: &dyn fmt::Display| CertificateError::Invalid(e.to_string());
    let [leaf, intermediates @ .., last] = chain else {
        return Err(CertificateError::NoChain);
    };
    let mut anchor = RootCertStore::empty();
    anchor.add(last.clone()).map_err(|e| invalid(&e))?;
    let verifier =
        WebPkiServerVerifier::builder_with_provider(Arc::new(anchor), oc_tls::provider())
            .build()
            .map_err(|e| invalid(&e))?;
    let name = ServerName::try_from(domain.to_owned()).map_err(|e| invalid(&e))?;
    verifier
        .verify_server_cert(leaf, intermediates, &name, &[], UnixTime::now())
        .map(drop)
        .map_err(|e| invalid(&e))
}

/// The certificate being served for the domain, replaced when the files change.
pub(crate) struct DomainCertificate {
    domain: String,
    files: CertificateFiles,
    current: RwLock<Arc<CertifiedKey>>,
}

impl DomainCertificate {
    /// Loads it for the first time.
    ///
    /// # Errors
    /// See [`load`]: the camp does not start with a certificate clients would refuse.
    pub(crate) fn open(files: CertificateFiles, domain: &str) -> Result<Self, CertificateError> {
        let domain = domain.to_ascii_lowercase();
        let current = load(&files, &domain)?;
        Ok(Self {
            domain,
            files,
            current: RwLock::new(current),
        })
    }

    fn current(&self) -> Arc<CertifiedKey> {
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// When the files were last changed; `None` while one is missing (mid-renewal).
    fn stamp(&self) -> Option<(SystemTime, SystemTime)> {
        let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        Some((modified(&self.files.chain)?, modified(&self.files.key)?))
    }

    /// Reads the files again. A broken renewal keeps the old certificate in service:
    /// it is still valid for weeks, and the owner sees why in the log.
    fn reload(&self) {
        match load(&self.files, &self.domain) {
            Ok(fresh) => {
                *self.current.write().unwrap_or_else(PoisonError::into_inner) = fresh;
                eprintln!("[oc-server] certificate for {} reloaded", self.domain);
            }
            Err(e) => eprintln!(
                "[oc-server] new certificate for {} not taken, the old one stays: {e}",
                self.domain
            ),
        }
    }
}

/// Looks at the files every [`RECHECK`] and reloads them after a change, until shutdown.
pub(crate) fn spawn_reload(
    certificate: Arc<DomainCertificate>,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut seen = certificate.stamp();
        loop {
            tokio::select! {
                () = tokio::time::sleep(RECHECK) => {}
                _ = shutdown.changed() => return,
            }
            let now = certificate.stamp();
            // Both files present and changed since: certbot writes them one by one, and
            // a minute later both are there.
            if now.is_some() && now != seen {
                seen = now;
                let certificate = Arc::clone(&certificate);
                let _ = tokio::task::spawn_blocking(move || certificate.reload()).await;
            }
        }
    })
}

/// Picks the certificate by the name the client asks for: the domain gets the
/// authority's certificate, everything else (pinned clients ask for
/// [`oc_tls::PINNED_NAME`]) the camp's own.
#[derive(Debug)]
pub(crate) struct Resolver {
    own: Arc<CertifiedKey>,
    domain: Option<Arc<DomainCertificate>>,
}

impl Resolver {
    pub(crate) fn new(own: Arc<CertifiedKey>, domain: Option<Arc<DomainCertificate>>) -> Self {
        Self { own, domain }
    }
}

impl fmt::Debug for DomainCertificate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DomainCertificate")
            .field("domain", &self.domain)
            .finish_non_exhaustive()
    }
}

impl ResolvesServerCert for Resolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let asked = hello.server_name();
        match &self.domain {
            // SNI arrives lower-cased by rustls.
            Some(domain) if asked == Some(domain.domain.as_str()) => Some(domain.current()),
            _ => Some(Arc::clone(&self.own)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};

    struct Issued {
        dir: PathBuf,
        files: CertificateFiles,
    }

    /// An authority, an intermediate and a certificate for `names`, written as certbot
    /// writes them. `with_intermediate: false` writes only the certificate.
    fn issue(name: &str, names: &[&str], with_intermediate: bool) -> Issued {
        let dir = std::env::temp_dir().join(format!("oc-cert-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ca_params = || {
            let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
            p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
            p
        };
        let root = CertifiedIssuer::self_signed(ca_params(), KeyPair::generate().unwrap()).unwrap();
        let intermediate =
            CertifiedIssuer::signed_by(ca_params(), KeyPair::generate().unwrap(), &root).unwrap();
        let key = KeyPair::generate().unwrap();
        let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        let leaf = CertificateParams::new(names)
            .unwrap()
            .signed_by(&key, &intermediate)
            .unwrap();
        let mut chain = leaf.pem();
        if with_intermediate {
            chain.push_str(&intermediate.pem());
        }
        let files = CertificateFiles {
            chain: dir.join("fullchain.pem"),
            key: dir.join("privkey.pem"),
        };
        std::fs::write(&files.chain, chain).unwrap();
        std::fs::write(&files.key, key.serialize_pem()).unwrap();
        Issued { dir, files }
    }

    #[test]
    fn a_full_chain_for_the_domain_is_taken() {
        let issued = issue("good", &["ourcamp.kz"], true);
        let certified = load(&issued.files, "ourcamp.kz").unwrap();
        assert_eq!(
            certified.cert.len(),
            2,
            "the certificate and its intermediate"
        );
        let _ = std::fs::remove_dir_all(issued.dir);
    }

    #[test]
    fn mistakes_with_the_files_are_named() {
        let issued = issue("bad", &["ourcamp.kz"], true);
        assert!(matches!(
            load(&issued.files, "other.kz"),
            Err(CertificateError::Invalid(_))
        ));

        let alone = issue("alone", &["ourcamp.kz"], false);
        assert!(matches!(
            load(&alone.files, "ourcamp.kz"),
            Err(CertificateError::NoChain)
        ));

        let swapped = CertificateFiles {
            chain: issued.files.chain.clone(),
            key: alone.files.key.clone(),
        };
        assert!(matches!(
            load(&swapped, "ourcamp.kz"),
            Err(CertificateError::KeyMismatch(_))
        ));

        let missing = CertificateFiles {
            chain: issued.dir.join("nope.pem"),
            key: issued.files.key.clone(),
        };
        assert!(matches!(
            load(&missing, "ourcamp.kz"),
            Err(CertificateError::Read(..))
        ));
        let _ = std::fs::remove_dir_all(issued.dir);
        let _ = std::fs::remove_dir_all(alone.dir);
    }

    #[test]
    fn a_broken_renewal_keeps_the_old_certificate() {
        let issued = issue("renew", &["ourcamp.kz"], true);
        let domain = DomainCertificate::open(issued.files.clone(), "OurCamp.kz").unwrap();
        let before = domain.current();

        std::fs::write(&issued.files.chain, "not a certificate").unwrap();
        domain.reload();
        assert!(Arc::ptr_eq(&before, &domain.current()), "still served");

        let fresh = issue("renew-new", &["ourcamp.kz"], true);
        std::fs::copy(&fresh.files.chain, &issued.files.chain).unwrap();
        std::fs::copy(&fresh.files.key, &issued.files.key).unwrap();
        domain.reload();
        assert_ne!(before.cert[0], domain.current().cert[0], "the renewed one");
        let _ = std::fs::remove_dir_all(issued.dir);
        let _ = std::fs::remove_dir_all(fresh.dir);
    }
}
