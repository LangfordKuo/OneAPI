use std::time::Duration;

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, HeaderValue, StatusCode},
    response::Response,
    Json,
};
use serde_json::{json, Value};

use crate::{backup, error::AppError, state::AppState};

use super::auth::AdminSession;

/// A personal database can grow large, but keep the upload bounded so a
/// mistaken request cannot exhaust memory.
pub const MAX_IMPORT_BYTES: usize = 256 * 1024 * 1024;

pub async fn export(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Response, AppError> {
    if state.config.database_url.is_some() {
        return Err(AppError::BadRequest(
            "database export requires the default SQLite file".to_string(),
        ));
    }

    let path = backup::snapshot(&state.db, &state.config.data_dir, "export", false)
        .await
        .map_err(AppError::Internal)?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(anyhow::Error::from)
        .map_err(AppError::Internal)?;
    let _ = tokio::fs::remove_file(&path).await;

    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("oneapi-backup.db");
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .map_err(|error| AppError::Internal(error.into()))?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/vnd.sqlite3")
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::from(bytes))
        .map_err(|error| AppError::Internal(error.into()))
}

pub async fn import(
    State(state): State<AppState>,
    _session: AdminSession,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    if state.config.database_url.is_some() {
        return Err(AppError::BadRequest(
            "database import requires the default SQLite file".to_string(),
        ));
    }
    if body.len() < 100 || !body.starts_with(b"SQLite format 3\0") {
        return Err(AppError::BadRequest(
            "uploaded file is not a SQLite database".to_string(),
        ));
    }

    let target = backup::import_path(&state.config.database_path);
    let mut staging = target.as_os_str().to_os_string();
    staging.push(".tmp");
    let staging = std::path::PathBuf::from(staging);

    tokio::fs::write(&staging, &body)
        .await
        .map_err(anyhow::Error::from)
        .map_err(AppError::Internal)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600));
    }

    if let Err(error) = backup::validate(&staging).await {
        let _ = tokio::fs::remove_file(&staging).await;
        return Err(AppError::BadRequest(format!(
            "uploaded database is not usable: {error}"
        )));
    }

    // Take a consistent copy of the live database before replacing it. The
    // startup swap also keeps its own copy, but this one includes the current
    // WAL and is created while the pool is still open.
    let pre_import = backup::snapshot(&state.db, &state.config.data_dir, "pre-import", true)
        .await
        .map_err(AppError::Internal)?;
    tracing::warn!(
        backup = %pre_import.display(),
        "database snapshot created before import"
    );

    tokio::fs::rename(&staging, &target)
        .await
        .map_err(anyhow::Error::from)
        .map_err(AppError::Internal)?;

    // Fold the WAL into the live file before the process replaces itself.
    let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&state.db)
        .await;

    // Let this response reach the browser before graceful shutdown starts.
    let updates = state.updates.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(executable) = std::env::current_exe() {
            if let Ok(mut slot) = updates.restart_exe.lock() {
                *slot = Some(executable);
            }
        }
        updates
            .restart_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        updates.shutdown.notify_one();
    });

    Ok(Json(json!({
        "ok": true,
        "message": "database import validated; OneAPI will restart with the restored data",
    })))
}
