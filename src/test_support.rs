//! Shared helpers for unit tests.
//!
//! This module is compiled only for tests (`mod test_support` in `main.rs` is
//! `#[cfg(test)]`). It gives every test module the same migrated in-memory
//! SQLite database, so quota and ownership tests exercise the real schema
//! instead of hand-written tables.
#![cfg(test)]

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

/// Create an empty in-memory database and apply every migration.
///
/// A single connection is mandatory: SQLite in-memory databases are scoped to
/// one connection, so a second pooled connection would see an empty schema.
pub async fn pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("create in-memory sqlite pool");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");
    pool
}

/// Insert an enabled API key with optional lifetime/daily token limits and
/// return its row id.
pub async fn insert_key(
    pool: &SqlitePool,
    name: &str,
    token_limit: Option<i64>,
    daily_limit: Option<i64>,
) -> i64 {
    let key_hash = format!("test-hash-{}", uuid::Uuid::new_v4());
    let result = sqlx::query(
        "INSERT INTO api_keys
         (name, key_hash, key_prefix, enabled, allowed_channel_ids, allowed_models,
          token_limit, daily_token_limit, created_at)
         VALUES (?, ?, ?, 1, '[]', '[]', ?, ?, 0)",
    )
    .bind(name)
    .bind(&key_hash)
    .bind("sk-test-")
    .bind(token_limit)
    .bind(daily_limit)
    .execute(pool)
    .await
    .expect("insert api key");
    result.last_insert_rowid()
}
