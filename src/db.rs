use std::{str::FromStr, time::Duration};

use anyhow::Context;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};

use crate::{backup, config::Config};

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

    let integrity: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_all(&pool)
        .await
        .context("failed to check database integrity")?;
    if !integrity.iter().all(|row| row == "ok") {
        anyhow::bail!(
            "database integrity check failed; restore a backup from {}",
            backup::backup_dir(&config.data_dir).display()
        );
    }

    let migrator = sqlx::migrate!("./migrations");
    let has_migrations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(&pool)
    .await
    .context("failed to inspect the migration table")?;
    if has_migrations > 0 {
        let applied: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success = 1")
                .fetch_all(&pool)
                .await
                .context("failed to read applied migrations")?;
        let pending = migrator
            .iter()
            .any(|migration| !applied.contains(&migration.version));
        if pending {
            let path = backup::snapshot(&pool, &config.data_dir, "oneapi", true)
                .await
                .context("failed to back up the database before migrations")?;
            tracing::info!(
                backup = %path.display(),
                "database backed up before applying migrations"
            );
        }
    }

    migrator
        .run(&pool)
        .await
        .context("failed to run database migrations")?;

    crate::models::channel::backfill_protocol_configs(&pool)
        .await
        .context("failed to backfill per-protocol channel configs")?;

    Ok(pool)
}
