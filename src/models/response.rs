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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{insert_key, pool};

    const LIMIT: i64 = 1_000_000;

    async fn usage(pool: &SqlitePool, api_key_id: i64) -> (i64, i64) {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT bytes, rows FROM response_usage WHERE api_key_id = ?",
        )
        .bind(api_key_id)
        .fetch_optional(pool)
        .await
        .expect("read response usage")
        .unwrap_or((0, 0))
    }

    #[tokio::test]
    async fn store_counts_multibyte_payloads_in_bytes() {
        let pool = pool().await;
        let key = insert_key(&pool, "bytes", None, None).await;
        let request_json = r#"{"prompt":"hola 世界"}"#;
        let response_json = r#"{"text":"你好，世界"}"#;

        let outcome = store_with_quota(
            &pool,
            "resp-bytes",
            key,
            "test-model",
            request_json,
            response_json,
            "completed",
            1,
            LIMIT,
            LIMIT,
        )
        .await
        .expect("store response");

        assert_eq!(outcome, StoreOutcome::Stored);
        let expected = (request_json.len() + response_json.len()) as i64;
        let character_count = (request_json.chars().count() + response_json.chars().count()) as i64;
        assert_ne!(
            expected, character_count,
            "payload must contain multi-byte characters for this test to be meaningful"
        );
        assert_eq!(
            usage(&pool, key).await,
            (expected, 1),
            "usage must count UTF-8 bytes, not characters"
        );
    }

    #[tokio::test]
    async fn replacing_a_response_counts_only_the_size_difference() {
        let pool = pool().await;
        let key = insert_key(&pool, "replace", None, None).await;
        let small_request = r#"{"prompt":"hi"}"#;
        let small_response = r#"{"text":"ok"}"#;
        store_with_quota(
            &pool,
            "resp-replace",
            key,
            "test-model",
            small_request,
            small_response,
            "completed",
            1,
            LIMIT,
            LIMIT,
        )
        .await
        .expect("initial store");

        let large_request = r#"{"prompt":"a much longer prompt with 中文 content"}"#;
        let large_response = r#"{"text":"a longer response body with 中文 too"}"#;
        let outcome = store_with_quota(
            &pool,
            "resp-replace",
            key,
            "test-model",
            large_request,
            large_response,
            "completed",
            2,
            LIMIT,
            LIMIT,
        )
        .await
        .expect("replace store");

        assert_eq!(outcome, StoreOutcome::Stored);
        let expected = (large_request.len() + large_response.len()) as i64;
        assert_eq!(
            usage(&pool, key).await,
            (expected, 1),
            "a replacement must adjust bytes to the new size and keep one row"
        );
        let stored = get(&pool, "resp-replace", key)
            .await
            .expect("get response")
            .expect("stored row");
        assert_eq!(stored.request_json, large_request);
        assert_eq!(stored.response_json, large_response);
    }

    #[tokio::test]
    async fn reusing_an_id_from_another_key_is_a_conflict_without_side_effects() {
        let pool = pool().await;
        let owner = insert_key(&pool, "owner", None, None).await;
        let other = insert_key(&pool, "other", None, None).await;
        let request_json = r#"{"prompt":"owner"}"#;
        let response_json = r#"{"text":"owner response"}"#;
        store_with_quota(
            &pool,
            "resp-conflict",
            owner,
            "test-model",
            request_json,
            response_json,
            "completed",
            1,
            LIMIT,
            LIMIT,
        )
        .await
        .expect("store owner response");
        let owner_usage = usage(&pool, owner).await;

        let outcome = store_with_quota(
            &pool,
            "resp-conflict",
            other,
            "test-model",
            r#"{"prompt":"other"}"#,
            r#"{"text":"other response"}"#,
            "completed",
            2,
            LIMIT,
            LIMIT,
        )
        .await
        .expect("conflicting store");

        assert_eq!(outcome, StoreOutcome::Conflict);
        assert_eq!(
            usage(&pool, owner).await,
            owner_usage,
            "the owner's accounting must not change"
        );
        assert_eq!(
            usage(&pool, other).await,
            (0, 0),
            "the rejected key must not gain accounting"
        );
        let stored = get(&pool, "resp-conflict", owner)
            .await
            .expect("get response")
            .expect("owner row");
        assert_eq!(stored.request_json, request_json);
        assert_eq!(stored.response_json, response_json);
    }

    #[tokio::test]
    async fn per_key_quota_rejection_leaves_accounting_untouched() {
        let pool = pool().await;
        let key = insert_key(&pool, "quota", None, None).await;
        let request_json = r#"{"prompt":"0123456789"}"#;
        let response_json = r#"{"text":"0123456789"}"#;

        let outcome = store_with_quota(
            &pool,
            "resp-quota",
            key,
            "test-model",
            request_json,
            response_json,
            "completed",
            1,
            10,
            0,
        )
        .await
        .expect("quota-checked store");

        assert_eq!(outcome, StoreOutcome::QuotaExceeded);
        assert_eq!(
            usage(&pool, key).await,
            (0, 0),
            "a rejected write must not consume quota"
        );
        assert!(
            get(&pool, "resp-quota", key)
                .await
                .expect("get response")
                .is_none(),
            "a rejected write must not store a row"
        );
    }

    #[tokio::test]
    async fn deleting_a_response_releases_its_bytes() {
        let pool = pool().await;
        let key = insert_key(&pool, "delete", None, None).await;
        let request_json = r#"{"prompt":"delete me"}"#;
        let response_json = r#"{"text":"deleted"}"#;
        store_with_quota(
            &pool,
            "resp-delete",
            key,
            "test-model",
            request_json,
            response_json,
            "completed",
            1,
            LIMIT,
            LIMIT,
        )
        .await
        .expect("store response");

        assert!(
            delete(&pool, "resp-delete", key).await.expect("delete"),
            "the owner can delete its response"
        );

        assert_eq!(
            usage(&pool, key).await.0,
            0,
            "deleting must release the stored bytes"
        );
        assert!(
            get(&pool, "resp-delete", key)
                .await
                .expect("get response")
                .is_none(),
            "the response row must be gone"
        );
    }
}
