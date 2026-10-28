use std::{str::FromStr, time::Duration};

use anyhow::Context;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};

use crate::config::Config;

pub async fn connect(config: &Config) -> anyhow::Result<SqlitePool> {
    let mut options = match &config.database_url {
        Some(url) => SqliteConnectOptions::from_str(url)
            .context("ONEAPI_DATABASE_URL is not a valid SQLite URL")?,
        None => SqliteConnectOptions::new().filename(&config.database_path),
    };

    options = options
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(config.database_pool_size)
        .connect_with(options)
        .await
        .context("failed to connect to SQLite")?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("failed to run database migrations")?;

    crate::models::channel::backfill_protocol_configs(&pool)
        .await
        .context("failed to backfill per-protocol channel configs")?;

    Ok(pool)
}
