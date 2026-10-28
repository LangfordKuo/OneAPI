use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};

use crate::config::Config;

pub const BACKUP_DIR: &str = "backups";
/// Keep the newest backups so a long-lived personal deployment cannot fill
/// the data directory.
pub const KEEP_BACKUPS: usize = 5;

pub fn backup_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(BACKUP_DIR)
}

pub fn import_path(database_path: &Path) -> PathBuf {
    let mut value = database_path.as_os_str().to_os_string();
    value.push(".import");
    PathBuf::from(value)
}

/// (main database bytes, WAL bytes).
pub fn database_sizes(database_path: &Path) -> (u64, u64) {
    let main = fs::metadata(database_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let wal = fs::metadata(wal_path(database_path))
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    (main, wal)
}

/// Modification time of the newest backup, in Unix milliseconds.
pub fn last_backup_at(data_dir: &Path) -> Option<i64> {
    newest_backup(data_dir).and_then(|path| {
        fs::metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
    })
}

fn newest_backup(data_dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<(SystemTime, PathBuf)> = fs::read_dir(backup_dir(data_dir))
        .ok()?
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "db")
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    entries.into_iter().next().map(|(_, path)| path)
}

/// Take a consistent snapshot with `VACUUM INTO`. The file name is prefixed
/// so automatic and pre-import backups can be told apart from exports.
pub async fn snapshot(pool: &SqlitePool, data_dir: &Path, prefix: &str) -> anyhow::Result<PathBuf> {
    let dir = backup_dir(data_dir);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = unique_path(&dir, prefix);
    // Build the statement with an escaped literal so the file is created on
    // every SQLite build; the path is our own, not user input.
    let escaped = path.to_string_lossy().replace('\'', "''");
    sqlx::raw_sql(&format!("VACUUM INTO '{escaped}'"))
        .execute(pool)
        .await
        .with_context(|| format!("failed to write database snapshot {}", path.display()))?;
    prune(&dir, KEEP_BACKUPS);
    Ok(path)
}

/// Validate a candidate database before swapping it into place.
pub async fn validate(path: &Path) -> anyhow::Result<()> {
    if !path.is_file() {
        bail!("database file does not exist");
    }
    if fs::metadata(path)?.len() == 0 {
        bail!("database file is empty");
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .context("failed to open the database file")?;

    let rows: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_all(&pool)
        .await
        .context("failed to run quick_check")?;
    if !rows.iter().all(|row| row == "ok") {
        bail!("database integrity check failed");
    }

    let admin_tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'admin'",
    )
    .fetch_one(&pool)
    .await
    .context("failed to inspect the database schema")?;
    if admin_tables == 0 {
        bail!("file is not a OneAPI database");
    }
    Ok(())
}

/// Swap `<database>.import` into place before the pool is opened. Returns the
/// path that was restored, or `None` when there is nothing to do.
pub async fn prepare_import(config: &Config) -> anyhow::Result<Option<PathBuf>> {
    let import = import_path(&config.database_path);
    if !import.exists() {
        return Ok(None);
    }
    if config.database_url.is_some() {
        let _ = fs::remove_file(&import);
        tracing::warn!("imported database ignored because ONEAPI_DATABASE_URL is set");
        return Ok(None);
    }

    if let Err(error) = validate(&import).await {
        let dir = backup_dir(&config.data_dir);
        fs::create_dir_all(&dir).ok();
        let target = unique_path(&dir, "invalid-import");
        let _ = fs::rename(&import, &target);
        tracing::error!(
            error = %error,
            file = %target.display(),
            "imported database is invalid; keeping the current database"
        );
        return Ok(None);
    }

    let dir = backup_dir(&config.data_dir);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    if config.database_path.exists() {
        let target = unique_path(&dir, "pre-import");
        fs::rename(&config.database_path, &target).with_context(|| {
            format!(
                "failed to move the current database to {}",
                target.display()
            )
        })?;
    }
    remove_sidecars(&config.database_path);
    fs::rename(&import, &config.database_path)
        .with_context(|| format!("failed to install {}", config.database_path.display()))?;
    prune(&dir, KEEP_BACKUPS);
    tracing::warn!(
        database = %config.database_path.display(),
        "restored database from an imported file"
    );
    Ok(Some(config.database_path.clone()))
}

fn wal_path(database_path: &Path) -> PathBuf {
    let mut value = database_path.as_os_str().to_os_string();
    value.push("-wal");
    PathBuf::from(value)
}

fn sidecar_paths(database_path: &Path) -> [PathBuf; 2] {
    let mut wal = database_path.as_os_str().to_os_string();
    wal.push("-wal");
    let mut shm = database_path.as_os_str().to_os_string();
    shm.push("-shm");
    [PathBuf::from(wal), PathBuf::from(shm)]
}

fn remove_sidecars(database_path: &Path) {
    for path in sidecar_paths(database_path) {
        let _ = fs::remove_file(path);
    }
}

fn unique_path(dir: &Path, prefix: &str) -> PathBuf {
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let mut path = dir.join(format!("{prefix}-{stamp}.db"));
    let mut suffix = 1;
    while path.exists() {
        path = dir.join(format!("{prefix}-{stamp}-{suffix}.db"));
        suffix += 1;
    }
    path
}

fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "db")
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    files.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, path) in files.into_iter().skip(keep) {
        if let Err(error) = fs::remove_file(&path) {
            tracing::warn!(file = %path.display(), error = %error, "failed to prune old backup");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn snapshot_is_a_valid_database_and_prunes_old_files() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let source = temp.path().join("source.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&source)
                    .create_if_missing(true),
            )
            .await
            .expect("source database");
        sqlx::query("CREATE TABLE admin (id INTEGER PRIMARY KEY, name TEXT)")
            .execute(&pool)
            .await
            .expect("schema");
        sqlx::query("INSERT INTO admin (id, name) VALUES (1, 'admin')")
            .execute(&pool)
            .await
            .expect("row");

        for _ in 0..(KEEP_BACKUPS + 2) {
            let path = snapshot(&pool, temp.path(), "oneapi")
                .await
                .expect("snapshot");
            assert!(path.is_file());
            validate(&path).await.expect("valid snapshot");
            // Make each file's mtime distinct so pruning has an order.
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let count = fs::read_dir(backup_dir(temp.path()))
            .expect("backups")
            .flatten()
            .count();
        assert_eq!(count, KEEP_BACKUPS);
        assert!(last_backup_at(temp.path()).is_some());
    }

    #[tokio::test]
    async fn invalid_import_is_set_aside_and_current_database_is_kept() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let data_dir = temp.path();
        let database_path = data_dir.join("oneapi.db");
        fs::write(&database_path, b"not a database").expect("current db");
        let import = import_path(&database_path);
        fs::write(&import, b"also not a database").expect("import db");

        let config = Config {
            addr: "127.0.0.1:0".parse().expect("addr"),
            data_dir: data_dir.to_path_buf(),
            database_url: None,
            database_path: database_path.clone(),
            log_level: "error".to_string(),
            cookie_secure: false,
            trust_proxy: false,
            database_pool_size: 1,
        };

        assert!(prepare_import(&config).await.expect("prepare").is_none());
        assert_eq!(
            fs::read(&database_path).expect("current db"),
            b"not a database"
        );
        assert!(!import.exists());
        assert!(backup_dir(data_dir).is_dir());
    }

    #[tokio::test]
    async fn prepared_import_replaces_the_database() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let data_dir = temp.path();
        let source = data_dir.join("source.db");
        let source_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&source)
                    .create_if_missing(true),
            )
            .await
            .expect("source");
        sqlx::query("CREATE TABLE admin (id INTEGER PRIMARY KEY, name TEXT)")
            .execute(&source_pool)
            .await
            .expect("schema");
        sqlx::query("INSERT INTO admin (id, name) VALUES (1, 'restored')")
            .execute(&source_pool)
            .await
            .expect("row");
        source_pool.close().await;

        let database_path = data_dir.join("oneapi.db");
        fs::copy(&source, import_path(&database_path)).expect("import copy");

        let config = Config {
            addr: "127.0.0.1:0".parse().expect("addr"),
            data_dir: data_dir.to_path_buf(),
            database_url: None,
            database_path: database_path.clone(),
            log_level: "error".to_string(),
            cookie_secure: false,
            trust_proxy: false,
            database_pool_size: 1,
        };

        let restored = prepare_import(&config).await.expect("prepare");
        assert_eq!(restored, Some(database_path.clone()));
        assert!(database_path.is_file());
        assert!(!import_path(&database_path).exists());
    }
}
