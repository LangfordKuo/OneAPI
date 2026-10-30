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
///
/// `prune` controls whether the retained backup set is trimmed afterwards.
/// Exports are downloaded and then deleted, so they must not prune: the freshly
/// written export file would otherwise occupy one of the retained slots and
/// evict a real backup every time an operator exports the database.
pub async fn snapshot(
    pool: &SqlitePool,
    data_dir: &Path,
    prefix: &str,
    prune: bool,
) -> anyhow::Result<PathBuf> {
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
    if prune {
        prune_backups(&dir, KEEP_BACKUPS);
    }
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
    prune_backups(&dir, KEEP_BACKUPS);
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

fn prune_backups(dir: &Path, keep: usize) {
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
