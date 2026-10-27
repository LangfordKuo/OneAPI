use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub api_key_enc: String,
    pub auth_header: String,
    pub models: String,
    pub model_mapping: String,
    pub extra_headers: String,
    pub proxy_url: Option<String>,
    pub priority: i64,
    pub weight: i64,
    pub timeout_ms: Option<i64>,
    pub enabled: i64,
    pub last_error: Option<String>,
    pub last_error_at: Option<i64>,
    pub disabled_until: Option<i64>,
    pub consecutive_failures: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct ChannelWrite {
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub api_key_enc: String,
    pub auth_header: String,
    pub models: String,
    pub model_mapping: String,
    pub extra_headers: String,
    pub proxy_url: Option<String>,
    pub priority: i64,
    pub weight: i64,
    pub timeout_ms: Option<i64>,
    pub enabled: i64,
}

const SELECT_COLUMNS: &str = "id, name, kind, base_url, api_key_enc, auth_header, models,
    model_mapping, extra_headers, proxy_url, priority, weight, timeout_ms, enabled,
    last_error, last_error_at, disabled_until, consecutive_failures, created_at, updated_at";

pub async fn list(pool: &SqlitePool) -> sqlx::Result<Vec<Channel>> {
    let query = format!("SELECT {SELECT_COLUMNS} FROM channels ORDER BY priority DESC, id ASC");
    sqlx::query_as(&query).fetch_all(pool).await
}

pub async fn get(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Channel>> {
    let query = format!("SELECT {SELECT_COLUMNS} FROM channels WHERE id = ?");
    sqlx::query_as(&query).bind(id).fetch_optional(pool).await
}

pub async fn create(pool: &SqlitePool, input: &ChannelWrite, now: i64) -> sqlx::Result<Channel> {
    let result = sqlx::query(
        "INSERT INTO channels
         (name, kind, base_url, api_key_enc, auth_header, models, model_mapping, extra_headers,
          proxy_url, priority, weight, timeout_ms, enabled, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&input.name)
    .bind(&input.kind)
    .bind(&input.base_url)
    .bind(&input.api_key_enc)
    .bind(&input.auth_header)
    .bind(&input.models)
    .bind(&input.model_mapping)
    .bind(&input.extra_headers)
    .bind(&input.proxy_url)
    .bind(input.priority)
    .bind(input.weight)
    .bind(input.timeout_ms)
    .bind(input.enabled)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    get(pool, result.last_insert_rowid())
        .await?
        .ok_or(sqlx::Error::RowNotFound)
}

pub async fn update(
    pool: &SqlitePool,
    id: i64,
    input: &ChannelWrite,
    now: i64,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE channels SET
            name = ?, kind = ?, base_url = ?, api_key_enc = ?, auth_header = ?, models = ?,
            model_mapping = ?, extra_headers = ?, proxy_url = ?, priority = ?, weight = ?,
            timeout_ms = ?, enabled = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(&input.name)
    .bind(&input.kind)
    .bind(&input.base_url)
    .bind(&input.api_key_enc)
    .bind(&input.auth_header)
    .bind(&input.models)
    .bind(&input.model_mapping)
    .bind(&input.extra_headers)
    .bind(&input.proxy_url)
    .bind(input.priority)
    .bind(input.weight)
    .bind(input.timeout_ms)
    .bind(input.enabled)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn delete(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM channels WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Record a failed attempt. Once a channel has failed twice in a row it enters
/// cooldown; each further consecutive failure doubles the previous cooldown,
/// capped at 30 minutes. A single failure only records the error so a transient
/// upstream hiccup does not remove a channel from rotation.
pub async fn mark_failure(pool: &SqlitePool, id: i64, error: &str, now: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE channels SET
            consecutive_failures = consecutive_failures + 1,
            last_error = ?,
            last_error_at = ?,
            updated_at = ?,
            disabled_until = CASE
                WHEN consecutive_failures + 1 >= 2 THEN
                    ? + min(1800000, 60000 * (1 << min(consecutive_failures - 1, 5)))
                ELSE disabled_until
            END
         WHERE id = ?",
    )
    .bind(error)
    .bind(now)
    .bind(now)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Clear failure state after a successful attempt. Returns whether a row was
/// changed so callers can skip cache invalidation for healthy channels.
pub async fn mark_success(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE channels SET
            consecutive_failures = 0,
            last_error = NULL,
            last_error_at = NULL,
            disabled_until = NULL
         WHERE id = ?
           AND (consecutive_failures <> 0 OR last_error IS NOT NULL OR disabled_until IS NOT NULL)",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("memory database");
        sqlx::query(
            "CREATE TABLE channels (
                id INTEGER PRIMARY KEY,
                last_error TEXT,
                last_error_at INTEGER,
                disabled_until INTEGER,
                consecutive_failures INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("schema");
        sqlx::query("INSERT INTO channels (id, updated_at) VALUES (1, 0)")
            .execute(&pool)
            .await
            .expect("seed");
        pool
    }

    async fn state(pool: &SqlitePool) -> (i64, Option<i64>, Option<String>) {
        sqlx::query_as(
            "SELECT consecutive_failures, disabled_until, last_error FROM channels WHERE id = 1",
        )
        .fetch_one(pool)
        .await
        .expect("row")
    }

    #[tokio::test]
    async fn first_failure_records_error_without_cooldown() {
        let pool = pool().await;
        mark_failure(&pool, 1, "boom", 1_000).await.expect("mark");

        let (failures, disabled_until, last_error) = state(&pool).await;
        assert_eq!(failures, 1);
        assert_eq!(disabled_until, None);
        assert_eq!(last_error.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn consecutive_failures_back_off_exponentially() {
        let pool = pool().await;
        let now = 10_000;

        mark_failure(&pool, 1, "boom", now).await.expect("first");
        assert_eq!(state(&pool).await.1, None);

        let expected = [
            60_000, 120_000, 240_000, 480_000, 960_000, 1_800_000, 1_800_000,
        ];
        for cooldown in expected {
            mark_failure(&pool, 1, "boom", now).await.expect("mark");
            assert_eq!(state(&pool).await.1, Some(now + cooldown));
        }
    }

    #[tokio::test]
    async fn success_clears_failure_state() {
        let pool = pool().await;
        mark_failure(&pool, 1, "boom", 1_000).await.expect("first");
        mark_failure(&pool, 1, "boom", 1_000).await.expect("second");

        assert!(mark_success(&pool, 1).await.expect("clear"));
        assert!(!mark_success(&pool, 1).await.expect("healthy"));

        let (failures, disabled_until, last_error) = state(&pool).await;
        assert_eq!(failures, 0);
        assert_eq!(disabled_until, None);
        assert_eq!(last_error, None);
    }
}
