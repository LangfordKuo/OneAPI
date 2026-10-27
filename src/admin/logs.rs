use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::{
    error::AppError,
    models::{
        self,
        usage_log::{LogFilters, UsageLogRow},
    },
    state::AppState,
};

use super::{auth::AdminSession, query};

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub channel_id: Option<i64>,
    #[serde(default)]
    pub api_key_id: Option<i64>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct LogEntry {
    pub id: i64,
    pub request_id: String,
    pub source: String,
    pub api_key_id: Option<i64>,
    pub channel_id: Option<i64>,
    pub model: String,
    pub upstream_model: Option<String>,
    pub inbound_protocol: String,
    pub upstream_protocol: String,
    pub stream: bool,
    pub status_code: i64,
    pub attempts: i64,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub latency_ms: i64,
    pub first_token_ms: Option<i64>,
    pub created_at: i64,
    pub api_key_name: Option<String>,
    pub channel_name: Option<String>,
}

impl From<UsageLogRow> for LogEntry {
    fn from(row: UsageLogRow) -> Self {
        Self {
            id: row.id,
            request_id: row.request_id,
            source: row.source,
            api_key_id: row.api_key_id,
            channel_id: row.channel_id,
            model: row.model,
            upstream_model: row.upstream_model,
            inbound_protocol: row.inbound_protocol,
            upstream_protocol: row.upstream_protocol,
            stream: row.stream != 0,
            status_code: row.status_code,
            attempts: row.attempts,
            error_type: row.error_type,
            error_message: row.error_message,
            prompt_tokens: row.prompt_tokens,
            completion_tokens: row.completion_tokens,
            total_tokens: row.total_tokens,
            cached_tokens: row.cached_tokens,
            cache_creation_tokens: row.cache_creation_tokens,
            reasoning_tokens: row.reasoning_tokens,
            latency_ms: row.latency_ms,
            first_token_ms: row.first_token_ms,
            created_at: row.created_at,
            api_key_name: row.api_key_name,
            channel_name: row.channel_name,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LogListOutput {
    pub items: Vec<LogEntry>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[derive(Debug, Serialize)]
pub struct LogDetailOutput {
    #[serde(flatten)]
    pub entry: LogEntry,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    _session: AdminSession,
    Query(query): Query<LogQuery>,
) -> Result<Json<LogListOutput>, AppError> {
    let status = match query.status.as_deref() {
        None | Some("all") => None,
        Some(value @ ("success" | "error")) => Some(value.to_string()),
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "unsupported status `{other}`"
            )))
        }
    };
    let protocol = match query.protocol.as_deref() {
        None | Some("all") => None,
        Some(value @ ("chat" | "responses" | "anthropic")) => Some(value.to_string()),
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "unsupported protocol `{other}`"
            )))
        }
    };
    let source = match query.source.as_deref() {
        None | Some("all") => None,
        Some(value @ ("gateway" | "playground")) => Some(value.to_string()),
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "unsupported source `{other}`"
            )))
        }
    };

    let filters = LogFilters {
        since: query::since(query.range.as_deref())?,
        model: query.model,
        channel_id: query.channel_id,
        api_key_id: query.api_key_id,
        status,
        inbound_protocol: protocol,
        source,
        stream: query.stream,
        request_id: query.request_id,
    };
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0).max(0);

    let (rows, total) = models::usage_log::list(&state.db, &filters, limit, offset).await?;

    Ok(Json(LogListOutput {
        items: rows.into_iter().map(LogEntry::from).collect(),
        total,
        limit,
        offset,
    }))
}

pub async fn detail(
    State(state): State<AppState>,
    _session: AdminSession,
    Path(request_id): Path<String>,
) -> Result<Json<LogDetailOutput>, AppError> {
    let row = models::usage_log::find_by_request_id(&state.db, &request_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("log `{request_id}` not found")))?;
    let (request_body, response_body) = models::usage_log::detail_bodies(&state.db, &request_id)
        .await?
        .unwrap_or((None, None));

    Ok(Json(LogDetailOutput {
        entry: LogEntry::from(row),
        request_body,
        response_body,
    }))
}
