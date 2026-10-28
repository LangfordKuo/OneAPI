use sqlx::{Executor, FromRow, Sqlite, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct ApiKey {
    pub id: i64,
    pub name: String,
    #[allow(dead_code)]
    pub key_hash: String,
    pub key_prefix: String,
    pub enabled: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub allowed_channel_ids: String,
    pub allowed_models: String,
    pub token_limit: Option<i64>,
    pub token_usage: i64,
    pub created_at: i64,
}

impl ApiKey {
    /// Empty means "all channels".
    pub fn channel_ids(&self) -> Vec<i64> {
        serde_json::from_str(&self.allowed_channel_ids).unwrap_or_default()
    }

    /// Empty means "all models"; `*` also matches everything.
    pub fn model_names(&self) -> Vec<String> {
        serde_json::from_str(&self.allowed_models).unwrap_or_default()
    }

    pub fn allows_model(&self, model: &str) -> bool {
        let models = self.model_names();
        models.is_empty() || models.iter().any(|item| item == "*" || item == model)
    }

    pub fn token_limit_reached(&self) -> bool {
        self.token_limit
            .is_some_and(|limit| limit > 0 && self.token_usage >= limit)
    }
}

#[derive(Debug)]
pub struct ApiKeyCreate<'a> {
    pub name: &'a str,
    pub key_hash: &'a str,
    pub key_prefix: &'a str,
    pub enabled: i64,
    pub expires_at: Option<i64>,
    pub allowed_channel_ids: &'a str,
    pub allowed_models: &'a str,
    pub token_limit: Option<i64>,
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
}

const SELECT_COLUMNS: &str = "id, name, key_hash, key_prefix, enabled, expires_at, last_used_at,
    allowed_channel_ids, allowed_models, token_limit, token_usage, created_at";

pub async fn list(pool: &SqlitePool) -> sqlx::Result<Vec<ApiKey>> {
    let query = format!("SELECT {SELECT_COLUMNS} FROM api_keys ORDER BY id DESC");
    sqlx::query_as(&query).fetch_all(pool).await
}

pub async fn get(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<ApiKey>> {
    let query = format!("SELECT {SELECT_COLUMNS} FROM api_keys WHERE id = ?");
    sqlx::query_as(&query).bind(id).fetch_optional(pool).await
}

pub async fn find_by_hash(pool: &SqlitePool, key_hash: &str) -> sqlx::Result<Option<ApiKey>> {
    let query = format!("SELECT {SELECT_COLUMNS} FROM api_keys WHERE key_hash = ?");
    sqlx::query_as(&query)
        .bind(key_hash)
        .fetch_optional(pool)
        .await
}

pub async fn touch_last_used(pool: &SqlitePool, id: i64, now: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Backwards-compatible creator with no restrictions, used by tests.
#[cfg(test)]
pub async fn create(
    pool: &SqlitePool,
    name: &str,
    key_hash: &str,
    key_prefix: &str,
    enabled: i64,
    expires_at: Option<i64>,
    created_at: i64,
) -> sqlx::Result<ApiKey> {
    create_with_limits(
        pool,
        &ApiKeyCreate {
            name,
            key_hash,
            key_prefix,
            enabled,
            expires_at,
            allowed_channel_ids: "[]",
            allowed_models: "[]",
            token_limit: None,
            created_at,
        },
    )
    .await
}

pub async fn create_with_limits(
    pool: &SqlitePool,
    input: &ApiKeyCreate<'_>,
) -> sqlx::Result<ApiKey> {
    let result = sqlx::query(
        "INSERT INTO api_keys
         (name, key_hash, key_prefix, enabled, expires_at, allowed_channel_ids, allowed_models,
          token_limit, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(input.name)
    .bind(input.key_hash)
    .bind(input.key_prefix)
    .bind(input.enabled)
    .bind(input.expires_at)
    .bind(input.allowed_channel_ids)
    .bind(input.allowed_models)
    .bind(input.token_limit)
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
            token_limit = ?
         WHERE id = ?",
    )
    .bind(input.name)
    .bind(input.enabled)
    .bind(input.expires_at)
    .bind(input.allowed_channel_ids)
    .bind(input.allowed_models)
    .bind(input.token_limit)
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

/// Add token usage and auto-disable the key when it reaches its limit. Runs
/// inside the usage-log transaction so the counter and the log stay consistent.
pub async fn add_usage<'e, E>(executor: E, id: i64, tokens: i64) -> sqlx::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    if tokens <= 0 {
        return Ok(());
    }

    sqlx::query(
        "UPDATE api_keys SET
            token_usage = token_usage + ?,
            enabled = CASE
                WHEN token_limit IS NOT NULL AND token_limit > 0
                     AND token_usage + ? >= token_limit THEN 0
                ELSE enabled
            END
         WHERE id = ?",
    )
    .bind(tokens)
    .bind(tokens)
    .bind(id)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM api_keys WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Parse a JSON array of channel IDs from the admin request, keeping only
/// positive values and removing duplicates.
pub fn parse_channel_ids(value: &[i64]) -> Vec<i64> {
    let mut ids = Vec::new();
    for id in value {
        if *id > 0 && !ids.contains(id) {
            ids.push(*id);
        }
    }
    ids
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
