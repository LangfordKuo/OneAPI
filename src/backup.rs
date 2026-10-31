use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use sqlx::SqlitePool;

pub const BACKUP_DIR: &str = "backups";
/// Keep the newest backups so a long-lived personal deployment cannot fill
/// the data directory.
pub const KEEP_BACKUPS: usize = 5;

/// Prefixes used by [`snapshot`]. Only files carrying one of these prefixes are
/// listed or pruned, so a database file an operator stores in the backup
/// directory is never mistaken for a snapshot and deleted.
const BACKUP_PREFIXES: [&str; 2] = ["oneapi", "manual"];

pub fn backup_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(BACKUP_DIR)
}

/// Whether `path` is a regular snapshot file created by [`snapshot`].
///
/// Symbolic links and other non-regular files are rejected without following
/// them, and foreign `.db` files are ignored, so listing and pruning cannot
/// escape the backup directory or remove data this module did not write.
fn is_managed_backup(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_file() {
        return false;
    }
    if !path.extension().is_some_and(|extension| extension == "db") {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    BACKUP_PREFIXES.iter().any(|prefix| {
        name.strip_prefix(*prefix)
            .is_some_and(|rest| rest.starts_with('-'))
    })
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
        fs::symlink_metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
    })
}

/// A retained backup file on the server.
#[derive(Debug, serde::Serialize)]
pub struct BackupFile {
    pub name: String,
    pub bytes: u64,
    /// Modification time in Unix milliseconds.
    pub modified_at: i64,
}

/// Retained backups, newest first. Unreadable entries are skipped rather than
/// failing the whole listing.
pub fn list(data_dir: &Path) -> Vec<BackupFile> {
    let Ok(entries) = fs::read_dir(backup_dir(data_dir)) else {
        return Vec::new();
    };

    let mut files: Vec<BackupFile> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !is_managed_backup(&path) {
                return None;
            }
            let metadata = fs::symlink_metadata(&path).ok()?;
            Some(BackupFile {
                name: entry.file_name().to_string_lossy().into_owned(),
                bytes: metadata.len(),
                modified_at: metadata
                    .modified()
                    .ok()?
                    .duration_since(UNIX_EPOCH)
                    .ok()?
                    .as_millis() as i64,
            })
        })
        .collect();
    files.sort_by_key(|file| std::cmp::Reverse(file.modified_at));
    files
}

fn newest_backup(data_dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<(SystemTime, PathBuf)> = fs::read_dir(backup_dir(data_dir))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_managed_backup(path))
        .filter_map(|path| {
            let modified = fs::symlink_metadata(&path).ok()?.modified().ok()?;
            Some((modified, path))
        })
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    entries.into_iter().next().map(|(_, path)| path)
}

/// Take a consistent snapshot with `VACUUM INTO`. The file name is prefixed so
/// automatic, manual and startup backups can be told apart.
///
/// Snapshots are written to `<data_dir>/backups` and never leave the server;
/// there is no download endpoint, so a compromised admin session cannot pull the
/// database out through the API.
///
/// `prune` controls whether the retained backup set is trimmed afterwards.
pub async fn snapshot(
    pool: &SqlitePool,
    data_dir: &Path,
    prefix: &str,
    prune: bool,
) -> anyhow::Result<PathBuf> {
    let dir = backup_dir(data_dir);
    ensure_backup_dir(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = unique_path(&dir, prefix);
    // Build the statement with an escaped literal so the file is created on
    // every SQLite build; the path is our own, not user input.
    let escaped = path.to_string_lossy().replace('\'', "''");
    sqlx::raw_sql(&format!("VACUUM INTO '{escaped}'"))
        .execute(pool)
        .await
        .with_context(|| format!("failed to write database snapshot {}", path.display()))?;
    // A snapshot is a full copy of the database; keep it unreadable to other
    // local users instead of relying on the process umask. A file that cannot
    // be made owner-only is removed: leaving it behind would expose the whole
    // database with whatever permissions VACUUM happened to use.
    if let Err(error) = crate::security::restrict_permissions(&path) {
        if let Err(remove_error) = fs::remove_file(&path) {
            tracing::warn!(
                file = %path.display(),
                error = %remove_error,
                "failed to remove backup after permission failure"
            );
        }
        return Err(anyhow::Error::from(error).context(format!(
            "failed to restrict permissions on backup {}",
            path.display()
        )));
    }
    if prune {
        prune_backups(&dir, KEEP_BACKUPS);
    }
    Ok(path)
}

/// Create the backup directory owner-only. The mode is applied at creation so
/// the directory is never briefly world-accessible, and an existing directory
/// is tightened explicitly in case its permissions drifted.
fn ensure_backup_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn wal_path(database_path: &Path) -> PathBuf {
    let mut value = database_path.as_os_str().to_os_string();
    value.push("-wal");
    PathBuf::from(value)
}

fn unique_path(dir: &Path, prefix: &str) -> PathBuf {
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let mut path = dir.join(format!("{prefix}-{stamp}.db"));
    let mut suffix = 1;
    // `Path::exists` follows symlinks, so a dangling link would look free and
    // let VACUUM create its target elsewhere. Treat any directory entry as a
    // collision instead.
    while fs::symlink_metadata(&path).is_ok() {
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
        .map(|entry| entry.path())
        .filter(|path| is_managed_backup(path))
        .filter_map(|path| {
            let modified = fs::symlink_metadata(&path).ok()?.modified().ok()?;
            Some((modified, path))
        })
        .collect();
    files.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, path) in files.into_iter().skip(keep) {
        // Re-check just before removing so a path swapped for a symlink or a
        // foreign file since the directory scan is left untouched.
        if !is_managed_backup(&path) {
            continue;
        }
        if let Err(error) = fs::remove_file(&path) {
            tracing::warn!(file = %path.display(), error = %error, "failed to prune old backup");
        }
    }
}
