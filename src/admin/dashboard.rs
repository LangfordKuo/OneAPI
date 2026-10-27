use std::collections::HashMap;

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::{error::AppError, models::now_ms, state::AppState};

use super::{auth::AdminSession, query};

#[derive(Debug, Deserialize)]
pub struct RangeQuery {
    #[serde(default)]
    pub range: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SummaryOutput {
    pub range: String,
    pub requests: i64,
    pub successes: i64,
    pub errors: i64,
    pub success_rate: f64,
    pub total_tokens: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cached_tokens: i64,
    pub reasoning_tokens: i64,
    pub avg_latency_ms: f64,
    pub avg_first_token_ms: Option<f64>,
    pub active_channels: i64,
    pub active_keys: i64,
}

#[derive(Debug, FromRow)]
struct SummaryRow {
    requests: i64,
    successes: i64,
    errors: i64,
    total_tokens: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    cached_tokens: i64,
    reasoning_tokens: i64,
    avg_latency_ms: f64,
    avg_first_token_ms: Option<f64>,
    active_channels: i64,
    active_keys: i64,
}

pub async fn summary(
    State(state): State<AppState>,
    _session: AdminSession,
    Query(query): Query<RangeQuery>,
) -> Result<Json<SummaryOutput>, AppError> {
    let range = query
        .range
        .unwrap_or_else(|| query::DEFAULT_RANGE.to_string());
    let since = query::since(Some(&range))?.unwrap_or(0);

    let row = sqlx::query_as::<_, SummaryRow>(
        "SELECT
            COUNT(*) AS requests,
            COALESCE(SUM(CASE WHEN status_code < 400 THEN 1 ELSE 0 END), 0) AS successes,
            COALESCE(SUM(CASE WHEN status_code >= 400 THEN 1 ELSE 0 END), 0) AS errors,
            COALESCE(SUM(total_tokens), 0) AS total_tokens,
            COALESCE(SUM(prompt_tokens), 0) AS prompt_tokens,
            COALESCE(SUM(completion_tokens), 0) AS completion_tokens,
            COALESCE(SUM(cached_tokens), 0) AS cached_tokens,
            COALESCE(SUM(reasoning_tokens), 0) AS reasoning_tokens,
            COALESCE(AVG(latency_ms), 0.0) AS avg_latency_ms,
            AVG(first_token_ms) AS avg_first_token_ms,
            COUNT(DISTINCT channel_id) AS active_channels,
            COUNT(DISTINCT api_key_id) AS active_keys
         FROM usage_logs WHERE created_at >= ?",
    )
    .bind(since)
    .fetch_one(&state.db)
    .await?;

    let success_rate = if row.requests > 0 {
        row.successes as f64 / row.requests as f64
    } else {
        0.0
    };

    Ok(Json(SummaryOutput {
        range,
        requests: row.requests,
        successes: row.successes,
        errors: row.errors,
        success_rate,
        total_tokens: row.total_tokens,
        prompt_tokens: row.prompt_tokens,
        completion_tokens: row.completion_tokens,
        cached_tokens: row.cached_tokens,
        reasoning_tokens: row.reasoning_tokens,
        avg_latency_ms: row.avg_latency_ms,
        avg_first_token_ms: row.avg_first_token_ms,
        active_channels: row.active_channels,
        active_keys: row.active_keys,
    }))
}

#[derive(Debug, Serialize)]
pub struct TimeseriesPoint {
    pub timestamp: i64,
    pub requests: i64,
    pub errors: i64,
    pub total_tokens: i64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Serialize)]
pub struct TimeseriesOutput {
    pub range: String,
    pub bucket_seconds: i64,
    pub points: Vec<TimeseriesPoint>,
}

#[derive(Debug, FromRow)]
struct TimeseriesRow {
    bucket: i64,
    requests: i64,
    errors: i64,
    total_tokens: i64,
    avg_latency_ms: f64,
}

pub async fn timeseries(
    State(state): State<AppState>,
    _session: AdminSession,
    Query(query): Query<RangeQuery>,
) -> Result<Json<TimeseriesOutput>, AppError> {
    let range = query
        .range
        .unwrap_or_else(|| query::DEFAULT_RANGE.to_string());
    let (since, bucket_seconds) = query::window(Some(&range))?;

    let rows = sqlx::query_as::<_, TimeseriesRow>(
        "SELECT
            (created_at / 1000 / ?) * ? AS bucket,
            COUNT(*) AS requests,
            COALESCE(SUM(CASE WHEN status_code >= 400 THEN 1 ELSE 0 END), 0) AS errors,
            COALESCE(SUM(total_tokens), 0) AS total_tokens,
            COALESCE(AVG(latency_ms), 0.0) AS avg_latency_ms
         FROM usage_logs
         WHERE created_at >= ?
         GROUP BY bucket
         ORDER BY bucket",
    )
    .bind(bucket_seconds)
    .bind(bucket_seconds)
    .bind(since)
    .fetch_all(&state.db)
    .await?;

    let mut by_bucket: HashMap<i64, TimeseriesRow> =
        rows.into_iter().map(|row| (row.bucket, row)).collect();

    let mut points = Vec::new();
    let mut bucket = (since / 1000 / bucket_seconds) * bucket_seconds;
    let last = now_ms() / 1000 / bucket_seconds * bucket_seconds;
    while bucket <= last {
        let row = by_bucket.remove(&bucket);
        points.push(TimeseriesPoint {
            timestamp: bucket * 1000,
            requests: row.as_ref().map(|row| row.requests).unwrap_or(0),
            errors: row.as_ref().map(|row| row.errors).unwrap_or(0),
            total_tokens: row.as_ref().map(|row| row.total_tokens).unwrap_or(0),
            avg_latency_ms: row.as_ref().map(|row| row.avg_latency_ms).unwrap_or(0.0),
        });
        bucket += bucket_seconds;
    }

    Ok(Json(TimeseriesOutput {
        range,
        bucket_seconds,
        points,
    }))
}

#[derive(Debug, Deserialize)]
pub struct BreakdownQuery {
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub dimension: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BreakdownItem {
    pub key: String,
    pub label: String,
    pub requests: i64,
    pub total_tokens: i64,
    pub errors: i64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Serialize)]
pub struct BreakdownOutput {
    pub dimension: String,
    pub items: Vec<BreakdownItem>,
}

#[derive(Debug, FromRow)]
struct BreakdownRow {
    bucket_key: String,
    label: String,
    requests: i64,
    total_tokens: i64,
    errors: i64,
    avg_latency_ms: f64,
}

pub async fn breakdown(
    State(state): State<AppState>,
    _session: AdminSession,
    Query(query): Query<BreakdownQuery>,
) -> Result<Json<BreakdownOutput>, AppError> {
    let dimension = query.dimension.unwrap_or_else(|| "model".to_string());
    let (key_expr, label_expr) = match dimension.as_str() {
        "model" => ("l.model", "l.model"),
        "channel" => (
            "COALESCE(CAST(l.channel_id AS TEXT), 'unknown')",
            "COALESCE(c.name, '未知渠道')",
        ),
        "key" => (
            "COALESCE(CAST(l.api_key_id AS TEXT), 'unknown')",
            "COALESCE(k.name, '未知 Key')",
        ),
        "protocol" => ("l.inbound_protocol", "l.inbound_protocol"),
        "upstream_protocol" => ("l.upstream_protocol", "l.upstream_protocol"),
        "source" => ("l.source", "l.source"),
        other => {
            return Err(AppError::BadRequest(format!(
                "unsupported breakdown dimension `{other}`"
            )))
        }
    };

    let since = query::since(query.range.as_deref())?.unwrap_or(0);
    let sql = format!(
        "SELECT {key_expr} AS bucket_key, {label_expr} AS label,
            COUNT(*) AS requests,
            COALESCE(SUM(l.total_tokens), 0) AS total_tokens,
            COALESCE(SUM(CASE WHEN l.status_code >= 400 THEN 1 ELSE 0 END), 0) AS errors,
            COALESCE(AVG(l.latency_ms), 0.0) AS avg_latency_ms
         FROM usage_logs l
         LEFT JOIN channels c ON c.id = l.channel_id
         LEFT JOIN api_keys k ON k.id = l.api_key_id
         WHERE l.created_at >= ?
         GROUP BY bucket_key, label
         ORDER BY requests DESC
         LIMIT 20"
    );

    let rows = sqlx::query_as::<_, BreakdownRow>(&sql)
        .bind(since)
        .fetch_all(&state.db)
        .await?;

    Ok(Json(BreakdownOutput {
        dimension,
        items: rows
            .into_iter()
            .map(|row| BreakdownItem {
                key: row.bucket_key,
                label: row.label,
                requests: row.requests,
                total_tokens: row.total_tokens,
                errors: row.errors,
                avg_latency_ms: row.avg_latency_ms,
            })
            .collect(),
    }))
}
