use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    error::AppError,
    models::{
        self,
        playground_session::{validate_input, SessionInput},
    },
    state::AppState,
};

use super::auth::AdminSession;

/// Sessions use client-generated UUIDs so the UI can persist the first message
/// without an extra create round-trip. Canonicalize before touching the table so
/// the same UUID cannot appear under two spellings.
fn canonical_id(id: &str) -> Result<String, AppError> {
    Uuid::parse_str(id)
        .map(|value| value.to_string())
        .map_err(|_| AppError::BadRequest("invalid playground session id".to_string()))
}

pub async fn list(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<Value>, AppError> {
    let items = models::playground_session::list(&state.db).await?;
    Ok(Json(json!({ "items": items })))
}

pub async fn get(
    Path(id): Path<String>,
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<Value>, AppError> {
    let id = canonical_id(&id)?;
    let session = models::playground_session::get(&state.db, &id)
        .await?
        .ok_or_else(|| AppError::NotFound("playground session not found".to_string()))?;
    Ok(Json(json!({ "session": session })))
}

pub async fn save(
    Path(id): Path<String>,
    State(state): State<AppState>,
    _session: AdminSession,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let id = canonical_id(&id)?;
    let input: SessionInput = serde_json::from_value(body)
        .map_err(|error| AppError::BadRequest(format!("invalid playground session: {error}")))?;
    validate_input(&input).map_err(AppError::BadRequest)?;
    models::playground_session::save(&state.db, &id, &input, models::now_ms()).await?;

    let session = models::playground_session::get(&state.db, &id)
        .await?
        .ok_or_else(|| {
            AppError::Internal(anyhow::anyhow!(
                "playground session disappeared after saving"
            ))
        })?;
    Ok(Json(json!({ "session": session })))
}

pub async fn delete(
    Path(id): Path<String>,
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<Value>, AppError> {
    let id = canonical_id(&id)?;
    if !models::playground_session::delete(&state.db, &id).await? {
        return Err(AppError::NotFound(
            "playground session not found".to_string(),
        ));
    }
    Ok(Json(json!({ "ok": true })))
}
