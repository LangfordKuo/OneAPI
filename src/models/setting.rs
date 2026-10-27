use sqlx::SqlitePool;

pub async fn all(pool: &SqlitePool) -> sqlx::Result<Vec<(String, String)>> {
    sqlx::query_as("SELECT key, value FROM settings ORDER BY key")
        .fetch_all(pool)
        .await
}

pub async fn get(pool: &SqlitePool, key: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await
}

pub async fn get_i64(pool: &SqlitePool, key: &str, default: i64) -> sqlx::Result<i64> {
    Ok(get(pool, key)
        .await?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(default))
}

pub async fn get_bool(pool: &SqlitePool, key: &str, default: bool) -> sqlx::Result<bool> {
    Ok(get(pool, key)
        .await?
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(default))
}

pub async fn upsert(pool: &SqlitePool, key: &str, value: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}
