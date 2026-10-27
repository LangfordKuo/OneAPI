use sqlx::{Executor, Sqlite, SqlitePool};
use tokio::sync::mpsc;

use crate::{protocol::Usage, state::AppState};

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

#[derive(Debug)]
pub struct OwnedUsageRecord {
    request_id: String,
    source: String,
    api_key_id: Option<i64>,
    channel_id: Option<i64>,
    model: String,
    upstream_model: Option<String>,
    inbound_protocol: String,
    upstream_protocol: String,
    stream: bool,
    status_code: u16,
    attempts: u32,
    error_type: Option<String>,
    error_message: Option<String>,
    usage: Usage,
    latency_ms: i64,
    first_token_ms: Option<i64>,
    created_at: i64,
}

impl From<&UsageRecord<'_>> for OwnedUsageRecord {
    fn from(record: &UsageRecord<'_>) -> Self {
        Self {
            request_id: record.request_id.to_string(),
            source: record.source.to_string(),
            api_key_id: record.api_key_id,
            channel_id: record.channel_id,
            model: record.model.to_string(),
            upstream_model: record.upstream_model.map(str::to_string),
            inbound_protocol: record.inbound_protocol.to_string(),
            upstream_protocol: record.upstream_protocol.to_string(),
            stream: record.stream,
            status_code: record.status_code,
            attempts: record.attempts,
            error_type: record.error_type.map(str::to_string),
            error_message: record.error_message.map(str::to_string),
            usage: record.usage.clone(),
            latency_ms: record.latency_ms,
            first_token_ms: record.first_token_ms,
            created_at: record.created_at,
        }
    }
}

/// Serializes usage-log writes through a single background task. The queue is
/// bounded; when it is full the caller falls back to a synchronous write so no
/// record is silently dropped.
#[derive(Debug)]
pub struct UsageWriter {
    tx: mpsc::Sender<OwnedUsageRecord>,
}

impl UsageWriter {
    const QUEUE_CAPACITY: usize = 1_024;
    const BATCH_SIZE: usize = 100;

    pub fn start(pool: SqlitePool) -> Self {
        let (tx, rx) = mpsc::channel(Self::QUEUE_CAPACITY);
        tokio::spawn(write_loop(pool, rx));
        Self { tx }
    }

    fn try_enqueue(&self, record: OwnedUsageRecord) -> bool {
        self.tx.try_send(record).is_ok()
    }
}

/// Write a usage row, through the queue when available.
pub async fn record(state: &AppState, record: &UsageRecord<'_>) {
    if let Some(writer) = &state.usage_writer {
        if writer.try_enqueue(OwnedUsageRecord::from(record)) {
            return;
        }
        tracing::warn!(
            request_id = record.request_id,
            "usage write queue is full; writing synchronously"
        );
    }

    if let Err(error) = insert(&state.db, record).await {
        tracing::error!(
            error = %error,
            request_id = record.request_id,
            "failed to write usage log"
        );
    }
}

pub async fn insert(pool: &SqlitePool, record: &UsageRecord<'_>) -> sqlx::Result<()> {
    insert_one(pool, &OwnedUsageRecord::from(record)).await
}

async fn insert_one<'e, E>(executor: E, record: &OwnedUsageRecord) -> sqlx::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "INSERT INTO usage_logs
         (request_id, source, api_key_id, channel_id, model, upstream_model,
          inbound_protocol, upstream_protocol, stream, status_code, attempts,
          error_type, error_message, prompt_tokens, completion_tokens, total_tokens,
          cached_tokens, cache_creation_tokens, reasoning_tokens, latency_ms, first_token_ms,
          created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&record.request_id)
    .bind(&record.source)
    .bind(record.api_key_id)
    .bind(record.channel_id)
    .bind(&record.model)
    .bind(&record.upstream_model)
    .bind(&record.inbound_protocol)
    .bind(&record.upstream_protocol)
    .bind(if record.stream { 1i64 } else { 0i64 })
    .bind(i64::from(record.status_code))
    .bind(i64::from(record.attempts))
    .bind(&record.error_type)
    .bind(&record.error_message)
    .bind(token(record.usage.input_tokens))
    .bind(token(record.usage.output_tokens))
    .bind(token(record.usage.total_tokens))
    .bind(token(record.usage.cached_tokens))
    .bind(token(record.usage.cache_creation_tokens))
    .bind(token(record.usage.reasoning_tokens))
    .bind(record.latency_ms)
    .bind(record.first_token_ms)
    .bind(record.created_at)
    .execute(executor)
    .await?;

    Ok(())
}

async fn write_loop(pool: SqlitePool, mut rx: mpsc::Receiver<OwnedUsageRecord>) {
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        while batch.len() < UsageWriter::BATCH_SIZE {
            match rx.try_recv() {
                Ok(record) => batch.push(record),
                Err(_) => break,
            }
        }

        if let Err(error) = write_batch(&pool, &batch).await {
            tracing::error!(
                error = %error,
                count = batch.len(),
                "failed to write usage batch"
            );
        }
    }
}

async fn write_batch(pool: &SqlitePool, batch: &[OwnedUsageRecord]) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    for record in batch {
        insert_one(&mut *tx, record).await?;
    }
    tx.commit().await
}

fn token(value: Option<u64>) -> Option<i64> {
    value.and_then(|value| i64::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn queued_records_are_persisted() {
        let temp = TempDir::new().expect("temp dir");
        let config = crate::config::Config {
            addr: "127.0.0.1:0".parse().expect("address"),
            data_dir: temp.path().to_path_buf(),
            database_url: None,
            database_path: temp.path().join("test.db"),
            log_level: "error".to_string(),
            cookie_secure: false,
            trust_proxy: false,
        };
        let pool = crate::db::connect(&config).await.expect("database");
        let writer = UsageWriter::start(pool.clone());

        writer.try_enqueue(OwnedUsageRecord {
            request_id: "req-queued".to_string(),
            source: "gateway".to_string(),
            api_key_id: None,
            channel_id: None,
            model: "gpt-test".to_string(),
            upstream_model: None,
            inbound_protocol: "chat".to_string(),
            upstream_protocol: "chat".to_string(),
            stream: false,
            status_code: 200,
            attempts: 1,
            error_type: None,
            error_message: None,
            usage: Usage::default(),
            latency_ms: 5,
            first_token_ms: None,
            created_at: crate::models::now_ms(),
        });

        for _ in 0..100 {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_logs")
                .fetch_one(&pool)
                .await
                .expect("count");
            if count == 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        panic!("queued usage record was not persisted");
    }
}
