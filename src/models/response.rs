use std::collections::HashMap;

use sqlx::{FromRow, SqlitePool};

/// Maximum size of one stored request or response payload. Larger payloads are
/// rejected before the transaction starts so a single row cannot exhaust the
/// per-key or global storage budget.
pub const MAX_STORED_BYTES: usize = 10 * 1024 * 1024;

/// Result of a quota-checked persistence attempt. `Conflict` mirrors the
/// existing L1 behaviour: the id belongs to a different API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOutcome {
    Stored,
    Conflict,
    QuotaExceeded,
    TooLarge,
}

#[allow(dead_code)]
#[derive(Debug, Clone, FromRow)]
pub struct StoredResponse {
    pub id: String,
    pub api_key_id: i64,
    pub model: String,
    pub request_json: String,
    pub response_json: String,
    pub status: String,
    pub created_at: i64,
}

/// Persist a Responses object while enforcing per-key and global quotas.
///
/// The ownership lookup, quota check, upsert and `response_usage` update all
/// run in one transaction. Callers should serialize invocations (the gateway
/// holds `AppState::response_store_lock`) so two concurrent writes cannot both
/// pass the same quota check.
///
/// A limit of `0` means unlimited. `old` bytes are the size of an existing
/// row owned by `api_key_id`; replacing a row therefore only counts the
/// difference against the quotas.
#[allow(clippy::too_many_arguments)]
pub async fn store_with_quota(
    pool: &SqlitePool,
    id: &str,
    api_key_id: i64,
    model: &str,
    request_json: &str,
    response_json: &str,
    status: &str,
    created_at: i64,
    per_key_limit: i64,
    total_limit: i64,
) -> sqlx::Result<StoreOutcome> {
    if request_json.len() > MAX_STORED_BYTES || response_json.len() > MAX_STORED_BYTES {
        return Ok(StoreOutcome::TooLarge);
    }

    let new_bytes =
        i64::try_from(request_json.len().saturating_add(response_json.len())).unwrap_or(i64::MAX);

    let mut tx = pool.begin().await?;

    // `length()` counts characters for TEXT and bytes for BLOB. The quotas are
    // enforced in bytes (Rust `.len()`), so the column is cast before measuring;
    // otherwise multi-byte content would be under-counted and could exceed the
    // configured budget.
    let old_row: Option<i64> = sqlx::query_scalar(
        "SELECT length(CAST(COALESCE(request_json, '') AS BLOB))
              + length(CAST(COALESCE(response_json, '') AS BLOB))
         FROM responses WHERE id = ? AND api_key_id = ?",
    )
    .bind(id)
    .bind(api_key_id)
    .fetch_optional(&mut *tx)
    .await?;
    let old_exists = old_row.is_some();
    let old_bytes = old_row.unwrap_or(0);

    let key_bytes: i64 =
        sqlx::query_scalar("SELECT bytes FROM response_usage WHERE api_key_id = ?")
            .bind(api_key_id)
            .fetch_optional(&mut *tx)
            .await?
            .unwrap_or(0);
    let total_bytes: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(bytes), 0) FROM response_usage")
        .fetch_one(&mut *tx)
        .await?;

    let over_key = per_key_limit > 0
        && key_bytes
            .saturating_sub(old_bytes)
            .saturating_add(new_bytes)
            > per_key_limit;
    let over_total = total_limit > 0
        && total_bytes
            .saturating_sub(old_bytes)
            .saturating_add(new_bytes)
            > total_limit;
    if over_key || over_total {
        tx.rollback().await?;
        return Ok(StoreOutcome::QuotaExceeded);
    }

    let result = sqlx::query(
        "INSERT INTO responses
         (id, api_key_id, model, request_json, response_json, status, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            model = excluded.model,
            request_json = excluded.request_json,
            response_json = excluded.response_json,
            status = excluded.status,
            created_at = excluded.created_at
         WHERE responses.api_key_id = excluded.api_key_id",
    )
    .bind(id)
    .bind(api_key_id)
    .bind(model)
    .bind(request_json)
    .bind(response_json)
    .bind(status)
    .bind(created_at)
    .execute(&mut *tx)
    .await?;

    if result.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(StoreOutcome::Conflict);
    }

    // Signed delta: replacing a row adjusts the accounting by the difference.
    // The insert arm clamps at zero, the conflict arm applies the raw delta and
    // then clamps, so deleting/replacing content can never produce negative
    // counters.
    let bytes_delta = new_bytes - old_bytes;
    let rows_delta: i64 = if old_exists { 0 } else { 1 };
    sqlx::query(
        "INSERT INTO response_usage (api_key_id, bytes, rows)
         VALUES (?, MAX(0, ?), MAX(0, ?))
         ON CONFLICT(api_key_id) DO UPDATE SET
            bytes = MAX(0, response_usage.bytes + ?),
            rows = MAX(0, response_usage.rows + ?)",
    )
    .bind(api_key_id)
    .bind(bytes_delta)
    .bind(rows_delta)
    .bind(bytes_delta)
    .bind(rows_delta)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(StoreOutcome::Stored)
}

/// Delete rows older than `cutoff` in bounded batches and keep
/// `response_usage` in sync from the deleted rows' actual sizes.
///
/// Each batch is one transaction: `DELETE ... RETURNING` reports the owner and
/// byte size of every removed row, which are aggregated per key before the
/// counters are decremented. SQLite 3.35+ (bundled with sqlx) provides
/// `RETURNING`; a build without it surfaces the SQL error instead of silently
/// falling back to an unaccounted delete.
pub async fn delete_before(pool: &SqlitePool, cutoff: i64, batch: i64) -> sqlx::Result<usize> {
    let mut deleted = 0usize;
    loop {
        let mut tx = pool.begin().await?;
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "DELETE FROM responses
             WHERE rowid IN
                 (SELECT rowid FROM responses WHERE created_at < ? LIMIT ?)
             RETURNING api_key_id,
                 length(CAST(COALESCE(request_json, '') AS BLOB))
                     + length(CAST(COALESCE(response_json, '') AS BLOB))",
        )
        .bind(cutoff)
        .bind(batch)
        .fetch_all(&mut *tx)
        .await?;

        if rows.is_empty() {
            tx.commit().await?;
            return Ok(deleted);
        }

        let mut per_key: HashMap<i64, (i64, i64)> = HashMap::new();
        for (api_key_id, bytes) in &rows {
            let entry = per_key.entry(*api_key_id).or_insert((0, 0));
            entry.0 = entry.0.saturating_add(*bytes);
            entry.1 = entry.1.saturating_add(1);
        }

        for (api_key_id, (bytes, count)) in per_key {
            sqlx::query(
                "UPDATE response_usage
                 SET bytes = MAX(0, bytes - ?), rows = MAX(0, rows - ?)
                 WHERE api_key_id = ?",
            )
            .bind(bytes)
            .bind(count)
            .bind(api_key_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        deleted += rows.len();
        if (rows.len() as i64) < batch {
            return Ok(deleted);
        }
    }
}

pub async fn get(
    pool: &SqlitePool,
    id: &str,
    api_key_id: i64,
) -> sqlx::Result<Option<StoredResponse>> {
    sqlx::query_as(
        "SELECT id, api_key_id, model, request_json, response_json, status, created_at
         FROM responses WHERE id = ? AND api_key_id = ?",
    )
    .bind(id)
    .bind(api_key_id)
    .fetch_optional(pool)
    .await
}

/// Delete one response and keep the owner's storage counters in sync. Without
/// the decrement, an explicit delete would leave `response_usage` inflated and
/// eventually reject writes that still fit.
pub async fn delete(pool: &SqlitePool, id: &str, api_key_id: i64) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;

    let bytes: Option<i64> = sqlx::query_scalar(
        "DELETE FROM responses WHERE id = ? AND api_key_id = ?
         RETURNING length(CAST(COALESCE(request_json, '') AS BLOB))
             + length(CAST(COALESCE(response_json, '') AS BLOB))",
    )
    .bind(id)
    .bind(api_key_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(bytes) = bytes else {
        tx.rollback().await?;
        return Ok(false);
    };

    sqlx::query(
        "UPDATE response_usage
         SET bytes = MAX(0, bytes - ?), rows = MAX(0, rows - 1)
         WHERE api_key_id = ?",
    )
    .bind(bytes)
    .bind(api_key_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}
