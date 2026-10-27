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

pub async fn mark_failure(
    pool: &SqlitePool,
    id: i64,
    error: &str,
    now: i64,
    cooldown_ms: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE channels SET
            consecutive_failures = consecutive_failures + 1,
            last_error = ?,
            last_error_at = ?,
            updated_at = ?,
            disabled_until = CASE
                WHEN consecutive_failures >= 2 THEN ?
                ELSE disabled_until
            END
         WHERE id = ?",
    )
    .bind(error)
    .bind(now)
    .bind(now)
    .bind(now + cooldown_ms)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_success(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE channels SET
            consecutive_failures = 0,
            last_error = NULL,
            last_error_at = NULL,
            disabled_until = NULL
         WHERE id = ?",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}
