use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::{
    error::AppError,
    models::{self, api_key::ApiKey},
    security,
    state::AppState,
};

use super::auth::AdminSession;

#[derive(Debug, Serialize)]
pub struct KeyOutput {
    pub id: i64,
    pub name: String,
    pub key_prefix: String,
    pub enabled: bool,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl KeyOutput {
    fn from_row(row: ApiKey) -> Self {
        Self {
            id: row.id,
            name: row.name,
            key_prefix: row.key_prefix,
            enabled: row.enabled != 0,
            expires_at: row.expires_at,
            last_used_at: row.last_used_at,
            created_at: row.created_at,
            key: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct KeyCreateInput {
    pub name: String,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct KeyUpdateInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub expires_at: Option<Option<i64>>,
}

pub async fn list(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<Vec<KeyOutput>>, AppError> {
    let keys = models::api_key::list(&state.db).await?;
    Ok(Json(keys.into_iter().map(KeyOutput::from_row).collect()))
}

pub async fn create(
    State(state): State<AppState>,
    _session: AdminSession,
    Json(input): Json<KeyCreateInput>,
) -> Result<(StatusCode, Json<KeyOutput>), AppError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("key name is required".to_string()));
    }

    let key = security::generate_api_key();
    let key_hash = security::api_key_hash(&key);
    let key_prefix: String = key.chars().take(12).collect();
    let enabled = if input.enabled.unwrap_or(true) { 1 } else { 0 };

    let row = models::api_key::create(
        &state.db,
        name,
        &key_hash,
        &key_prefix,
        enabled,
        input.expires_at,
        models::now_ms(),
    )
    .await?;

    let mut output = KeyOutput::from_row(row);
    output.key = Some(key);
    Ok((StatusCode::CREATED, Json(output)))
}

pub async fn update(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<i64>,
    Json(input): Json<KeyUpdateInput>,
) -> Result<Json<KeyOutput>, AppError> {
    let existing = models::api_key::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("key not found".to_string()))?;

    let name = input
        .name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or(existing.name);
    let enabled = input
        .enabled
        .map(|value| if value { 1 } else { 0 })
        .unwrap_or(existing.enabled);
    let expires_at = match input.expires_at {
        Some(value) => value,
        None => existing.expires_at,
    };

    if !models::api_key::update(&state.db, id, &name, enabled, expires_at).await? {
        return Err(AppError::NotFound("key not found".to_string()));
    }

    let row = models::api_key::get(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("key not found".to_string()))?;
    Ok(Json(KeyOutput::from_row(row)))
}

pub async fn delete(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    if !models::api_key::delete(&state.db, id).await? {
        return Err(AppError::NotFound("key not found".to_string()));
    }
    Ok(StatusCode::NO_CONTENT)
}
