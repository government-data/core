//! Encryption of the system settings' secrets (the LDAP service account's
//! password, OAuth and Graph client secrets, the SMTP password) before
//! storing them in Postgres.
//!
//! AES-256-GCM with a random nonce per secret. The result is
//! `v1:<base64(nonce ‖ ciphertext)>`. The key does not live in the database:
//! it comes from `SETTINGS_ENCRYPTION_KEY` (32 bytes in base64) or from a
//! file generated the first time, so a database backup does not expose the
//! secrets. Losing the key means typing those secrets again in the UI.

use std::path::Path;

use aes_gcm::aead::{Aead, Generate, Key, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

const PREFIX: &str = "v1:";
const NONCE_LEN: usize = 12;

/// Where the key came from, to show it in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// `SETTINGS_ENCRYPTION_KEY`.
    Environment,
    /// A file that already existed.
    File,
    /// A file just generated (if it is not on a volume, it is lost when the
    /// container is recreated).
    Generated,
}

/// The cipher.
#[derive(Clone)]
pub struct SecretBox {
    cipher: Aes256Gcm,
    source: KeySource,
}

impl std::fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretBox")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl SecretBox {
    /// The key of `SETTINGS_ENCRYPTION_KEY` if set; otherwise the one in
    /// `key_file`, creating it (readable only by its owner) if it does not
    /// exist.
    pub fn from_env_or_file(key_file: impl AsRef<Path>) -> Result<Self, String> {
        if let Some(raw) = std::env::var("SETTINGS_ENCRYPTION_KEY")
            .ok()
            .filter(|v| !v.trim().is_empty())
        {
            return Self::from_base64(raw.trim(), KeySource::Environment)
                .map_err(|e| format!("SETTINGS_ENCRYPTION_KEY: {e}"));
        }
        let path = key_file.as_ref();
        match std::fs::read_to_string(path) {
            Ok(raw) => Self::from_base64(raw.trim(), KeySource::File)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                let key = Key::<Aes256Gcm>::generate();
                write_private(path, STANDARD.encode(key.as_slice()).as_bytes())
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                tracing::warn!(
                    path = %path.display(),
                    "settings encryption key generated; back it up"
                );
                Ok(Self {
                    cipher: Aes256Gcm::new(&key),
                    source: KeySource::Generated,
                })
            }
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// With a 32-byte key already at hand (derived from another, for
    /// example).
    pub fn from_key(key: &[u8]) -> Result<Self, String> {
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|_| format!("la llave debe tener 32 bytes, tiene {}", key.len()))?;
        Ok(Self {
            cipher,
            source: KeySource::File,
        })
    }

    fn from_base64(raw: &str, source: KeySource) -> Result<Self, String> {
        let bytes = STANDARD
            .decode(raw)
            .map_err(|_| "no es base64 válido".to_string())?;
        let cipher = Aes256Gcm::new_from_slice(&bytes)
            .map_err(|_| format!("la llave debe tener 32 bytes, tiene {}", bytes.len()))?;
        Ok(Self { cipher, source })
    }

    /// Where the key came from.
    pub fn source(&self) -> KeySource {
        self.source
    }

    pub fn encrypt(&self, plain: &str) -> Result<String, String> {
        let nonce = Nonce::generate();
        let sealed = self
            .cipher
            .encrypt(&nonce, plain.as_bytes())
            .map_err(|_| "no se pudo cifrar".to_string())?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&sealed);
        Ok(format!("{PREFIX}{}", STANDARD.encode(out)))
    }

    pub fn decrypt(&self, sealed: &str) -> Result<String, String> {
        let raw = sealed
            .strip_prefix(PREFIX)
            .ok_or_else(|| "formato de secreto desconocido".to_string())?;
        let bytes = STANDARD
            .decode(raw)
            .map_err(|_| "secreto corrupto".to_string())?;
        if bytes.len() <= NONCE_LEN {
            return Err("secreto corrupto".into());
        }
        let (nonce, body) = bytes.split_at(NONCE_LEN);
        let nonce = Nonce::try_from(nonce).map_err(|_| "secreto corrupto".to_string())?;
        let plain = self
            .cipher
            .decrypt(&nonce, body)
            .map_err(|_| "no se descifra con la llave actual".to_string())?;
        String::from_utf8(plain).map_err(|_| "secreto corrupto".to_string())
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("secret-box-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trips_and_uses_a_fresh_nonce() {
        let dir = temp_dir();
        let key = dir.join("k/settings.key");
        let sb = SecretBox::from_env_or_file(&key).unwrap();
        assert_eq!(sb.source(), KeySource::Generated);
        let a = sb.encrypt("p4ss").unwrap();
        let b = sb.encrypt("p4ss").unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with("v1:"));
        assert_eq!(sb.decrypt(&a).unwrap(), "p4ss");

        // The same file gives the same key again.
        let again = SecretBox::from_env_or_file(&key).unwrap();
        assert_eq!(again.source(), KeySource::File);
        assert_eq!(again.decrypt(&a).unwrap(), "p4ss");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn another_key_or_a_tampered_value_does_not_decrypt() {
        let one = SecretBox::from_base64(&STANDARD.encode([1u8; 32]), KeySource::File).unwrap();
        let two = SecretBox::from_base64(&STANDARD.encode([2u8; 32]), KeySource::File).unwrap();
        let sealed = one.encrypt("x").unwrap();
        assert!(two.decrypt(&sealed).is_err());
        let mut tampered = sealed.clone();
        tampered.pop();
        tampered.push(if sealed.ends_with('A') { 'B' } else { 'A' });
        assert!(one.decrypt(&tampered).is_err());
        assert!(SecretBox::from_base64(&STANDARD.encode([1u8; 16]), KeySource::File).is_err());
    }
}
