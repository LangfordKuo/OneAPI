use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use serde_json::Value;
use sqlx::{Executor, Sqlite, SqlitePool};
use tokio::sync::mpsc;

use crate::{events::EventBus, models, protocol::Usage, state::AppState};

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

/// One stored request/response detail pair, queued so the gateway does not wait
/// for a second write transaction on every request.
pub struct DetailRecord<'a> {
    pub request_id: &'a str,
    pub api_key_id: i64,
    pub request_body: &'a str,
    pub response_body: &'a str,
    pub created_at: i64,
}

#[derive(Debug)]
struct OwnedDetailRecord {
    request_id: String,
    api_key_id: i64,
    request_body: String,
    response_body: String,
    created_at: i64,
}

impl From<&DetailRecord<'_>> for OwnedDetailRecord {
    fn from(record: &DetailRecord<'_>) -> Self {
        Self {
            request_id: record.request_id.to_string(),
            api_key_id: record.api_key_id,
            request_body: record.request_body.to_string(),
            response_body: record.response_body.to_string(),
            created_at: record.created_at,
        }
    }
}

/// Work item for the background writer. Usage rows and request details share
/// one bounded queue and one transaction per batch.
#[derive(Debug)]
enum WriteJob {
    Usage(Box<OwnedUsageRecord>),
    Detail(OwnedDetailRecord),
}

/// Serializes usage-log writes through a single background task. The queue is
/// bounded; when it is full the caller falls back to a synchronous write so no
/// record is silently dropped.
#[derive(Debug)]
pub struct UsageWriter {
    tx: Mutex<Option<mpsc::Sender<WriteJob>>>,
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    stats: Arc<UsageWriterStats>,
}

/// Counters surfaced by `readyz` so operators can see backpressure and loss
/// without grepping logs.
#[derive(Debug, Default)]
pub struct UsageWriterStats {
    sync_fallbacks: AtomicU64,
    failed_writes: AtomicU64,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct UsageWriterSnapshot {
    pub queue_depth: usize,
    pub queue_capacity: usize,
    pub sync_fallbacks: u64,
    pub failed_writes: u64,
}

impl UsageWriter {
    const QUEUE_CAPACITY: usize = 1_024;
    const BATCH_SIZE: usize = 100;

    pub fn start(pool: SqlitePool, events: Arc<EventBus>) -> Self {
        let (tx, rx) = mpsc::channel(Self::QUEUE_CAPACITY);
        let stats = Arc::new(UsageWriterStats::default());
        let handle = tokio::spawn(write_loop(pool, rx, Arc::clone(&stats), events));
        Self {
            tx: Mutex::new(Some(tx)),
            handle: Mutex::new(Some(handle)),
            stats,
        }
    }

    pub fn snapshot(&self) -> UsageWriterSnapshot {
        let slot = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (queue_depth, queue_capacity) = match slot.as_ref() {
            Some(tx) => (
                tx.max_capacity().saturating_sub(tx.capacity()),
                tx.max_capacity(),
            ),
            None => (0, Self::QUEUE_CAPACITY),
        };
        UsageWriterSnapshot {
            queue_depth,
            queue_capacity,
            sync_fallbacks: self.stats.sync_fallbacks.load(Ordering::Relaxed),
            failed_writes: self.stats.failed_writes.load(Ordering::Relaxed),
        }
    }

    fn note_sync_fallback(&self) {
        self.stats.sync_fallbacks.fetch_add(1, Ordering::Relaxed);
    }

    fn note_failed_writes(&self, count: u64) {
        self.stats.failed_writes.fetch_add(count, Ordering::Relaxed);
    }

    fn try_send(&self, job: WriteJob) -> bool {
        let slot = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match slot.as_ref() {
            Some(tx) => tx.try_send(job).is_ok(),
            // Shutdown has taken the sender; the caller writes synchronously.
            None => false,
        }
    }

    fn try_enqueue(&self, record: OwnedUsageRecord) -> bool {
        self.try_send(WriteJob::Usage(Box::new(record)))
    }

    fn try_enqueue_detail(&self, record: OwnedDetailRecord) -> bool {
        self.try_send(WriteJob::Detail(record))
    }

    /// Close the queue and wait until every queued record is persisted. Taking
    /// the sender first guarantees no record can be enqueued after the drain
    /// starts; later callers fall back to synchronous writes.
    pub async fn shutdown(&self) {
        let sender = self
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        drop(sender);

        let handle = self
            .handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }
}

/// Write a usage row, through the queue when available.
pub async fn record(state: &AppState, record: &UsageRecord<'_>) {
    if let Some(writer) = &state.usage_writer {
        if writer.try_enqueue(OwnedUsageRecord::from(record)) {
            return;
        }
        writer.note_sync_fallback();
        tracing::warn!(
            request_id = record.request_id,
            "usage write queue is full; writing synchronously"
        );
    }

    if let Err(error) = insert(&state.db, record).await {
        if let Some(writer) = &state.usage_writer {
            writer.note_failed_writes(1);
        }
        tracing::error!(
            error = %error,
            request_id = record.request_id,
            "failed to write usage log"
        );
    } else {
        state.events.usage_written(1);
    }
}

/// Store a request/response detail pair through the same bounded queue as the
/// usage rows. Falls back to a synchronous write when the queue is full or the
/// process is shutting down.
pub async fn record_detail(state: &AppState, record: &DetailRecord<'_>) {
    if let Some(writer) = &state.usage_writer {
        if writer.try_enqueue_detail(OwnedDetailRecord::from(record)) {
            return;
        }
        writer.note_sync_fallback();
        tracing::warn!(
            request_id = record.request_id,
            "detail write queue is full; writing synchronously"
        );
    }

    if let Err(error) = models::request_detail::insert(
        &state.db,
        record.request_id,
        record.api_key_id,
        record.request_body,
        record.response_body,
        record.created_at,
    )
    .await
    {
        tracing::warn!(
            error = %error,
            request_id = record.request_id,
            "failed to store request detail"
        );
    }
}

pub async fn insert(pool: &SqlitePool, record: &UsageRecord<'_>) -> sqlx::Result<()> {
    let owned = OwnedUsageRecord::from(record);
    let mut tx = pool.begin().await?;
    insert_one(&mut *tx, &owned).await?;
    if let Some(api_key_id) = owned.api_key_id {
        models::api_key::add_usage(
            &mut *tx,
            api_key_id,
            total_tokens(&owned.usage),
            owned.created_at,
        )
        .await?;
    }
    tx.commit().await
}

async fn insert_one<'e, E>(executor: E, record: &OwnedUsageRecord) -> sqlx::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "INSERT INTO usage_logs
         (request_id, source, api_key_id, detail_api_key_id, channel_id, model, upstream_model,
          inbound_protocol, upstream_protocol, stream, status_code, attempts,
          error_type, error_message, prompt_tokens, completion_tokens, total_tokens,
          cached_tokens, cache_creation_tokens, reasoning_tokens, latency_ms, first_token_ms,
          created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&record.request_id)
    .bind(&record.source)
    .bind(record.api_key_id)
    // Pins the detail row to the key that wrote it, independently of
    // usage_logs.api_key_id, which is cleared when the key is deleted.
    // Playground requests (no key) store NULL.
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

async fn write_loop(
    pool: SqlitePool,
    mut rx: mpsc::Receiver<WriteJob>,
    stats: Arc<UsageWriterStats>,
    events: Arc<EventBus>,
) {
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        while batch.len() < UsageWriter::BATCH_SIZE {
            match rx.try_recv() {
                Ok(job) => batch.push(job),
                Err(_) => break,
            }
        }

        if let Err(error) = write_batch(&pool, &batch, &events).await {
            stats
                .failed_writes
                .fetch_add(batch.len() as u64, Ordering::Relaxed);
            tracing::error!(
                error = %error,
                count = batch.len(),
                "failed to write usage batch"
            );
        }
    }
}

async fn write_batch(pool: &SqlitePool, batch: &[WriteJob], events: &EventBus) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    let mut key_usage: HashMap<i64, i64> = HashMap::new();
    let mut usage_rows = 0usize;
    for job in batch {
        match job {
            WriteJob::Usage(record) => {
                usage_rows += 1;
                insert_one(&mut *tx, record).await?;
                if let Some(api_key_id) = record.api_key_id {
                    *key_usage.entry(api_key_id).or_default() += total_tokens(&record.usage);
                }
            }
            WriteJob::Detail(record) => {
                models::request_detail::insert(
                    &mut *tx,
                    &record.request_id,
                    record.api_key_id,
                    &record.request_body,
                    &record.response_body,
                    record.created_at,
                )
                .await?;
            }
        }
    }
    let now = models::now_ms();
    for (api_key_id, tokens) in key_usage {
        models::api_key::add_usage(&mut *tx, api_key_id, tokens, now).await?;
    }
    tx.commit().await?;

    // Notify admin pages only after the rows are durable, so a refetch never
    // races ahead of the write it is meant to show.
    if usage_rows > 0 {
        events.usage_written(usage_rows);
    }
    Ok(())
}

fn total_tokens(usage: &Usage) -> i64 {
    let total = usage
        .total_tokens
        .or_else(|| match (usage.input_tokens, usage.output_tokens) {
            (Some(input), Some(output)) => Some(input + output),
            _ => None,
        });
    total
        .and_then(|value| i64::try_from(value).ok())
        .unwrap_or(0)
}

fn token(value: Option<u64>) -> Option<i64> {
    value.and_then(|value| i64::try_from(value).ok())
}

/// Approximate token count for free text (roughly four characters per token).
pub fn estimate_text_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// Byte-oriented variant used on the response path: valid UTF-8 (all JSON
/// responses) is counted without allocating an intermediate string.
pub fn estimate_bytes_tokens(bytes: &[u8]) -> u64 {
    match std::str::from_utf8(bytes) {
        Ok(text) => estimate_text_tokens(text),
        Err(_) => estimate_text_tokens(&String::from_utf8_lossy(bytes)),
    }
}

/// Approximate input token count for a request body. Only used when the
/// upstream omits its usage block, so an unaccounted request can never be
/// billed as zero tokens.
pub fn estimate_request_tokens(value: &Value) -> u64 {
    fn chars(value: &Value) -> u64 {
        match value {
            Value::String(text) => text.chars().count() as u64,
            Value::Array(items) => items.iter().map(chars).sum(),
            Value::Object(object) => object
                .iter()
                .filter(|(key, _)| key.as_str() != "model" && key.as_str() != "stream")
                .map(|(_, value)| chars(value))
                .sum(),
            _ => 0,
        }
    }

    chars(value).div_ceil(4).max(1)
}
