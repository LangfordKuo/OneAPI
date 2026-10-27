use std::{env, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub addr: SocketAddr,
    pub data_dir: PathBuf,
    pub database_url: Option<String>,
    pub database_path: PathBuf,
    pub log_level: String,
    #[allow(dead_code)]
    pub cookie_secure: bool,
}

impl Config {
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

        Ok(Self {
            addr,
            data_dir,
            database_url,
            database_path,
            log_level,
            cookie_secure,
        })
    }

    pub fn data_dir_display(&self) -> String {
        self.data_dir.to_string_lossy().into_owned()
    }
}
