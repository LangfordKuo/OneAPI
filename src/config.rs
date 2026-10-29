use std::{
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub addr: SocketAddr,
    pub data_dir: PathBuf,
    pub database_url: Option<String>,
    pub database_path: PathBuf,
    pub log_level: String,
    pub cookie_secure: bool,
    pub trust_proxy: bool,
    pub database_pool_size: u32,
}

impl Config {
    /// Read configuration from the process environment and `.env` files.
    /// Callers must have run [`load_env_before_startup`] first; this function
    /// never mutates the environment itself.
    pub fn from_env(data_dir: Option<PathBuf>) -> Result<Self> {
        let data_dir = data_dir
            .or_else(|| env::var_os("ONEAPI_DATA_DIR").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("./data"));

        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("failed to create data directory {}", data_dir.display()))?;

        let addr: SocketAddr = env::var("ONEAPI_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:3000".to_string())
            .parse()
            .context("ONEAPI_ADDR is not a valid socket address")?;

        let database_url = env::var("ONEAPI_DATABASE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let database_path = data_dir.join("oneapi.db");

        let log_level = env::var("ONEAPI_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let cookie_secure = env::var("ONEAPI_COOKIE_SECURE")
            .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        let trust_proxy = env::var("ONEAPI_TRUST_PROXY")
            .map(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        let database_pool_size = env::var("ONEAPI_DB_POOL_SIZE")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(5)
            .clamp(1, 64);

        Ok(Self {
            addr,
            data_dir,
            database_url,
            database_path,
            log_level,
            cookie_secure,
            trust_proxy,
            database_pool_size,
        })
    }

    pub fn data_dir_display(&self) -> String {
        self.data_dir.to_string_lossy().into_owned()
    }
}

/// Load the dotenv files and resolve the data directory before the async
/// runtime starts.
///
/// `env::set_var` mutates process-global state and is not thread-safe, so all
/// environment writes must happen while the process is still single-threaded.
/// The setup wizard writes `.env` into the data directory; loading it here
/// keeps the config travel with the SQLite database and master.key.
pub fn load_env_before_startup(data_dir: Option<PathBuf>) -> Result<PathBuf> {
    // A .env next to the binary/working directory overrides defaults.
    load_env_file(Path::new(".env"));

    let data_dir = data_dir
        .or_else(|| env::var_os("ONEAPI_DATA_DIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("./data"));

    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("failed to create data directory {}", data_dir.display()))?;
    load_env_file(&data_dir.join(".env"));

    Ok(data_dir)
}

/// Minimal dotenv loader. Processes only `KEY=value` lines, ignores comments,
/// and never overrides variables already present in the process environment.
fn load_env_file(path: &Path) {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return;
    };

    for line in contents.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || env::var_os(key).is_some() {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        env::set_var(key, value);
    }
}
