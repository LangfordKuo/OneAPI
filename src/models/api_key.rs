use sqlx::{FromRow, SqlitePool};

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
    pub created_at: i64,
}

const SELECT_COLUMNS: &str =
    "id, name, key_hash, key_prefix, enabled, expires_at, last_used_at, created_at";

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

pub async fn create(
    pool: &SqlitePool,
    name: &str,
    key_hash: &str,
    key_prefix: &str,
    enabled: i64,
    expires_at: Option<i64>,
    created_at: i64,
) -> sqlx::Result<ApiKey> {
    let result = sqlx::query(
        "INSERT INTO api_keys (name, key_hash, key_prefix, enabled, expires_at, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(key_hash)
    .bind(key_prefix)
    .bind(enabled)
    .bind(expires_at)
    .bind(created_at)
    .execute(pool)
    .await?;

    get(pool, result.last_insert_rowid())
        .await?
        .ok_or(sqlx::Error::RowNotFound)
}

pub async fn update(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    enabled: i64,
    expires_at: Option<i64>,
) -> sqlx::Result<bool> {
    let result =
        sqlx::query("UPDATE api_keys SET name = ?, enabled = ?, expires_at = ? WHERE id = ?")
            .bind(name)
            .bind(enabled)
            .bind(expires_at)
            .bind(id)
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM api_keys WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}
