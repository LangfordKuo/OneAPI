use std::time::{Duration, Instant};

use axum::{extract::State, Json};
use serde_json::{json, Value};

use crate::{backup, error::AppError, state::AppState};

use super::auth::AdminSession;

/// Minimum spacing between manual snapshots. A `VACUUM INTO` copies the whole
/// database, so an admin session hammering the endpoint can fill the data
/// directory faster than pruning reclaims it.
const MIN_BACKUP_INTERVAL: Duration = Duration::from_secs(10);

/// Create a snapshot on the server.
///
/// The file stays in `<data_dir>/backups` and is never returned to the client.
/// Downloading or uploading the database over HTTP would turn an admin session
/// into a way to exfiltrate the whole database or to plant a crafted one, so
/// backups are deliberately server-side only.
pub async fn snapshot(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<Value>, AppError> {
    if state.config.database_url.is_some() {
        return Err(AppError::BadRequest(
            "backup requires the default SQLite file".to_string(),
        ));
    }

    // Only one snapshot at a time. Queuing would let requests pile up behind a
    // slow VACUUM and multiply the disk and CPU cost, so the second caller is
    // rejected instead of waiting for the lock.
    let _snapshot_guard = state
        .backup_lock
        .try_lock()
        .map_err(|_| AppError::BadRequest("a backup is already in progress".to_string()))?;

    // Rate limit attempts rather than successes so a failing snapshot (for
    // example a full disk) cannot be retried in a tight loop.
    {
        let mut last = state
            .last_backup_at
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(previous) = *last {
            let elapsed = previous.elapsed();
            if elapsed < MIN_BACKUP_INTERVAL {
                let retry_after = (MIN_BACKUP_INTERVAL - elapsed).as_secs().max(1);
                return Err(AppError::TooManyRequests(format!(
                    "backup requested too often; retry in {retry_after}s"
                )));
            }
        }
        *last = Some(Instant::now());
    }

    let path = backup::snapshot(&state.db, &state.config.data_dir, "manual", true)
        .await
        .map_err(AppError::Internal)?;
    let bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("manual.db")
        .to_string();

    tracing::info!(backup = %name, bytes, "manual database snapshot created");

    Ok(Json(json!({
        "ok": true,
        "file": name,
        "bytes": bytes,
        "message": "备份已写入服务器 data/backups 目录",
    })))
}

/// List the retained backups. Read-only metadata, no file contents.
pub async fn list(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<Value>, AppError> {
    Ok(Json(
        json!({ "items": backup::list(&state.config.data_dir) }),
    ))
}
