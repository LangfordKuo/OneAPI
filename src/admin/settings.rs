use std::collections::HashMap;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::{error::AppError, models::setting, state::AppState};

use super::auth::AdminSession;

#[derive(Debug, Serialize)]
pub struct SettingsOutput {
    pub site_name: String,
    pub default_max_tokens: i64,
    pub nonstream_timeout_seconds: i64,
    pub stream_idle_timeout_seconds: i64,
    pub usage_log_retention_days: i64,
    pub response_retention_days: i64,
    pub store_request_bodies: bool,
    pub update_source: String,
}

impl SettingsOutput {
    fn from_rows(rows: Vec<(String, String)>) -> Self {
        let map: HashMap<String, String> = rows.into_iter().collect();
        let integer = |key: &str, default: i64| {
            map.get(key)
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(default)
        };
        let boolean = |key: &str, default: bool| {
            map.get(key)
                .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
                .unwrap_or(default)
        };

        Self {
            site_name: map
                .get("site_name")
                .cloned()
                .unwrap_or_else(|| "OneAPI".to_string()),
            default_max_tokens: integer("default_max_tokens", 4096),
            nonstream_timeout_seconds: integer("nonstream_timeout_seconds", 600),
            stream_idle_timeout_seconds: integer("stream_idle_timeout_seconds", 120),
            usage_log_retention_days: integer("usage_log_retention_days", 30),
            response_retention_days: integer("response_retention_days", 7),
            store_request_bodies: boolean("store_request_bodies", false),
            update_source: map
                .get("update_source")
                .filter(|value| value.as_str() == "github" || value.as_str() == "ghproxy")
                .cloned()
                .unwrap_or_else(|| "github".to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SettingsInput {
    #[serde(default)]
    pub site_name: Option<String>,
    #[serde(default)]
    pub default_max_tokens: Option<i64>,
    #[serde(default)]
    pub nonstream_timeout_seconds: Option<i64>,
    #[serde(default)]
    pub stream_idle_timeout_seconds: Option<i64>,
    #[serde(default)]
    pub usage_log_retention_days: Option<i64>,
    #[serde(default)]
    pub response_retention_days: Option<i64>,
    #[serde(default)]
    pub store_request_bodies: Option<bool>,
    #[serde(default)]
    pub update_source: Option<String>,
}

pub async fn get(
    State(state): State<AppState>,
    _session: AdminSession,
) -> Result<Json<SettingsOutput>, AppError> {
    let rows = setting::all(&state.db).await?;
    Ok(Json(SettingsOutput::from_rows(rows)))
}

pub async fn update(
    State(state): State<AppState>,
    _session: AdminSession,
    Json(input): Json<SettingsInput>,
) -> Result<Json<SettingsOutput>, AppError> {
    if let Some(site_name) = input.site_name {
        let site_name = site_name.trim();
        if site_name.is_empty() {
            return Err(AppError::BadRequest(
                "site_name cannot be empty".to_string(),
            ));
        }
        setting::upsert(&state.db, "site_name", site_name).await?;
    }

    if let Some(value) = input.default_max_tokens {
        if value <= 0 {
            return Err(AppError::BadRequest(
                "default_max_tokens must be positive".to_string(),
            ));
        }
        setting::upsert(&state.db, "default_max_tokens", &value.to_string()).await?;
    }

    if let Some(value) = input.nonstream_timeout_seconds {
        if value <= 0 {
            return Err(AppError::BadRequest(
                "nonstream_timeout_seconds must be positive".to_string(),
            ));
        }
        setting::upsert(&state.db, "nonstream_timeout_seconds", &value.to_string()).await?;
    }

    if let Some(value) = input.stream_idle_timeout_seconds {
        if value <= 0 {
            return Err(AppError::BadRequest(
                "stream_idle_timeout_seconds must be positive".to_string(),
            ));
        }
        setting::upsert(&state.db, "stream_idle_timeout_seconds", &value.to_string()).await?;
    }

    if let Some(value) = input.usage_log_retention_days {
        if value < 0 {
            return Err(AppError::BadRequest(
                "usage_log_retention_days cannot be negative".to_string(),
            ));
        }
        setting::upsert(&state.db, "usage_log_retention_days", &value.to_string()).await?;
    }

    if let Some(value) = input.response_retention_days {
        if value < 0 {
            return Err(AppError::BadRequest(
                "response_retention_days cannot be negative".to_string(),
            ));
        }
        setting::upsert(&state.db, "response_retention_days", &value.to_string()).await?;
    }

    if let Some(value) = input.store_request_bodies {
        setting::upsert(
            &state.db,
            "store_request_bodies",
            if value { "true" } else { "false" },
        )
        .await?;
    }

    if let Some(value) = input.update_source {
        let value = value.trim();
        if !matches!(value, "github" | "ghproxy") {
            return Err(AppError::BadRequest(
                "update_source must be github or ghproxy".to_string(),
            ));
        }
        setting::upsert(&state.db, "update_source", value).await?;
    }

    let rows = setting::all(&state.db).await?;
    Ok(Json(SettingsOutput::from_rows(rows)))
}
