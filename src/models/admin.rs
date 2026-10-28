use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Admin {
    pub id: i64,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub created_at: i64,
}

pub async fn count(pool: &SqlitePool) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM admin")
        .fetch_one(pool)
        .await
}

pub async fn find_by_username(pool: &SqlitePool, username: &str) -> sqlx::Result<Option<Admin>> {
    sqlx::query_as("SELECT id, username, password_hash, created_at FROM admin WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await
}

pub async fn get(pool: &SqlitePool) -> sqlx::Result<Option<Admin>> {
    sqlx::query_as("SELECT id, username, password_hash, created_at FROM admin WHERE id = 1")
        .fetch_optional(pool)
        .await
}

pub async fn create(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
    created_at: i64,
) -> sqlx::Result<i64> {
    let result = sqlx::query(
        "INSERT INTO admin (id, username, password_hash, created_at) VALUES (1, ?, ?, ?)",
    )
    .bind(username)
    .bind(password_hash)
    .bind(created_at)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

pub async fn update_password(
    pool: &SqlitePool,
    admin_id: i64,
    password_hash: &str,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE admin SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(admin_id)
        .execute(pool)
        .await?;
    Ok(())
}
