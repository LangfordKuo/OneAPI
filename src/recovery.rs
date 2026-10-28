use std::{sync::Arc, time::Duration};

use axum::http::HeaderMap;
use futures_util::{stream, StreamExt};

use crate::{
    gateway::error::truncate_message,
    models::{
        channel::{self, Channel, PreparedChannel, ProtocolConfig},
        now_ms,
    },
    state::AppState,
    upstream,
};

/// How often unhealthy channels are re-checked.
const RECOVERY_INTERVAL: Duration = Duration::from_secs(60);
/// Cap for one probe so a hanging channel cannot stall the loop. It is
/// deliberately longer than a typical first token: a healthy but slow
/// reasoning model must not be kept offline by a false negative.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Probes run concurrently, but not all at once on large deployments.
const PROBE_CONCURRENCY: usize = 8;
/// Failed probes postpone the next attempt by one interval.
const RETRY_DELAY_MS: i64 = 60_000;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecoverySummary {
    pub probed: usize,
    pub recovered: usize,
    pub still_down: usize,
}

/// Start the periodic channel recovery task. It probes every channel that has
/// recorded failures, so a channel can come back online during its cooldown
/// even when no client traffic reaches it.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RECOVERY_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let summary = probe_unhealthy_channels(&state).await;
            if summary.probed > 0 {
                tracing::info!(
                    probed = summary.probed,
                    recovered = summary.recovered,
                    still_down = summary.still_down,
                    "channel recovery probe"
                );
            }
        }
    });
}

/// Probe every enabled channel with recorded failures once. A successful probe
/// clears the cooldown immediately; a failed probe postpones the next attempt
/// without growing the traffic-failure backoff.
pub async fn probe_unhealthy_channels(state: &AppState) -> RecoverySummary {
    let channels = match state.cache.channels(&state.db).await {
        Ok(channels) => channels,
        Err(error) => {
            tracing::warn!(error = %error, "failed to load channels for recovery probes");
            return RecoverySummary::default();
        }
    };
    let health = state.cache.channel_health();
    let targets: Vec<Arc<PreparedChannel>> = channels
        .iter()
        .filter(|prepared| prepared.channel.enabled != 0)
        .filter(|prepared| {
            let entry = health.get(&prepared.channel.id);
            let failures = entry
                .map(|health| health.consecutive_failures)
                .unwrap_or(prepared.channel.consecutive_failures);
            let disabled_until = entry
                .and_then(|health| health.disabled_until)
                .or(prepared.channel.disabled_until);
            // Only channels that were put into cooldown (or whose cooldown
            // already expired but have not recovered yet) are probed.
            failures > 0 && disabled_until.is_some()
        })
        .cloned()
        .collect();

    let outcomes = stream::iter(targets.into_iter().map(|prepared| {
        let state = state.clone();
        async move { probe_one(&state, &prepared).await }
    }))
    .buffer_unordered(PROBE_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    let mut summary = RecoverySummary::default();
    for outcome in outcomes {
        match outcome {
            ProbeOutcome::Skipped => {}
            ProbeOutcome::Recovered => {
                summary.probed += 1;
                summary.recovered += 1;
            }
            ProbeOutcome::StillDown => {
                summary.probed += 1;
                summary.still_down += 1;
            }
        }
    }
    if summary.recovered > 0 {
        // Cached rows still carry the old cooldown. Drop the snapshot so the
        // recovered channel routes immediately, not after the 30-second TTL.
        state.cache.invalidate_channels();
    }
    summary
}

enum ProbeOutcome {
    Skipped,
    Recovered,
    StillDown,
}

async fn probe_one(state: &AppState, prepared: &PreparedChannel) -> ProbeOutcome {
    let channel = &prepared.channel;
    let Some(_lease) = state.probes.try_acquire_recovery(channel.id) else {
        // A client half-open probe or another tick is already checking it.
        return ProbeOutcome::Skipped;
    };
    match probe_channel(state, channel).await {
        Ok(message) => {
            state.mark_channel_success(channel.id).await;
            tracing::info!(
                channel_id = channel.id,
                channel = %channel.name,
                message,
                "channel recovered"
            );
            ProbeOutcome::Recovered
        }
        Err(message) => {
            let retry_at = now_ms() + RETRY_DELAY_MS;
            state
                .defer_channel_recovery(channel.id, &message, retry_at)
                .await;
            tracing::warn!(
                channel_id = channel.id,
                channel = %channel.name,
                error = %message,
                "channel recovery probe failed"
            );
            ProbeOutcome::StillDown
        }
    }
}

async fn probe_channel(state: &AppState, channel: &Channel) -> Result<String, String> {
    let mut configs = channel::native_configs(channel)
        .into_iter()
        .filter(|config| !config.base_url.trim().is_empty());
    let Some(first) = configs.next() else {
        return Err("channel has no routable native protocol".to_string());
    };
    let client = upstream::build_client_with_timeout(channel, PROBE_TIMEOUT.as_millis() as u64)
        .map_err(|error| error.to_string())?;

    // A channel can expose several native protocols. A success on any one of
    // them proves the upstream is reachable again; traffic failures will cool
    // it down again if a different route is still broken.
    let mut last = "recovery probe failed".to_string();
    for config in std::iter::once(first).chain(configs) {
        match probe_config(state, &client, &config).await {
            Ok(message) => return Ok(message),
            Err(message) => last = message,
        }
    }
    Err(last)
}

async fn probe_config(
    state: &AppState,
    client: &reqwest::Client,
    config: &ProtocolConfig,
) -> Result<String, String> {
    let (request, description) = match upstream::probe_model(config) {
        Some(model) => {
            let body = upstream::probe_body(&config.protocol, &model);
            let request = upstream::build_request(
                client,
                config,
                &state.crypto,
                &config.protocol,
                &body,
                &HeaderMap::new(),
            )
            .map_err(|error| error.to_string())?;
            (request, format!("{} model {model}", config.protocol))
        }
        None => {
            // Wildcard-only channels have no concrete model to send, so verify
            // connectivity through the model catalog endpoint instead.
            let request = upstream::apply_auth(
                client.get(upstream::models_url(config)),
                config,
                &state.crypto,
                &config.protocol,
                &HeaderMap::new(),
            )
            .map_err(|error| error.to_string())?;
            (request, format!("{} models", config.protocol))
        }
    };

    let response = request
        .send()
        .await
        .map_err(|error| format!("{description}: {error}"))?;
    let status = response.status();
    let text = upstream::read_body_limited(response)
        .await
        .unwrap_or_default();
    if status.is_success() {
        Ok(format!("{description} ok"))
    } else {
        Err(format!(
            "{description}: HTTP {} {}",
            status.as_u16(),
            truncate_message(&text, 200)
        ))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    async fn pool() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("memory database");
        sqlx::query(
            "CREATE TABLE channels (
                id INTEGER PRIMARY KEY,
                last_error TEXT,
                last_error_at INTEGER,
                disabled_until INTEGER,
                consecutive_failures INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("schema");
        pool
    }

    #[tokio::test]
    async fn deferring_recovery_keeps_the_traffic_failure_count() {
        let pool = pool().await;
        sqlx::query(
            "INSERT INTO channels (id, last_error, disabled_until, consecutive_failures, updated_at)
             VALUES (1, 'boom', 1000, 2, 0)",
        )
        .execute(&pool)
        .await
        .expect("row");

        let health = crate::models::channel::defer_recovery(&pool, 1, "still down", 2000, 62000)
            .await
            .expect("defer")
            .expect("row");

        assert_eq!(health.consecutive_failures, 2);
        // The old cooldown had expired, so the probe extends it by one interval.
        assert_eq!(health.disabled_until, Some(62000));
        let (failures, error): (i64, Option<String>) =
            sqlx::query_as("SELECT consecutive_failures, last_error FROM channels WHERE id = 1")
                .fetch_one(&pool)
                .await
                .expect("row");
        assert_eq!(failures, 2);
        assert_eq!(error.as_deref(), Some("still down"));
    }

    #[tokio::test]
    async fn deferring_recovery_does_not_shorten_a_running_cooldown() {
        let pool = pool().await;
        sqlx::query(
            "INSERT INTO channels (id, last_error, disabled_until, consecutive_failures, updated_at)
             VALUES (1, 'boom', 1800000, 5, 0)",
        )
        .execute(&pool)
        .await
        .expect("row");

        let health = crate::models::channel::defer_recovery(&pool, 1, "still down", 2000, 62000)
            .await
            .expect("defer")
            .expect("row");

        assert_eq!(health.consecutive_failures, 5);
        assert_eq!(health.disabled_until, Some(1_800_000));
    }

    #[test]
    fn probe_body_asks_for_a_tiny_response() {
        let body = upstream::probe_body("openai_chat", "gpt-test");
        assert_eq!(body["max_tokens"], Value::from(1));
        assert_eq!(body["stream"], Value::from(false));
    }
}
