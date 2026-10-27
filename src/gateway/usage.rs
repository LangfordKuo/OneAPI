use sqlx::SqlitePool;

use crate::protocol::Usage;

pub struct UsageRecord<'a> {
    pub request_id: &'a str,
    pub source: &'a str,
    pub api_key_id: Option<i64>,
    pub channel_id: Option<i64>,
    pub model: &'a str,
    pub upstream_model: Option<&'a str>,
    pub inbound_protocol: &'a str,
    pub upstream_protocol: &'a str,
    pub stream: bool,
    pub status_code: u16,
    pub attempts: u32,
    pub error_type: Option<&'a str>,
    pub error_message: Option<&'a str>,
    pub usage: &'a Usage,
    pub latency_ms: i64,
    pub first_token_ms: Option<i64>,
    pub created_at: i64,
}

pub async fn insert(pool: &SqlitePool, record: &UsageRecord<'_>) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO usage_logs
         (request_id, source, api_key_id, channel_id, model, upstream_model,
          inbound_protocol, upstream_protocol, stream, status_code, attempts,
          error_type, error_message, prompt_tokens, completion_tokens, total_tokens,
          cached_tokens, cache_creation_tokens, reasoning_tokens, latency_ms, first_token_ms,
          created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(record.request_id)
    .bind(record.source)
    .bind(record.api_key_id)
    .bind(record.channel_id)
    .bind(record.model)
    .bind(record.upstream_model)
    .bind(record.inbound_protocol)
    .bind(record.upstream_protocol)
    .bind(if record.stream { 1i64 } else { 0i64 })
    .bind(i64::from(record.status_code))
    .bind(i64::from(record.attempts))
    .bind(record.error_type)
    .bind(record.error_message)
    .bind(token(record.usage.input_tokens))
    .bind(token(record.usage.output_tokens))
    .bind(token(record.usage.total_tokens))
    .bind(token(record.usage.cached_tokens))
    .bind(token(record.usage.cache_creation_tokens))
    .bind(token(record.usage.reasoning_tokens))
    .bind(record.latency_ms)
    .bind(record.first_token_ms)
    .bind(record.created_at)
    .execute(pool)
    .await?;

    Ok(())
}

fn token(value: Option<u64>) -> Option<i64> {
    value.and_then(|value| i64::try_from(value).ok())
}
