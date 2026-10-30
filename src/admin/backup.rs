use axum::{extract::State, Json};
use serde_json::{json, Value};

use crate::{backup, error::AppError, state::AppState};

use super::auth::AdminSession;

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
