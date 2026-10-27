//! The device key (ADR-0015): generated once, kept in one 32-byte file (the Ed25519
//! seed), never sent anywhere — only signatures and the public key leave the device.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use oc_proto::PUBLIC_KEY_LEN;
use ring::signature::{Ed25519KeyPair, KeyPair};

const SEED_LEN: usize = 32;

/// This device's identity in every camp.
pub struct DeviceKey {
    pair: Ed25519KeyPair,
}

// Never print key material.
impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceKey({})", self.user_id())
    }
}

impl DeviceKey {
    /// A fresh random key.
    ///
    /// # Panics
    /// If the operating system's random number generator fails.
    #[must_use]
    pub fn generate() -> Self {
        Self::from_seed(&random_seed().expect("operating system random number generator"))
    }

    fn from_seed(seed: &[u8; SEED_LEN]) -> Self {
        // Any 32 bytes are a valid Ed25519 seed; "unchecked" refers to not having a
        // public key to compare against, which we derive instead.
        let pair = Ed25519KeyPair::from_seed_unchecked(seed).expect("32-byte seed");
        Self { pair }
    }

    /// Loads the key from `path`, creating it there on first use.
    ///
    /// # Errors
    /// If the file cannot be read or written, or is not a key (refusing beats silently
    /// becoming a different person in every camp).
    pub fn load_or_create(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let seed: [u8; SEED_LEN] = bytes.try_into().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{} is not a device key", path.display()),
                    )
                })?;
                Ok(Self::from_seed(&seed))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let seed = random_seed()?;
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir)?;
                }
                // Atomic: a crash mid-write must not leave a truncated key behind.
                let tmp = path.with_extension("tmp");
                fs::write(&tmp, seed)?;
                restrict_to_owner(&tmp)?;
                fs::rename(&tmp, path)?;
                Ok(Self::from_seed(&seed))
            }
            Err(e) => Err(e),
        }
    }

    /// Ed25519 public key.
    ///
    /// # Panics
    /// Never: Ed25519 public keys are always 32 bytes.
    #[must_use]
    pub fn public_key(&self) -> [u8; PUBLIC_KEY_LEN] {
        self.pair
            .public_key()
            .as_ref()
            .try_into()
            .expect("Ed25519 public keys are 32 bytes")
    }

    /// This device's user id (ADR-0018).
    #[must_use]
    pub fn user_id(&self) -> String {
        oc_proto::user_id(&self.public_key())
    }

    pub(crate) fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.pair.sign(message).as_ref().to_vec()
    }
}

fn random_seed() -> io::Result<[u8; SEED_LEN]> {
    let mut seed = [0u8; SEED_LEN];
    getrandom::fill(&mut seed).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(seed)
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
    // Windows: the app data directory is private to the user by default ACLs.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oc-key-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("device.key")
    }

    #[test]
    fn key_is_created_once_and_reloaded_identically() {
        let path = temp_path("reuse");
        let a = DeviceKey::load_or_create(&path).unwrap();
        let b = DeviceKey::load_or_create(&path).unwrap();
        assert_eq!(a.public_key(), b.public_key());
        assert_eq!(fs::read(&path).unwrap().len(), SEED_LEN);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn signatures_verify_with_the_shared_protocol_code() {
        let key = DeviceKey::generate();
        let msg = b"hello";
        assert!(oc_proto::verify(&key.public_key(), msg, &key.sign(msg)));
        assert_ne!(key.user_id(), DeviceKey::generate().user_id());
    }

    #[test]
    fn a_damaged_key_file_is_refused() {
        let path = temp_path("damaged");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"short").unwrap();
        let err = DeviceKey::load_or_create(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&path).unwrap(), b"short", "never overwritten");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
