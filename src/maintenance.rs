use std::time::Duration;

use crate::{
    models::{now_ms, setting},
    state::AppState,
};

const DAY_MS: i64 = 86_400_000;
const CLEANUP_INTERVAL_SECONDS: u64 = 6 * 60 * 60;
/// Cap each delete transaction so the SQLite write lock is released regularly.
/// A single unbounded DELETE can block gateway usage writes past the 5 second
/// busy timeout on large tables.
const DELETE_BATCH: i64 = 5_000;

const DELETE_SESSIONS: &str = "DELETE FROM sessions WHERE rowid IN
    (SELECT rowid FROM sessions WHERE expires_at < ? LIMIT ?)";
const DELETE_USAGE_LOGS: &str = "DELETE FROM usage_logs WHERE rowid IN
    (SELECT rowid FROM usage_logs WHERE created_at < ? LIMIT ?)";
const DELETE_REQUEST_DETAILS: &str = "DELETE FROM request_details WHERE rowid IN
    (SELECT rowid FROM request_details WHERE created_at < ? LIMIT ?)";
const DELETE_RESPONSES: &str = "DELETE FROM responses WHERE rowid IN
    (SELECT rowid FROM responses WHERE created_at < ? LIMIT ?)";

#[derive(Debug, Default)]
pub struct CleanupStats {
    pub usage_logs: u64,
    pub request_details: u64,
    pub responses: u64,
    pub sessions: u64,
}

impl CleanupStats {
    fn changed(&self) -> bool {
        self.usage_logs > 0 || self.request_details > 0 || self.responses > 0 || self.sessions > 0
    }
}

pub async fn cleanup_once(state: &AppState) -> anyhow::Result<CleanupStats> {
    if !state.config.data_dir.exists() {
        return Ok(CleanupStats::default());
    }

    let usage_days = setting::get_i64(&state.db, "usage_log_retention_days", 30).await?;
    let request_detail_days =
        setting::get_i64(&state.db, "request_detail_retention_days", 7).await?;
    let response_days = setting::get_i64(&state.db, "response_retention_days", 7).await?;
    let now = now_ms();

    let usage_cutoff = (usage_days > 0).then(|| now - usage_days.saturating_mul(DAY_MS));
    // Request bodies are far more sensitive than usage metadata, so their
    // retention is independent of the usage log window. `0` disables the
    // time-based sweep; rows are then removed only when the owning Key is
    // deleted (migration 0009 trigger).
    let request_detail_cutoff =
        (request_detail_days > 0).then(|| now - request_detail_days.saturating_mul(DAY_MS));
    let response_cutoff = (response_days > 0).then(|| now - response_days.saturating_mul(DAY_MS));

    let mut stats = CleanupStats {
        sessions: delete_before(&state.db, DELETE_SESSIONS, now).await?,
        ..CleanupStats::default()
    };

    if let Some(cutoff) = usage_cutoff {
        stats.usage_logs = delete_before(&state.db, DELETE_USAGE_LOGS, cutoff).await?;
    }

    if let Some(cutoff) = request_detail_cutoff {
        stats.request_details = delete_before(&state.db, DELETE_REQUEST_DETAILS, cutoff).await?;
    }

    if let Some(cutoff) = response_cutoff {
        stats.responses = delete_before(&state.db, DELETE_RESPONSES, cutoff).await?;
    }

    Ok(stats)
}

/// Delete expired rows in bounded batches, committing after each batch. Each
/// batch is its own transaction so the write lock is never held for the whole
/// table sweep.
async fn delete_before(
    pool: &sqlx::SqlitePool,
    sql: &'static str,
    cutoff: i64,
) -> anyhow::Result<u64> {
    let mut deleted = 0u64;
    loop {
        let mut tx = pool.begin().await?;
        let rows = sqlx::query(sql)
            .bind(cutoff)
            .bind(DELETE_BATCH)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;

        deleted = deleted.saturating_add(rows);
        if rows < DELETE_BATCH as u64 {
            return Ok(deleted);
        }
    }
}

/// Run cleanup immediately and then every six hours. Each retention setting is
/// read independently; a value of `0` disables that category's time-based
/// sweep (request details are then only removed with their Key), and negative
/// values are never written by the settings API.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(CLEANUP_INTERVAL_SECONDS));
        loop {
            interval.tick().await;
            match cleanup_once(&state).await {
                Ok(stats) if stats.changed() => {
                    tracing::info!(
                        usage_logs = stats.usage_logs,
                        request_details = stats.request_details,
                        responses = stats.responses,
                        sessions = stats.sessions,
                        "retention cleanup completed"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(error = %error, "retention cleanup failed");
                }
            }
        }
    });
}
