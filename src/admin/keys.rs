use std::collections::HashSet;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    error::AppError,
    models::{
        self,
        api_key::{self, ApiKey, ApiKeyCreate, ApiKeyUpdate},
    },
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
    pub allowed_channel_ids: Vec<i64>,
    pub allowed_models: Vec<String>,
    pub token_limit: Option<i64>,
    pub token_usage: i64,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl KeyOutput {
    fn from_row(row: ApiKey) -> Self {
        let allowed_channel_ids = row.channel_ids();
        let allowed_models = row.model_names();
        Self {
            id: row.id,
            name: row.name,
            key_prefix: row.key_prefix,
            enabled: row.enabled != 0,
            expires_at: row.expires_at,
            last_used_at: row.last_used_at,
            allowed_channel_ids,
            allowed_models,
            token_limit: row.token_limit,
            token_usage: row.token_usage,
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
    #[serde(default)]
    pub allowed_channel_ids: Vec<i64>,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default)]
    pub token_limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct KeyUpdateInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    pub expires_at: Option<Option<i64>>,
    #[serde(default)]
    pub allowed_channel_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub allowed_models: Option<Vec<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub token_limit: Option<Option<i64>>,
}

/// Distinguish "field omitted" (None) from an explicit `null` (Some(None)).
fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Ok(Some(Option::<T>::deserialize(deserializer)?))
}

/// Validate and normalize optional restrictions. Empty lists mean "no limit".
async fn validate_restrictions(
    state: &AppState,
    channel_ids: Vec<i64>,
    models: Vec<String>,
    token_limit: Option<i64>,
) -> Result<(Vec<i64>, Vec<String>, Option<i64>), AppError> {
    let channel_ids = api_key::parse_channel_ids(&channel_ids);
    if !channel_ids.is_empty() {
        let existing: HashSet<i64> = models::channel::list(&state.db)
            .await?
            .into_iter()
            .map(|channel| channel.id)
            .collect();
        if let Some(missing) = channel_ids.iter().find(|id| !existing.contains(id)) {
            return Err(AppError::BadRequest(format!(
                "channel {missing} does not exist"
            )));
        }
    }

    let models = api_key::parse_model_names(models);
    let token_limit = match token_limit {
        Some(value) if value < 0 => {
            return Err(AppError::BadRequest(
                "token_limit cannot be negative".to_string(),
            ))
        }
        Some(0) | None => None,
        Some(value) => Some(value),
    };

    Ok((channel_ids, models, token_limit))
}

fn encode<T: Serialize>(value: &T) -> Result<String, AppError> {
    serde_json::to_string(value).map_err(|error| AppError::Internal(error.into()))
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

    let (channel_ids, model_names, token_limit) = validate_restrictions(
        &state,
        input.allowed_channel_ids,
        input.allowed_models,
        input.token_limit,
    )
    .await?;

    let key = security::generate_api_key();
    let key_hash = security::api_key_hash(&key);
    let key_prefix: String = key.chars().take(12).collect();
    let enabled = if input.enabled.unwrap_or(true) { 1 } else { 0 };
    let allowed_channel_ids = encode(&channel_ids)?;
    let allowed_models = encode(&model_names)?;

    let row = models::api_key::create_with_limits(
        &state.db,
        &ApiKeyCreate {
            name,
            key_hash: &key_hash,
            key_prefix: &key_prefix,
            enabled,
            expires_at: input.expires_at,
            allowed_channel_ids: &allowed_channel_ids,
            allowed_models: &allowed_models,
            token_limit,
            created_at: models::now_ms(),
        },
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
        .unwrap_or_else(|| existing.name.clone());
    let enabled = input
        .enabled
        .map(|value| if value { 1 } else { 0 })
        .unwrap_or(existing.enabled);
    let expires_at = match input.expires_at {
        Some(value) => value,
        None => existing.expires_at,
    };

    let channel_ids = input
        .allowed_channel_ids
        .unwrap_or_else(|| existing.channel_ids());
    let models = input
        .allowed_models
        .unwrap_or_else(|| existing.model_names());
    let token_limit = input.token_limit.unwrap_or(existing.token_limit);
    let (channel_ids, models, token_limit) =
        validate_restrictions(&state, channel_ids, models, token_limit).await?;
    let allowed_channel_ids = encode(&channel_ids)?;
    let allowed_models = encode(&models)?;

    if !models::api_key::update(
        &state.db,
        id,
        &ApiKeyUpdate {
            name: &name,
            enabled,
            expires_at,
            allowed_channel_ids: &allowed_channel_ids,
            allowed_models: &allowed_models,
            token_limit,
        },
    )
    .await?
    {
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
