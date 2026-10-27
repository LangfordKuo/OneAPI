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

/// Gateway-facing settings snapshot. Load once per request so a single request
/// observes a consistent view even if the settings are edited concurrently.
#[derive(Debug, Clone, Copy)]
pub struct GatewayDefaults {
    pub default_max_tokens: u32,
    pub nonstream_timeout_ms: u64,
    pub stream_idle_timeout_ms: u64,
    pub store_request_bodies: bool,
}

impl GatewayDefaults {
    pub async fn load(pool: &SqlitePool) -> sqlx::Result<Self> {
        let max_tokens = get_i64(pool, "default_max_tokens", 4096).await?;
        let nonstream = get_i64(pool, "nonstream_timeout_seconds", 600).await?;
        let stream_idle = get_i64(pool, "stream_idle_timeout_seconds", 120).await?;

        Ok(Self {
            default_max_tokens: u32::try_from(max_tokens).unwrap_or(4096).max(1),
            nonstream_timeout_ms: u64::try_from(nonstream).unwrap_or(600).max(1) * 1000,
            stream_idle_timeout_ms: u64::try_from(stream_idle).unwrap_or(120).max(1) * 1000,
            store_request_bodies: get_bool(pool, "store_request_bodies", false).await?,
        })
    }
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
