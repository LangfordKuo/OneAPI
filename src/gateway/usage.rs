use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use serde_json::Value;
use sqlx::{Executor, Sqlite, SqliteConnection, SqlitePool};
use tokio::sync::mpsc;

use crate::{
    events::EventBus,
    models,
    protocol::{Usage, MAX_TOKEN_COUNT},
    state::AppState,
};

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
    /// Tokens reserved at admission for this request, settled by the writer.
    /// `0` means the request made no reservation.
    pub reserved_tokens: i64,
    /// UTC day the reservation belongs to; only meaningful with
    /// `reserved_tokens > 0`.
    pub reserved_day: i64,
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
    reserved_tokens: i64,
    reserved_day: i64,
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
            reserved_tokens: record.reserved_tokens,
            reserved_day: record.reserved_day,
            created_at: record.created_at,
        }
    }
}

/// Why a reservation was refused. Mapped to the protocol-shaped client error by
/// the gateway; `Database` is an internal failure, not a quota decision.
#[derive(Debug, thiserror::Error)]
pub enum ReserveError {
    #[error("API key token limit reached")]
    TokenLimit,
    #[error("API key daily token limit reached")]
    DailyLimit,
    #[error("API key is disabled")]
    Disabled,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// In-flight admission reservation.
///
/// The guard is armed when the counters were incremented. Until [`disarm`] is
/// called, dropping it (early return, cancellation, panic) spawns a release so
/// a failed request cannot pin quota forever. Once the usage row carrying the
/// reservation has been handed to the writer, the row itself settles it, so the
/// guard is disarmed to avoid releasing twice.
///
/// [`disarm`]: Reservation::disarm
pub struct Reservation {
    state: AppState,
    key_id: i64,
    tokens: i64,
    day: i64,
    armed: bool,
}

impl Reservation {
    pub fn tokens(&self) -> i64 {
        self.tokens
    }

    pub fn day(&self) -> i64 {
        self.day
    }

    /// The reservation was handed to a usage row (or deliberately abandoned
    /// with a compensating release); do not release it on drop.
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let state = self.state.clone();
        let key_id = self.key_id;
        let tokens = self.tokens;
        let day = self.day;
        // `Drop` cannot await. A detached release is enough: if it fails the
        // janitor will clear the counters once the reservation goes stale.
        let release = async move {
            if let Err(error) =
                models::api_key::release_reservation(&state.db, key_id, tokens, day).await
            {
                tracing::warn!(
                    error = %error,
                    key_id,
                    tokens,
                    "failed to release quota reservation"
                );
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(release);
            }
            Err(_) => tracing::warn!(
                key_id,
                tokens,
                "runtime is gone; quota reservation left for the janitor"
            ),
        }
    }
}

/// Whether the emergency switch `ONEAPI_DISABLE_QUOTA_RESERVATION=1` is set.
/// Read per request so an operator can turn reservation off without a rebuild.
fn reservation_disabled() -> bool {
    std::env::var("ONEAPI_DISABLE_QUOTA_RESERVATION")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

/// Reserve `estimate` tokens for `key_id` before an upstream attempt.
///
/// Returns `Ok(None)` when reservation is disabled by the emergency switch, so
/// the caller simply falls back to post-hoc accounting. `day` is the UTC day
/// (milliseconds / 86_400_000) the request was admitted on; settlement uses the
/// same value.
pub async fn reserve(
    state: &AppState,
    key_id: i64,
    estimate: i64,
    day: i64,
) -> Result<Option<Reservation>, ReserveError> {
    if reservation_disabled() {
        return Ok(None);
    }

    let tokens = estimate.max(1);
    match models::api_key::reserve(&state.db, key_id, tokens, day, models::now_ms()).await? {
        models::api_key::ReserveOutcome::Reserved => Ok(Some(Reservation {
            state: state.clone(),
            key_id,
            tokens,
            day,
            armed: true,
        })),
        models::api_key::ReserveOutcome::TokenLimit => Err(ReserveError::TokenLimit),
        models::api_key::ReserveOutcome::DailyLimit => Err(ReserveError::DailyLimit),
        models::api_key::ReserveOutcome::Disabled => Err(ReserveError::Disabled),
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
    /// Bytes this record holds in the queue, measured after truncation. Kept
    /// with the record so the writer can release exactly what it reserved.
    size: usize,
}

impl From<&DetailRecord<'_>> for OwnedDetailRecord {
    fn from(record: &DetailRecord<'_>) -> Self {
        // Truncate before copying so an oversized body is never duplicated in
        // full just to be discarded by `request_detail::insert` later.
        let request_body = models::request_detail::truncate_detail(record.request_body);
        let response_body = models::request_detail::truncate_detail(record.response_body);
        let size = record.request_id.len() + request_body.len() + response_body.len();
        Self {
            request_id: record.request_id.to_string(),
            api_key_id: record.api_key_id,
            request_body,
            response_body,
            created_at: record.created_at,
            size,
        }
    }
}

/// Outcome of queueing one request detail. `Full` keeps the synchronous
/// fallback; `ByteLimit` drops the detail instead of blocking the request path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailEnqueue {
    Queued,
    ByteLimit,
    Full,
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
/// record is silently dropped. Request details additionally have a byte budget:
/// past it they are dropped instead of being written inline.
#[derive(Debug)]
pub struct UsageWriter {
    tx: Mutex<Option<mpsc::Sender<WriteJob>>>,
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    stats: Arc<UsageWriterStats>,
    /// Bytes reserved by detail records currently in the queue.
    queued_detail_bytes: Arc<AtomicUsize>,
    detail_dropped: Arc<AtomicU64>,
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
    pub detail_queue_bytes: usize,
    pub detail_dropped: u64,
}

impl UsageWriter {
    const QUEUE_CAPACITY: usize = 1_024;
    const BATCH_SIZE: usize = 100;
    /// Upper bound on memory held by queued request details. Bodies are already
    /// truncated to 64 KiB, but a full channel of them would still pin tens of
    /// MiB; beyond this point details are dropped rather than buffered.
    const MAX_DETAIL_QUEUE_BYTES: usize = 16 * 1024 * 1024;

    pub fn start(pool: SqlitePool, events: Arc<EventBus>) -> Self {
        let (tx, rx) = mpsc::channel(Self::QUEUE_CAPACITY);
        let stats = Arc::new(UsageWriterStats::default());
        let queued_detail_bytes = Arc::new(AtomicUsize::new(0));
        let detail_dropped = Arc::new(AtomicU64::new(0));
        let handle = tokio::spawn(write_loop(
            pool,
            rx,
            Arc::clone(&stats),
            Arc::clone(&queued_detail_bytes),
            events,
        ));
        Self {
            tx: Mutex::new(Some(tx)),
            handle: Mutex::new(Some(handle)),
            stats,
            queued_detail_bytes,
            detail_dropped,
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
            detail_queue_bytes: self.queued_detail_bytes.load(Ordering::Relaxed),
            detail_dropped: self.detail_dropped.load(Ordering::Relaxed),
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

    fn try_enqueue_detail(&self, record: OwnedDetailRecord) -> DetailEnqueue {
        let size = record.size;

        // Reserve the bytes before handing the job over, so concurrent
        // producers cannot jointly overshoot the cap.
        let mut queued = self.queued_detail_bytes.load(Ordering::Relaxed);
        loop {
            if queued.saturating_add(size) > Self::MAX_DETAIL_QUEUE_BYTES {
                self.detail_dropped.fetch_add(1, Ordering::Relaxed);
                return DetailEnqueue::ByteLimit;
            }
            match self.queued_detail_bytes.compare_exchange_weak(
                queued,
                queued + size,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(current) => queued = current,
            }
        }

        if !self.try_send(WriteJob::Detail(record)) {
            self.queued_detail_bytes.fetch_sub(size, Ordering::AcqRel);
            return DetailEnqueue::Full;
        }
        DetailEnqueue::Queued
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
///
/// Returns whether the row was handed to the writer (or written synchronously).
/// `false` means the row was not persisted, so the caller must keep a quota
/// reservation armed instead of disarming it: nothing else will settle it.
pub async fn record(state: &AppState, record: &UsageRecord<'_>) -> bool {
    if let Some(writer) = &state.usage_writer {
        if writer.try_enqueue(OwnedUsageRecord::from(record)) {
            return true;
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
        false
    } else {
        state.events.usage_written(1);
        true
    }
}

/// Store a request/response detail pair through the same bounded queue as the
/// usage rows. Falls back to a synchronous write when the queue is full or the
/// process is shutting down, and drops the detail once the queue's byte budget
/// is exhausted so a burst of large bodies cannot pin memory.
pub async fn record_detail(state: &AppState, record: &DetailRecord<'_>) {
    if let Some(writer) = &state.usage_writer {
        match writer.try_enqueue_detail(OwnedDetailRecord::from(record)) {
            DetailEnqueue::Queued => return,
            // The byte budget is exhausted: drop the detail rather than block
            // the request or pile more bodies into memory.
            DetailEnqueue::ByteLimit => {
                tracing::debug!(
                    request_id = record.request_id,
                    "detail write queue is over its byte budget; dropping detail"
                );
                return;
            }
            DetailEnqueue::Full => {
                writer.note_sync_fallback();
                tracing::warn!(
                    request_id = record.request_id,
                    "detail write queue is full; writing synchronously"
                );
            }
        }
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

/// Settle one API key's admission reservation and actual usage on `executor`.
///
/// This is the single settlement path shared by the synchronous write and the
/// batch writer. The two callers differ only in the day used to assign the
/// charge: the synchronous path keeps the row's `created_at` (the request was
/// admitted then), while the batch path uses the batch write time. Both are
/// passed in as `now_ms`.
///
/// Releases run before `add_usage` so the same request can never be counted
/// twice (reserved + actual). The executor is reborrowed for every statement so
/// all of this stays on the caller's transaction.
async fn settle_key<'e, E>(
    mut executor: E,
    api_key_id: i64,
    releases: &[(i64, i64)],
    actual_tokens: i64,
    now_ms: i64,
) -> sqlx::Result<()>
where
    E: Executor<'e, Database = Sqlite> + std::ops::DerefMut<Target = SqliteConnection>,
{
    for &(day, tokens) in releases {
        models::api_key::release_reservation(&mut *executor, api_key_id, tokens, day).await?;
    }
    models::api_key::add_usage(&mut *executor, api_key_id, actual_tokens, now_ms).await?;
    Ok(())
}

pub async fn insert(pool: &SqlitePool, record: &UsageRecord<'_>) -> sqlx::Result<()> {
    let owned = OwnedUsageRecord::from(record);
    let mut tx = pool.begin().await?;
    insert_one(&mut *tx, &owned).await?;
    if let Some(api_key_id) = owned.api_key_id {
        // Settlement: give the reservation back before charging the actual
        // usage, so the same request is never counted twice. The charge keeps
        // the row's `created_at` day, matching the pre-refactor behaviour.
        let release = [(owned.reserved_day, owned.reserved_tokens)];
        let releases: &[(i64, i64)] = if owned.reserved_tokens > 0 {
            &release
        } else {
            &[]
        };
        settle_key(
            &mut *tx,
            api_key_id,
            releases,
            total_tokens(&owned.usage),
            owned.created_at,
        )
        .await?;
    }
    tx.commit().await
}

/// Bounds for the text fields that land in indexed usage-log columns.
const MAX_ERROR_TYPE_LEN: usize = 128;
const MAX_ERROR_MESSAGE_LEN: usize = 1024;

/// Clamp a usage-log text field to `max_chars` Unicode scalar values before it
/// is bound to a column. Char-based truncation never splits a UTF-8 sequence.
fn truncate_field<'a>(field: &'static str, value: &'a str, max_chars: usize) -> Cow<'a, str> {
    let chars = value.chars().count();
    if chars <= max_chars {
        return Cow::Borrowed(value);
    }
    tracing::warn!(
        field,
        chars,
        max_chars,
        "usage log field exceeds the storage limit; truncating"
    );
    Cow::Owned(value.chars().take(max_chars).collect())
}

async fn insert_one<'e, E>(executor: E, record: &OwnedUsageRecord) -> sqlx::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    // Write-side backstop: the request path validates model names, but any
    // caller can still hand the writer a long value. Clamp the text fields
    // that feed indexed columns so one row cannot bloat the index.
    let model = truncate_field(
        "model",
        &record.model,
        models::usage_log::MAX_MODEL_NAME_LEN,
    );
    let upstream_model = record.upstream_model.as_deref().map(|value| {
        truncate_field(
            "upstream_model",
            value,
            models::usage_log::MAX_MODEL_NAME_LEN,
        )
    });
    let error_type = record
        .error_type
        .as_deref()
        .map(|value| truncate_field("error_type", value, MAX_ERROR_TYPE_LEN));
    let error_message = record
        .error_message
        .as_deref()
        .map(|value| truncate_field("error_message", value, MAX_ERROR_MESSAGE_LEN));

    // The two foreign keys are normalized through an existence check instead of
    // binding the id directly. A key or channel deleted between admission and
    // this batch write would otherwise make the whole 100-row transaction fail
    // on the FK constraint; a missing row now stores NULL, which is exactly
    // what the schema's ON DELETE SET NULL produces for already-written rows.
    sqlx::query(
        "INSERT INTO usage_logs
         (request_id, source, api_key_id, detail_api_key_id, channel_id, model, upstream_model,
          inbound_protocol, upstream_protocol, stream, status_code, attempts,
          error_type, error_message, prompt_tokens, completion_tokens, total_tokens,
          cached_tokens, cache_creation_tokens, reasoning_tokens, latency_ms, first_token_ms,
          created_at)
         VALUES (?, ?, (SELECT id FROM api_keys WHERE id = ?), ?,
                 (SELECT id FROM channels WHERE id = ?), ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&record.request_id)
    .bind(&record.source)
    .bind(record.api_key_id)
    // Pins the detail row to the key that wrote it, independently of
    // usage_logs.api_key_id, which is cleared when the key is deleted.
    // Playground requests (no key) store NULL.
    .bind(record.api_key_id)
    .bind(record.channel_id)
    .bind(model.as_ref())
    .bind(upstream_model.as_deref())
    .bind(&record.inbound_protocol)
    .bind(&record.upstream_protocol)
    .bind(if record.stream { 1i64 } else { 0i64 })
    .bind(i64::from(record.status_code))
    .bind(i64::from(record.attempts))
    .bind(error_type.as_deref())
    .bind(error_message.as_deref())
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
    queued_detail_bytes: Arc<AtomicUsize>,
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

        // Release the byte reservations once the batch has been attempted,
        // whether it committed or rolled back, so the counter cannot leak.
        for job in &batch {
            if let WriteJob::Detail(record) = job {
                // Manual CAS loop keeps MSRV 1.82 compatibility; `take`/`try_update`
                // naming changed after this crate's minimum toolchain.
                let mut current = queued_detail_bytes.load(Ordering::Relaxed);
                loop {
                    let next = current.saturating_sub(record.size);
                    match queued_detail_bytes.compare_exchange_weak(
                        current,
                        next,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(observed) => current = observed,
                    }
                }
            }
        }
    }
}

async fn write_batch(pool: &SqlitePool, batch: &[WriteJob], events: &EventBus) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    let mut key_usage: HashMap<i64, i64> = HashMap::new();
    // Per key, the reservations to give back, grouped by their UTC day. Rows
    // from the same request are aggregated so settlement is one UPDATE per key
    // and day instead of one per row.
    let mut key_releases: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
    let mut usage_rows = 0usize;
    for job in batch {
        match job {
            WriteJob::Usage(record) => {
                usage_rows += 1;
                insert_one(&mut *tx, record).await?;
                if let Some(api_key_id) = record.api_key_id {
                    *key_usage.entry(api_key_id).or_default() += total_tokens(&record.usage);
                    if record.reserved_tokens > 0 {
                        let releases = key_releases.entry(api_key_id).or_default();
                        match releases
                            .iter_mut()
                            .find(|(day, _)| *day == record.reserved_day)
                        {
                            Some((_, tokens)) => *tokens += record.reserved_tokens,
                            None => releases.push((record.reserved_day, record.reserved_tokens)),
                        }
                    }
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
    // Release reservations first, then charge actual usage: the counters are
    // MAX(0, ...) guarded, and this ordering keeps a partially applied batch
    // (rolled back as a whole anyway) from double counting.
    //
    // Both maps are filled from the same `api_key_id` branch above, so every
    // key with a release also has a usage aggregate. Iterating `key_usage` and
    // handing each key its own releases keeps the per-key order (release, then
    // charge) while settling everything in one transaction. The batch path
    // keeps its existing day assignment: the charge uses the write time.
    let now = models::now_ms();
    for (api_key_id, tokens) in key_usage {
        let releases = key_releases.remove(&api_key_id).unwrap_or_default();
        settle_key(&mut *tx, api_key_id, &releases, tokens, now).await?;
    }
    // Defensive: the aggregation above cannot produce a key with reservations
    // but no usage row. If that invariant ever breaks, release the reservation
    // without charging (add_usage ignores a zero token count), which is what
    // the previous two-pass write did.
    for (api_key_id, releases) in key_releases {
        settle_key(&mut *tx, api_key_id, &releases, 0, now).await?;
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
            (Some(input), Some(output)) => Some(input.saturating_add(output)),
            _ => None,
        });
    match total {
        Some(value) => i64::try_from(value).unwrap_or_else(|_| {
            // A total outside i64 cannot be billed faithfully. Fall back to the
            // per-field counters (already filled with local estimates when the
            // upstream omitted usage) rather than recording zero tokens.
            let estimate = usage
                .input_tokens
                .unwrap_or(0)
                .saturating_add(usage.output_tokens.unwrap_or(0));
            token_i64(estimate).max(1)
        }),
        None => 0,
    }
}

fn token(value: Option<u64>) -> Option<i64> {
    value.map(token_i64)
}

/// Clamp a token counter to the same ceiling used when parsing upstream usage,
/// so a value that does not fit in the database's i64 column is still recorded
/// (bounded) instead of being dropped as NULL/zero.
fn token_i64(value: u64) -> i64 {
    let bounded = value.min(MAX_TOKEN_COUNT);
    i64::try_from(bounded).unwrap_or(MAX_TOKEN_COUNT as i64)
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
