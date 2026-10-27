use std::time::Duration;

use crate::{
    models::{now_ms, setting},
    state::AppState,
};

const DAY_MS: i64 = 86_400_000;
const CLEANUP_INTERVAL_SECONDS: u64 = 6 * 60 * 60;

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
    let response_days = setting::get_i64(&state.db, "response_retention_days", 7).await?;
    let now = now_ms();

    let usage_cutoff = (usage_days > 0).then(|| now - usage_days.saturating_mul(DAY_MS));
    let response_cutoff = (response_days > 0).then(|| now - response_days.saturating_mul(DAY_MS));

    let mut stats = CleanupStats::default();
    let mut tx = state.db.begin().await?;

    stats.sessions = sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    if let Some(cutoff) = usage_cutoff {
        stats.usage_logs = sqlx::query("DELETE FROM usage_logs WHERE created_at < ?")
            .bind(cutoff)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        stats.request_details = sqlx::query("DELETE FROM request_details WHERE created_at < ?")
            .bind(cutoff)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }

    if let Some(cutoff) = response_cutoff {
        stats.responses = sqlx::query("DELETE FROM responses WHERE created_at < ?")
            .bind(cutoff)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }

    tx.commit().await?;

    Ok(stats)
}

/// Run cleanup immediately and then every six hours. A retention value of `0`
/// disables that category; negative values are never written by the settings API.
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::TempDir;

    use crate::{config::Config, db, models, security, state::AppState};

    use super::*;

    async fn test_state() -> (TempDir, AppState) {
        let temp = TempDir::new().expect("temp dir");
        let data_dir = temp.path().to_path_buf();
        let config = Config {
            addr: "127.0.0.1:0".parse().expect("address"),
            data_dir: data_dir.clone(),
            database_url: None,
            database_path: data_dir.join("test.db"),
            log_level: "error".to_string(),
            cookie_secure: false,
            trust_proxy: false,
        };
        let db = db::connect(&config).await.expect("database");
        let crypto =
            Arc::new(security::Crypto::load_or_create(&config.data_dir).expect("master key"));
        (
            temp,
            AppState {
                config: Arc::new(config),
                db,
                crypto,
                live: std::sync::Arc::new(crate::metrics::LiveMetrics::default()),
                updates: std::sync::Arc::new(crate::state::UpdateState::default()),
                login_limiter: std::sync::Arc::new(crate::admin::LoginLimiter::default()),
                clients: std::sync::Arc::new(crate::upstream::ClientCache::default()),
                cache: std::sync::Arc::new(crate::state::RuntimeCache::default()),
                usage_writer: None,
                started_at: chrono::Utc::now(),
            },
        )
    }

    async fn seed_key(state: &AppState) -> i64 {
        let key = security::generate_api_key();
        let hash = security::api_key_hash(&key);
        models::api_key::create(&state.db, "test", &hash, "sk-test", 1, None, now_ms())
            .await
            .expect("api key")
            .id
    }

    async fn insert_usage(state: &AppState, request_id: &str, created_at: i64) {
        sqlx::query(
            "INSERT INTO usage_logs
             (request_id, source, model, inbound_protocol, upstream_protocol, stream,
              status_code, attempts, latency_ms, created_at)
             VALUES (?, 'gateway', 'gpt', 'chat', 'chat', 0, 200, 1, 10, ?)",
        )
        .bind(request_id)
        .bind(created_at)
        .execute(&state.db)
        .await
        .expect("usage log");
    }

    async fn insert_detail(state: &AppState, request_id: &str, created_at: i64) {
        sqlx::query(
            "INSERT INTO request_details (request_id, request_body, response_body, created_at)
             VALUES (?, '{}', '{}', ?)",
        )
        .bind(request_id)
        .bind(created_at)
        .execute(&state.db)
        .await
        .expect("request detail");
    }

    async fn insert_response(state: &AppState, id: &str, api_key_id: i64, created_at: i64) {
        sqlx::query(
            "INSERT INTO responses
             (id, api_key_id, model, request_json, response_json, status, created_at)
             VALUES (?, ?, 'gpt', '{}', '{}', 'completed', ?)",
        )
        .bind(id)
        .bind(api_key_id)
        .bind(created_at)
        .execute(&state.db)
        .await
        .expect("response");
    }

    #[tokio::test]
    async fn removes_only_entries_older_than_retention() {
        let (_temp, state) = test_state().await;
        let key_id = seed_key(&state).await;
        let now = now_ms();
        let old = now - 2 * DAY_MS;

        setting::upsert(&state.db, "usage_log_retention_days", "1")
            .await
            .expect("usage setting");
        setting::upsert(&state.db, "response_retention_days", "1")
            .await
            .expect("response setting");

        insert_usage(&state, "old-usage", old).await;
        insert_usage(&state, "new-usage", now).await;
        insert_detail(&state, "old-detail", old).await;
        insert_detail(&state, "new-detail", now).await;
        insert_response(&state, "resp_old", key_id, old).await;
        insert_response(&state, "resp_new", key_id, now).await;

        let stats = cleanup_once(&state).await.expect("cleanup");
        assert_eq!(stats.usage_logs, 1);
        assert_eq!(stats.request_details, 1);
        assert_eq!(stats.responses, 1);

        let usage: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_logs")
            .fetch_one(&state.db)
            .await
            .expect("count");
        let details: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM request_details")
            .fetch_one(&state.db)
            .await
            .expect("count");
        let responses: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM responses")
            .fetch_one(&state.db)
            .await
            .expect("count");
        assert_eq!(usage, 1);
        assert_eq!(details, 1);
        assert_eq!(responses, 1);
    }

    #[tokio::test]
    async fn zero_retention_disables_cleanup() {
        let (_temp, state) = test_state().await;
        let key_id = seed_key(&state).await;
        let old = now_ms() - 365 * DAY_MS;

        setting::upsert(&state.db, "usage_log_retention_days", "0")
            .await
            .expect("usage setting");
        setting::upsert(&state.db, "response_retention_days", "0")
            .await
            .expect("response setting");
        insert_usage(&state, "old-usage", old).await;
        insert_detail(&state, "old-detail", old).await;
        insert_response(&state, "resp_old", key_id, old).await;

        let stats = cleanup_once(&state).await.expect("cleanup");
        assert_eq!(stats.usage_logs, 0);
        assert_eq!(stats.request_details, 0);
        assert_eq!(stats.responses, 0);

        let usage: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM usage_logs")
            .fetch_one(&state.db)
            .await
            .expect("count");
        assert_eq!(usage, 1);
    }

    #[tokio::test]
    async fn removes_expired_sessions() {
        let (_temp, state) = test_state().await;
        setting::upsert(&state.db, "usage_log_retention_days", "0")
            .await
            .expect("usage setting");
        setting::upsert(&state.db, "response_retention_days", "0")
            .await
            .expect("response setting");

        let hash = security::hash_password("password123").expect("hash");
        models::admin::create(&state.db, "admin", &hash, now_ms())
            .await
            .expect("admin");
        let expired = now_ms() - 1_000;
        models::session::create(&state.db, "expired-hash", 1, expired - 1_000, expired)
            .await
            .expect("session");

        let stats = cleanup_once(&state).await.expect("cleanup");
        assert_eq!(stats.sessions, 1);

        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&state.db)
            .await
            .expect("count");
        assert_eq!(sessions, 0);
    }
}
