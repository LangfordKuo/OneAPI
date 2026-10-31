use serde::de::Error as _;
use sqlx::{Executor, FromRow, Sqlite, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct ApiKey {
    pub id: i64,
    pub name: String,
    #[allow(dead_code)]
    pub key_hash: String,
    /// Peppered hash; `None` for keys created before migration 0009.
    pub key_hash_v2: Option<String>,
    pub key_prefix: String,
    pub enabled: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub allowed_channel_ids: String,
    pub allowed_models: String,
    pub token_limit: Option<i64>,
    pub token_usage: i64,
    pub daily_token_limit: Option<i64>,
    pub daily_token_usage: i64,
    pub daily_usage_day: i64,
    /// Tokens held by in-flight requests. Settled (released) when their usage
    /// row is written, or by the stale-reservation janitor after a crash.
    pub reserved_tokens: i64,
    pub reserved_daily_tokens: i64,
    pub reserved_day: i64,
    pub created_at: i64,
}

/// Result of an admission-time quota reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveOutcome {
    Reserved,
    TokenLimit,
    DailyLimit,
    Disabled,
}

/// Parsed per-key restrictions. Both lists use the same convention: an empty
/// list means "no restriction". Parsing is fallible so a malformed allowlist
/// can never be mistaken for an empty (unrestricted) one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyLimits {
    pub channel_ids: Vec<i64>,
    pub models: Vec<String>,
}

impl ApiKeyLimits {
    /// Empty means "all models"; `*` also matches everything.
    pub fn allows_model(&self, model: &str) -> bool {
        self.models.is_empty() || self.models.iter().any(|item| item == "*" || item == model)
    }
}

impl ApiKey {
    /// Parse both allowlists. Malformed JSON, or a non-positive channel id,
    /// is an error: it must never degrade to "no restriction".
    pub fn parse_limits(&self) -> Result<ApiKeyLimits, serde_json::Error> {
        let channel_ids: Vec<i64> = serde_json::from_str(&self.allowed_channel_ids)?;
        let channel_ids = parse_channel_ids(&channel_ids).map_err(|invalid| {
            serde_json::Error::custom(format!("invalid channel id {invalid}"))
        })?;
        let models: Vec<String> = serde_json::from_str(&self.allowed_models)?;
        Ok(ApiKeyLimits {
            channel_ids,
            models,
        })
    }

    pub fn token_limit_reached(&self) -> bool {
        self.token_limit
            .is_some_and(|limit| limit > 0 && self.token_usage >= limit)
    }

    /// Daily usage resets lazily: a row from an earlier UTC day counts as zero
    /// until the next usage write.
    pub fn usage_today(&self, today: i64) -> i64 {
        if self.daily_usage_day == today {
            self.daily_token_usage
        } else {
            0
        }
    }

    pub fn daily_limit_reached(&self, today: i64) -> bool {
        self.daily_token_limit.is_some_and(|limit| {
            limit > 0 && self.daily_usage_day == today && self.daily_token_usage >= limit
        })
    }
}

#[derive(Debug)]
pub struct ApiKeyCreate<'a> {
    pub name: &'a str,
    pub key_hash: &'a str,
    pub key_hash_v2: &'a str,
    pub key_prefix: &'a str,
    pub enabled: i64,
    pub expires_at: Option<i64>,
    pub allowed_channel_ids: &'a str,
    pub allowed_models: &'a str,
    pub token_limit: Option<i64>,
    pub daily_token_limit: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug)]
pub struct ApiKeyUpdate<'a> {
    pub name: &'a str,
    pub enabled: i64,
    pub expires_at: Option<i64>,
    pub allowed_channel_ids: &'a str,
    pub allowed_models: &'a str,
    pub token_limit: Option<i64>,
    pub daily_token_limit: Option<i64>,
}

const SELECT_COLUMNS: &str = "id, name, key_hash, key_hash_v2, key_prefix, enabled, expires_at,
    last_used_at, allowed_channel_ids, allowed_models, token_limit, token_usage, daily_token_limit,
    daily_token_usage, daily_usage_day, reserved_tokens, reserved_daily_tokens, reserved_day,
    created_at";

pub async fn list(pool: &SqlitePool) -> sqlx::Result<Vec<ApiKey>> {
    let query = format!("SELECT {SELECT_COLUMNS} FROM api_keys ORDER BY id DESC");
    sqlx::query_as(&query).fetch_all(pool).await
}

pub async fn get(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<ApiKey>> {
    let query = format!("SELECT {SELECT_COLUMNS} FROM api_keys WHERE id = ?");
    sqlx::query_as(&query).bind(id).fetch_optional(pool).await
}

pub async fn find_by_key_hash(
    pool: &SqlitePool,
    v2_hash: &str,
    legacy_hash: &str,
) -> sqlx::Result<Option<ApiKey>> {
    let query =
        format!("SELECT {SELECT_COLUMNS} FROM api_keys WHERE key_hash_v2 = ?1 OR key_hash = ?2");
    sqlx::query_as(&query)
        .bind(v2_hash)
        .bind(legacy_hash)
        .fetch_optional(pool)
        .await
}

/// Backfill the peppered hash for a key that only has the legacy one. The
/// `IS NULL` guard keeps a concurrent upgrade from overwriting the first
/// writer's value.
pub async fn set_key_hash_v2(pool: &SqlitePool, id: i64, hash: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE api_keys SET key_hash_v2 = ? WHERE id = ? AND key_hash_v2 IS NULL")
        .bind(hash)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn touch_last_used(pool: &SqlitePool, id: i64, now: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn create_with_limits(
    pool: &SqlitePool,
    input: &ApiKeyCreate<'_>,
) -> sqlx::Result<ApiKey> {
    let result = sqlx::query(
        "INSERT INTO api_keys
         (name, key_hash, key_hash_v2, key_prefix, enabled, expires_at, allowed_channel_ids,
          allowed_models, token_limit, daily_token_limit, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(input.name)
    .bind(input.key_hash)
    .bind(input.key_hash_v2)
    .bind(input.key_prefix)
    .bind(input.enabled)
    .bind(input.expires_at)
    .bind(input.allowed_channel_ids)
    .bind(input.allowed_models)
    .bind(input.token_limit)
    .bind(input.daily_token_limit)
    .bind(input.created_at)
    .execute(pool)
    .await?;

    get(pool, result.last_insert_rowid())
        .await?
        .ok_or(sqlx::Error::RowNotFound)
}

pub async fn update(pool: &SqlitePool, id: i64, input: &ApiKeyUpdate<'_>) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE api_keys SET
            name = ?, enabled = ?, expires_at = ?, allowed_channel_ids = ?, allowed_models = ?,
            token_limit = ?, daily_token_limit = ?
         WHERE id = ?",
    )
    .bind(input.name)
    .bind(input.enabled)
    .bind(input.expires_at)
    .bind(input.allowed_channel_ids)
    .bind(input.allowed_models)
    .bind(input.token_limit)
    .bind(input.daily_token_limit)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn disable(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE api_keys SET enabled = 0 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Add token usage and auto-disable the key when it reaches its lifetime
/// limit. Runs inside the usage-log transaction so the counter and the log
/// stay consistent. Daily counters roll over lazily on the first write of a
/// new UTC day.
pub async fn add_usage<'e, E>(executor: E, id: i64, tokens: i64, now_ms: i64) -> sqlx::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    if tokens <= 0 {
        return Ok(());
    }

    let day = now_ms / 86_400_000;
    sqlx::query(
        "UPDATE api_keys SET
            daily_token_usage = CASE WHEN daily_usage_day = ? THEN daily_token_usage + ? ELSE ? END,
            daily_usage_day = ?,
            token_usage = token_usage + ?,
            enabled = CASE
                WHEN token_limit IS NOT NULL AND token_limit > 0
                     AND token_usage + ? >= token_limit THEN 0
                ELSE enabled
            END
         WHERE id = ?",
    )
    .bind(day)
    .bind(tokens)
    .bind(tokens)
    .bind(day)
    .bind(tokens)
    .bind(tokens)
    .bind(id)
    .execute(executor)
    .await?;
    Ok(())
}

/// Admission-time reservation. A single conditional UPDATE keeps the quota
/// check and the counter increment atomic, so concurrent requests cannot all
/// pass the same preimage (the TOCTOU window the asynchronous usage writer
/// used to leave open).
///
/// `token_limit`/`daily_token_limit` that are NULL or `<= 0` mean "unlimited";
/// the daily counters only apply to the UTC day they were recorded for.
pub async fn reserve(
    pool: &SqlitePool,
    id: i64,
    estimate: i64,
    day: i64,
    now: i64,
) -> sqlx::Result<ReserveOutcome> {
    let estimate = estimate.max(1);
    let result = sqlx::query(
        "UPDATE api_keys
         SET reserved_tokens = reserved_tokens + ?1,
             reserved_daily_tokens = CASE WHEN reserved_day = ?2
                 THEN reserved_daily_tokens + ?1 ELSE ?1 END,
             reserved_day = ?2,
             reserved_at = ?3
         WHERE id = ?4
           AND enabled = 1
           AND (token_limit IS NULL OR token_limit <= 0
                OR token_usage + reserved_tokens + ?1 <= token_limit)
           AND (daily_token_limit IS NULL OR daily_token_limit <= 0
                OR (CASE WHEN daily_usage_day = ?2 THEN daily_token_usage ELSE 0 END)
                   + (CASE WHEN reserved_day = ?2 THEN reserved_daily_tokens ELSE 0 END)
                   + ?1 <= daily_token_limit)",
    )
    .bind(estimate)
    .bind(day)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;

    if result.rows_affected() == 1 {
        return Ok(ReserveOutcome::Reserved);
    }

    // The UPDATE matched no row: the key is missing/disabled, or one of the
    // quota predicates failed. Read the row to report the actionable reason.
    let Some(row) = get(pool, id).await? else {
        return Ok(ReserveOutcome::Disabled);
    };
    if row.enabled == 0 {
        return Ok(ReserveOutcome::Disabled);
    }
    let token_limit = row.token_limit.unwrap_or(0);
    if token_limit > 0
        && row
            .token_usage
            .saturating_add(row.reserved_tokens)
            .saturating_add(estimate)
            > token_limit
    {
        return Ok(ReserveOutcome::TokenLimit);
    }
    let daily_limit = row.daily_token_limit.unwrap_or(0);
    let daily_usage = if row.daily_usage_day == day {
        row.daily_token_usage
    } else {
        0
    };
    let daily_reserved = if row.reserved_day == day {
        row.reserved_daily_tokens
    } else {
        0
    };
    if daily_limit > 0
        && daily_usage
            .saturating_add(daily_reserved)
            .saturating_add(estimate)
            > daily_limit
    {
        return Ok(ReserveOutcome::DailyLimit);
    }

    // No predicate explains the miss (for example the row changed between the
    // UPDATE and this read), so fail closed without reserving.
    Ok(ReserveOutcome::TokenLimit)
}

/// Release a reservation. The daily counter is only decremented when the
/// reservation still belongs to `day`; both counters never go below zero.
/// Accepts any executor so settlement can share the usage-log transaction.
pub async fn release_reservation<'e, E>(
    executor: E,
    id: i64,
    tokens: i64,
    day: i64,
) -> sqlx::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    if tokens <= 0 {
        return Ok(());
    }
    sqlx::query(
        "UPDATE api_keys SET
            reserved_tokens = MAX(0, reserved_tokens - ?1),
            reserved_daily_tokens = CASE WHEN reserved_day = ?2
                THEN MAX(0, reserved_daily_tokens - ?1)
                ELSE reserved_daily_tokens END
         WHERE id = ?3",
    )
    .bind(tokens)
    .bind(day)
    .bind(id)
    .execute(executor)
    .await?;
    Ok(())
}

/// Drop every reservation. Called once at startup: reservations are in-memory
/// admission state for a single process, so a restart means no request is
/// in flight and every counter must start at zero.
pub async fn reset_reservations(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE api_keys SET
            reserved_tokens = 0,
            reserved_daily_tokens = 0,
            reserved_day = 0,
            reserved_at = NULL",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Clear reservations older than `cutoff_now_ms`. Covers crashes, cancelled
/// streams and any path that failed to settle; returns the number of keys reset.
pub async fn clear_stale_reservations(pool: &SqlitePool, cutoff_now_ms: i64) -> sqlx::Result<u64> {
    let result = sqlx::query(
        "UPDATE api_keys SET
            reserved_tokens = 0,
            reserved_daily_tokens = 0,
            reserved_day = 0,
            reserved_at = NULL
         WHERE reserved_at IS NOT NULL AND reserved_at < ?",
    )
    .bind(cutoff_now_ms)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM api_keys WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Normalize a JSON array of channel IDs from the admin request, removing
/// duplicates. Non-positive IDs are rejected rather than dropped: an empty list
/// means "no channel restriction", so silently filtering a bad ID would widen
/// the key's scope instead of reporting the mistake.
pub fn parse_channel_ids(value: &[i64]) -> Result<Vec<i64>, i64> {
    let mut ids = Vec::new();
    for id in value {
        if *id <= 0 {
            return Err(*id);
        }
        if !ids.contains(id) {
            ids.push(*id);
        }
    }
    Ok(ids)
}

/// Normalize model restrictions: trim, deduplicate, drop empties. `*` is kept
/// as an explicit wildcard (equivalent to an empty list).
pub fn parse_model_names<I, S>(values: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut models = Vec::new();
    for value in values {
        let value = value.as_ref().trim();
        if !value.is_empty() && !models.iter().any(|item| item == value) {
            models.push(value.to_string());
        }
    }
    models
}
