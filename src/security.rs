use std::{fs, io::Write, path::Path, sync::OnceLock};

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Context, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rand::{distributions::Alphanumeric, Rng, RngCore};
use sha2::{Digest, Sha256};

const API_KEY_PREFIX: &str = "sk-";
const API_KEY_RANDOM_LEN: usize = 48;
const MASTER_KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

#[derive(Clone)]
pub struct Crypto {
    key: [u8; MASTER_KEY_LEN],
}

impl std::fmt::Debug for Crypto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Crypto(<redacted>)")
    }
}

impl Crypto {
    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        if let Ok(secret) = std::env::var("ONEAPI_SECRET") {
            if !secret.trim().is_empty() {
                return Ok(Self {
                    key: sha256_bytes(secret.as_bytes()),
                });
            }
        }

        let path = data_dir.join("master.key");
        let key = match fs::read(&path) {
            Ok(bytes) if bytes.len() == MASTER_KEY_LEN => {
                let mut key = [0u8; MASTER_KEY_LEN];
                key.copy_from_slice(&bytes);
                key
            }
            Ok(_) => {
                return Err(anyhow!(
                    "{} must contain exactly {} bytes",
                    path.display(),
                    MASTER_KEY_LEN
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut key = [0u8; MASTER_KEY_LEN];
                rand::thread_rng().fill_bytes(&mut key);
                write_secret_file(&path, &key)?;
                tracing::info!(path = %path.display(), "generated master key for upstream secrets");
                key
            }
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", path.display()))
            }
        };

        Ok(Self { key })
    }

    pub fn encrypt(&self, plaintext: &str) -> Result<String> {
        let cipher =
            Aes256Gcm::new_from_slice(&self.key).map_err(|_| anyhow!("invalid master key"))?;
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|_| anyhow!("failed to encrypt secret"))?;

        Ok(format!(
            "v1:{}:{}",
            BASE64.encode(nonce_bytes),
            BASE64.encode(ciphertext)
        ))
    }

    pub fn decrypt(&self, encoded: &str) -> Result<String> {
        let mut parts = encoded.splitn(3, ':');
        let version = parts.next().unwrap_or_default();
        let nonce_b64 = parts
            .next()
            .ok_or_else(|| anyhow!("invalid encrypted secret format"))?;
        let ciphertext_b64 = parts
            .next()
            .ok_or_else(|| anyhow!("invalid encrypted secret format"))?;

        if version != "v1" {
            return Err(anyhow!("unsupported encrypted secret version"));
        }

        let nonce_bytes = BASE64
            .decode(nonce_b64)
            .context("invalid encrypted secret nonce")?;
        let ciphertext = BASE64
            .decode(ciphertext_b64)
            .context("invalid encrypted secret payload")?;

        if nonce_bytes.len() != NONCE_LEN {
            return Err(anyhow!("invalid encrypted secret nonce length"));
        }

        let cipher =
            Aes256Gcm::new_from_slice(&self.key).map_err(|_| anyhow!("invalid master key"))?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
            .map_err(|_| anyhow!("failed to decrypt secret"))?;

        String::from_utf8(plaintext).context("decrypted secret is not valid UTF-8")
    }

    pub fn mask(&self, encoded: &str) -> String {
        match self.decrypt(encoded) {
            Ok(plaintext) => {
                let chars: Vec<char> = plaintext.chars().collect();
                if chars.len() <= 4 {
                    return "••••".to_string();
                }
                let suffix: String = chars[chars.len() - 4..].iter().collect();
                format!("••••{suffix}")
            }
            Err(_) => "••••".to_string(),
        }
    }
}

fn write_secret_file(path: &Path, key: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(key)?;
    }

    #[cfg(not(unix))]
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(key)?;
    }

    Ok(())
}

pub fn hash_password(password: &str) -> Result<String> {
    let mut salt_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|error| anyhow!("failed to encode password salt: {error}"))?;

    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| anyhow!("failed to hash password: {error}"))
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// A hash with default Argon2 parameters, generated once from random input.
/// Verifying against it keeps the login path's cost constant when the username
/// does not exist, so response timing does not reveal account existence.
pub fn dummy_password_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        let mut random = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut random);
        let password = BASE64.encode(random);
        hash_password(&password).expect("default Argon2 parameters are valid")
    })
}

pub fn generate_api_key() -> String {
    let random: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(API_KEY_RANDOM_LEN)
        .map(char::from)
        .collect();
    format!("{API_KEY_PREFIX}{random}")
}

pub fn api_key_hash(key: &str) -> String {
    sha256_hex(key)
}

pub fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex(&bytes)
}

pub fn sha256_hex(value: &str) -> String {
    hex(&sha256_bytes(value.as_bytes()))
}

fn sha256_bytes(value: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(value);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_round_trip() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password(&hash, "correct horse battery staple"));
        assert!(!verify_password(&hash, "wrong"));
    }

    #[test]
    fn api_keys_are_unique_and_hashed() {
        let first = generate_api_key();
        let second = generate_api_key();
        assert!(first.starts_with("sk-"));
        assert_ne!(first, second);
        assert_eq!(api_key_hash(&first).len(), 64);
        assert_ne!(api_key_hash(&first), api_key_hash(&second));
    }

    #[test]
    fn secret_encryption_round_trip() {
        let key = Crypto {
            key: [7u8; MASTER_KEY_LEN],
        };
        let encrypted = key.encrypt("sk-upstream-secret").unwrap();
        assert!(encrypted.starts_with("v1:"));
        assert_eq!(key.decrypt(&encrypted).unwrap(), "sk-upstream-secret");
        assert!(key.mask(&encrypted).ends_with("cret"));
    }

    #[test]
    fn short_secrets_are_fully_masked() {
        let key = Crypto {
            key: [7u8; MASTER_KEY_LEN],
        };
        for secret in ["a", "abc", "abcd"] {
            let encrypted = key.encrypt(secret).unwrap();
            assert_eq!(key.mask(&encrypted), "••••");
        }
    }

    #[test]
    fn dummy_hash_is_well_formed_and_rejects_passwords() {
        assert!(PasswordHash::new(dummy_password_hash()).is_ok());
        assert!(!verify_password(dummy_password_hash(), "anything"));
    }
}
