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
        api_key::{self, ApiKey, ApiKeyCreate, ApiKeyLimits, ApiKeyUpdate},
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
    pub daily_token_limit: Option<i64>,
    pub daily_token_usage: i64,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl KeyOutput {
    fn from_row(row: ApiKey) -> Self {
        // A malformed stored allowlist is shown as empty for display only; the
        // gateway auth path treats it as an error and rejects the key.
        let limits = match row.parse_limits() {
            Ok(limits) => limits,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    key_id = row.id,
                    "failed to parse API key restrictions; showing empty lists"
                );
                ApiKeyLimits {
                    channel_ids: Vec::new(),
                    models: Vec::new(),
                }
            }
        };
        let daily_token_usage = row.usage_today(models::now_ms() / 86_400_000);
        Self {
            id: row.id,
            name: row.name,
            key_prefix: row.key_prefix,
            enabled: row.enabled != 0,
            expires_at: row.expires_at,
            last_used_at: row.last_used_at,
            allowed_channel_ids: limits.channel_ids,
            allowed_models: limits.models,
            token_limit: row.token_limit,
            token_usage: row.token_usage,
            daily_token_limit: row.daily_token_limit,
            daily_token_usage,
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
    #[serde(default)]
    pub daily_token_limit: Option<i64>,
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
    #[serde(default, deserialize_with = "double_option")]
    pub daily_token_limit: Option<Option<i64>>,
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
    daily_token_limit: Option<i64>,
) -> Result<(Vec<i64>, Vec<String>, Option<i64>, Option<i64>), AppError> {
    let channel_ids = api_key::parse_channel_ids(&channel_ids).map_err(|invalid| {
        AppError::BadRequest(format!(
            "channel id {invalid} is invalid; use positive channel IDs or an empty list"
        ))
    })?;
    if !channel_ids.is_empty() {
        let existing = models::channel::existing_ids(&state.db, &channel_ids).await?;
        if let Some(missing) = channel_ids.iter().find(|id| !existing.contains(id)) {
            return Err(AppError::BadRequest(format!(
                "channel {missing} does not exist"
            )));
        }
    }

    let models = api_key::parse_model_names(models);
    let token_limit = normalize_limit(token_limit, "token_limit")?;
    let daily_token_limit = normalize_limit(daily_token_limit, "daily_token_limit")?;

    Ok((channel_ids, models, token_limit, daily_token_limit))
}

fn normalize_limit(value: Option<i64>, field: &str) -> Result<Option<i64>, AppError> {
    match value {
        Some(value) if value < 0 => {
            Err(AppError::BadRequest(format!("{field} cannot be negative")))
        }
        Some(0) | None => Ok(None),
        Some(value) => Ok(Some(value)),
    }
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

    let (channel_ids, model_names, token_limit, daily_token_limit) = validate_restrictions(
        &state,
        input.allowed_channel_ids,
        input.allowed_models,
        input.token_limit,
        input.daily_token_limit,
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
            daily_token_limit,
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

    let name = match input.name {
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(AppError::BadRequest("name cannot be empty".to_string()));
            }
            trimmed.to_string()
        }
        None => existing.name.clone(),
    };
    let enabled = input
        .enabled
        .map(|value| if value { 1 } else { 0 })
        .unwrap_or(existing.enabled);
    let expires_at = match input.expires_at {
        Some(value) => value,
        None => existing.expires_at,
    };

    // Falling back to the stored restrictions requires decoding them. A
    // malformed allowlist is an error here as well: rewriting it as an empty
    // (unrestricted) list would silently widen the key's scope.
    let existing_limits = if input.allowed_channel_ids.is_none() || input.allowed_models.is_none() {
        Some(existing.parse_limits().map_err(|error| {
            tracing::error!(
                error = %error,
                key_id = existing.id,
                "failed to parse existing API key restrictions"
            );
            AppError::Internal(anyhow::anyhow!(
                "failed to parse existing restrictions for API key {}: {error}",
                existing.id
            ))
        })?)
    } else {
        None
    };
    let channel_ids = match input.allowed_channel_ids {
        Some(value) => value,
        None => existing_limits
            .as_ref()
            .expect("existing limits parsed when a field is omitted")
            .channel_ids
            .clone(),
    };
    let models = match input.allowed_models {
        Some(value) => value,
        None => existing_limits
            .as_ref()
            .expect("existing limits parsed when a field is omitted")
            .models
            .clone(),
    };
    let token_limit = input.token_limit.unwrap_or(existing.token_limit);
    let daily_token_limit = input
        .daily_token_limit
        .unwrap_or(existing.daily_token_limit);
    let (channel_ids, models, token_limit, daily_token_limit) =
        validate_restrictions(&state, channel_ids, models, token_limit, daily_token_limit).await?;
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
            daily_token_limit,
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
