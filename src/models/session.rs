use sqlx::SqlitePool;

use super::admin::Admin;

pub async fn create(
    pool: &SqlitePool,
    token_hash: &str,
    admin_id: i64,
    created_at: i64,
    expires_at: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO sessions (token_hash, admin_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(token_hash)
    .bind(admin_id)
    .bind(created_at)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn find_admin(
    pool: &SqlitePool,
    token_hash: &str,
    now: i64,
) -> sqlx::Result<Option<Admin>> {
    sqlx::query_as(
        "SELECT a.id, a.username, a.password_hash, a.created_at
         FROM sessions s
         JOIN admin a ON a.id = s.admin_id
         WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(token_hash)
    .bind(now)
    .fetch_optional(pool)
    .await
}

pub async fn delete(pool: &SqlitePool, token_hash: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_all_for_admin(pool: &SqlitePool, admin_id: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE admin_id = ?")
        .bind(admin_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_expired(pool: &SqlitePool, now: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}
