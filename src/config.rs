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
    #[allow(dead_code)]
    pub cookie_secure: bool,
}

impl Config {
    pub fn from_env(data_dir: Option<PathBuf>) -> Result<Self> {
        // A .env next to the binary/working directory overrides defaults.
        load_env_file(Path::new(".env"));

        let data_dir = data_dir
            .or_else(|| env::var_os("ONEAPI_DATA_DIR").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("./data"));

        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("failed to create data directory {}", data_dir.display()))?;

        // The setup wizard writes .env into the data directory so the config
        // travels with the SQLite database and master.key.
        load_env_file(&data_dir.join(".env"));

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotenv_loader_reads_simple_values_without_overriding() {
        let key = "ONEAPI_TEST_DOTENV_VALUE";
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(".env");
        std::fs::write(
            &path,
            "ONEAPI_TEST_DOTENV_VALUE=from-file\n# comment\nexport ONEAPI_TEST_DOTENV_OTHER='quoted'\n",
        )
        .expect("write env");

        std::env::remove_var(key);
        std::env::remove_var("ONEAPI_TEST_DOTENV_OTHER");
        load_env_file(&path);
        assert_eq!(std::env::var(key).expect("value"), "from-file");
        assert_eq!(
            std::env::var("ONEAPI_TEST_DOTENV_OTHER").expect("quoted"),
            "quoted"
        );

        std::env::set_var(key, "from-process");
        load_env_file(&path);
        assert_eq!(std::env::var(key).expect("value"), "from-process");

        std::env::remove_var(key);
        std::env::remove_var("ONEAPI_TEST_DOTENV_OTHER");
    }
}
