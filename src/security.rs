use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};

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
use hmac::{Hmac, Mac};
use rand::{distributions::Alphanumeric, Rng, RngCore};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const API_KEY_PREFIX: &str = "sk-";
const API_KEY_RANDOM_LEN: usize = 48;
const MASTER_KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
/// Domain separator for deriving the API key pepper from the master key.
const API_KEY_PEPPER_CONTEXT: &[u8] = b"oneapi-api-key-pepper";

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct Crypto {
    /// The master key is kept in a `Zeroizing` wrapper so it is wiped from
    /// memory when the last `Crypto` clone is dropped.
    key: Zeroizing<[u8; MASTER_KEY_LEN]>,
    /// Decrypting a key just to render a mask shows up when listing channels;
    /// the ciphertext is content-addressed, so entries never go stale.
    mask_cache: Arc<Mutex<HashMap<String, String>>>,
}

impl std::fmt::Debug for Crypto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Crypto(<redacted>)")
    }
}

impl Crypto {
    fn new(key: Zeroizing<[u8; MASTER_KEY_LEN]>) -> Self {
        Self {
            key,
            mask_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        if let Ok(secret) = std::env::var("ONEAPI_SECRET") {
            let secret = Zeroizing::new(secret);
            if !secret.trim().is_empty() {
                return Ok(Self::new(Zeroizing::new(sha256_bytes(secret.as_bytes()))));
            }
        }

        let path = data_dir.join("master.key");
        let key = match fs::read(&path) {
            Ok(bytes) if bytes.len() == MASTER_KEY_LEN => {
                let bytes = Zeroizing::new(bytes);
                let mut key = Zeroizing::new([0u8; MASTER_KEY_LEN]);
                key[..].copy_from_slice(bytes.as_slice());
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
                let mut key = Zeroizing::new([0u8; MASTER_KEY_LEN]);
                rand::thread_rng().fill_bytes(&mut key[..]);
                write_secret_file(&path, &key[..])?;
                tracing::info!(path = %path.display(), "generated master key for upstream secrets");
                key
            }
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", path.display()))
            }
        };

        Ok(Self::new(key))
    }

    pub fn encrypt(&self, plaintext: &str) -> Result<String> {
        let cipher =
            Aes256Gcm::new_from_slice(&self.key[..]).map_err(|_| anyhow!("invalid master key"))?;
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

    pub fn decrypt(&self, encoded: &str) -> Result<Zeroizing<String>> {
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
            Aes256Gcm::new_from_slice(&self.key[..]).map_err(|_| anyhow!("invalid master key"))?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
            .map_err(|_| anyhow!("failed to decrypt secret"))?;

        String::from_utf8(plaintext)
            .map(Zeroizing::new)
            .context("decrypted secret is not valid UTF-8")
    }

    pub fn mask(&self, encoded: &str) -> String {
        if let Some(cached) = self
            .mask_cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(encoded).cloned())
        {
            return cached;
        }

        let masked = match self.decrypt(encoded) {
            Ok(plaintext) => {
                let len = plaintext.chars().count();
                if len <= 4 {
                    "••••".to_string()
                } else {
                    // Take only the visible suffix so the full plaintext never
                    // gets copied into an unzeroized `Vec<char>`.
                    let suffix: String = plaintext.chars().skip(len - 4).collect();
                    format!("••••{suffix}")
                }
            }
            Err(_) => "••••".to_string(),
        };

        if let Ok(mut cache) = self.mask_cache.lock() {
            // Ciphertext is unique per key version, so a tiny cache is enough;
            // clear it wholesale instead of tracking LRU order.
            if cache.len() >= 256 {
                cache.clear();
            }
            cache.insert(encoded.to_string(), masked.clone());
        }
        masked
    }

    /// Pepper for API key hashes, derived from the master key with a fixed
    /// domain separator. It is never stored, so a database copy alone cannot
    /// reproduce (or brute-force) the stored hashes.
    fn api_key_pepper(&self) -> [u8; 32] {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.key[..])
            .expect("HMAC-SHA256 accepts keys of any length");
        mac.update(API_KEY_PEPPER_CONTEXT);
        let mut pepper = [0u8; 32];
        pepper.copy_from_slice(&mac.finalize().into_bytes());
        pepper
    }

    /// Peppered API key hash: `HMAC-SHA256(pepper, key)`, hex encoded.
    pub fn api_key_hash_v2(&self, key: &str) -> String {
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.api_key_pepper())
            .expect("HMAC-SHA256 accepts keys of any length");
        mac.update(key.as_bytes());
        hex(&mac.finalize().into_bytes())
    }

    /// Legacy unpeppered SHA-256 hash. Existing rows keep authenticating with
    /// it (and an older binary can still read them after a rollback); new
    /// writes use [`Self::api_key_hash_v2`].
    pub fn api_key_hash_legacy(key: &str) -> String {
        sha256_hex(key)
    }
}

fn write_secret_file(path: &Path, key: &[u8]) -> Result<()> {
    write_private_file(path, key)?;
    Ok(())
}

/// Minimum length accepted for every password in this service.
pub const MIN_PASSWORD_CHARS: usize = 8;

/// Return a policy violation description, or `None` when the password is
/// acceptable. Kept in one place so the setup wizard, password change, env
/// bootstrap and reset-password command cannot drift apart.
pub fn password_violation(password: &str) -> Option<String> {
    (password.chars().count() < MIN_PASSWORD_CHARS)
        .then(|| format!("must be at least {MIN_PASSWORD_CHARS} characters"))
}

/// Create a file with owner-only permissions and write `contents` into it.
/// Fails if the file already exists, so callers never overwrite an existing
/// secret (master key, generated `.env`).
pub fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?
    };

    #[cfg(not(unix))]
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;

    file.write_all(contents)
}

/// Restrict an existing file to owner-only permissions (no-op off Unix).
pub fn restrict_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
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

/// Caps concurrent Argon2 work. Hashing is CPU- and memory-bound, so a burst
/// of login attempts must not occupy every Tokio worker: the blocking pool
/// does the work, and this semaphore bounds how much of that pool is consumed.
static PASSWORD_SEMAPHORE: OnceLock<tokio::sync::Semaphore> = OnceLock::new();

fn password_semaphore() -> &'static tokio::sync::Semaphore {
    PASSWORD_SEMAPHORE.get_or_init(|| tokio::sync::Semaphore::new(4))
}

/// Verify a password on the blocking pool so the Tokio worker running the
/// request stays available. The hash and password are moved into the closure.
pub async fn verify_password_blocking(
    hash: String,
    password: Zeroizing<String>,
) -> anyhow::Result<bool> {
    let _permit = password_semaphore()
        .acquire()
        .await
        .map_err(|_| anyhow!("password verification semaphore is closed"))?;
    tokio::task::spawn_blocking(move || verify_password(&hash, password.as_str()))
        .await
        .map_err(|error| anyhow!("password verification task failed: {error}"))
}

/// Hash a password on the blocking pool, bounded by the same semaphore.
pub async fn hash_password_blocking(password: Zeroizing<String>) -> anyhow::Result<String> {
    let _permit = password_semaphore()
        .acquire()
        .await
        .map_err(|_| anyhow!("password hashing semaphore is closed"))?;
    tokio::task::spawn_blocking(move || hash_password(password.as_str()))
        .await
        .map_err(|error| anyhow!("password hashing task failed: {error}"))?
}

/// Valid Argon2id PHC string using the crate's default parameters
/// (`m=19456,t=2,p=1`). It is only a constant-cost fallback for
/// [`dummy_password_hash`]; no real password ever verifies against it.
const FALLBACK_DUMMY_PASSWORD_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// A hash with default Argon2 parameters, generated once from random input.
/// Verifying against it keeps the login path's cost constant when the username
/// does not exist, so response timing does not reveal account existence.
pub fn dummy_password_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        let mut random = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut random);
        let password = Zeroizing::new(BASE64.encode(random));
        match hash_password(password.as_str()) {
            Ok(hash) => hash,
            Err(error) => {
                tracing::error!(
                    error = %error,
                    "failed to precompute dummy password hash; using built-in fallback"
                );
                FALLBACK_DUMMY_PASSWORD_HASH.to_string()
            }
        }
    })
    .as_str()
}

pub fn generate_api_key() -> String {
    let random: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(API_KEY_RANDOM_LEN)
        .map(char::from)
        .collect();
    format!("{API_KEY_PREFIX}{random}")
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
