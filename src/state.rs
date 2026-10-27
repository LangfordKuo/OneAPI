use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use tokio::sync::Notify;

use crate::{
    admin::LoginLimiter,
    config::Config,
    gateway::usage::UsageWriter,
    metrics::LiveMetrics,
    models::{
        channel::{self, Channel},
        now_ms,
        setting::GatewayDefaults,
    },
    security::Crypto,
    upstream::ClientCache,
};

const CACHE_TTL: Duration = Duration::from_secs(30);

/// Process-local cache for channel configuration and gateway defaults. Entries
/// are invalidated by the admin write paths; the TTL is a safety net for direct
/// database edits and multi-process deployments.
#[derive(Debug, Default)]
pub struct RuntimeCache {
    channels: RwLock<Option<(Instant, Vec<Channel>)>>,
    defaults: RwLock<Option<(Instant, GatewayDefaults)>>,
}

impl RuntimeCache {
    pub async fn channels(&self, db: &SqlitePool) -> sqlx::Result<Vec<Channel>> {
        if let Some((loaded_at, channels)) =
            self.channels.read().ok().and_then(|guard| guard.clone())
        {
            if loaded_at.elapsed() < CACHE_TTL {
                return Ok(channels);
            }
        }

        let channels = crate::models::channel::list(db).await?;
        if let Ok(mut guard) = self.channels.write() {
            *guard = Some((Instant::now(), channels.clone()));
        }
        Ok(channels)
    }

    pub async fn defaults(&self, db: &SqlitePool) -> sqlx::Result<GatewayDefaults> {
        if let Some((loaded_at, defaults)) = self
            .defaults
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().copied())
        {
            if loaded_at.elapsed() < CACHE_TTL {
                return Ok(defaults);
            }
        }

        let defaults = GatewayDefaults::load(db).await?;
        if let Ok(mut guard) = self.defaults.write() {
            *guard = Some((Instant::now(), defaults));
        }
        Ok(defaults)
    }

    pub fn invalidate_channels(&self) {
        if let Ok(mut guard) = self.channels.write() {
            *guard = None;
        }
    }

    pub fn invalidate_defaults(&self) {
        if let Ok(mut guard) = self.defaults.write() {
            *guard = None;
        }
    }
}

#[derive(Default)]
pub struct UpdateState {
    pub in_progress: AtomicBool,
    pub restart_requested: AtomicBool,
    pub restart_exe: Mutex<Option<PathBuf>>,
    pub shutdown: Notify,
}

/// Result of asking to try a channel that is coming out of cooldown.
pub enum ProbeAttempt {
    /// The channel is not recovering, so normal traffic may use it.
    NotNeeded,
    /// This request owns the single half-open trial for the channel.
    Acquired(ProbeLease),
    /// Another request is already probing the channel.
    Busy,
}

/// Process-local guard that lets exactly one request probe a recovering
/// channel. The lease releases the slot when the attempt (or stream) finishes.
#[derive(Debug, Default)]
pub struct ChannelProbes {
    active: Mutex<HashSet<i64>>,
}

pub struct ProbeLease {
    probes: Arc<ChannelProbes>,
    channel_id: i64,
}

impl Drop for ProbeLease {
    fn drop(&mut self) {
        if let Ok(mut active) = self.probes.active.lock() {
            active.remove(&self.channel_id);
        }
    }
}

impl ChannelProbes {
    pub fn acquire(self: &Arc<Self>, channel: &Channel, now: i64) -> ProbeAttempt {
        let recovering = channel.disabled_until.is_some_and(|until| until <= now);
        if !recovering || channel.consecutive_failures <= 0 {
            return ProbeAttempt::NotNeeded;
        }

        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active.insert(channel.id) {
            ProbeAttempt::Acquired(ProbeLease {
                probes: self.clone(),
                channel_id: channel.id,
            })
        } else {
            ProbeAttempt::Busy
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub crypto: Arc<Crypto>,
    pub live: Arc<LiveMetrics>,
    pub updates: Arc<UpdateState>,
    pub login_limiter: Arc<LoginLimiter>,
    pub clients: Arc<ClientCache>,
    pub cache: Arc<RuntimeCache>,
    pub probes: Arc<ChannelProbes>,
    pub usage_writer: Option<Arc<UsageWriter>>,
    pub started_at: DateTime<Utc>,
}

impl AppState {
    /// Persist a failed attempt and drop the cached channel list so the next
    /// request observes the new cooldown immediately.
    pub async fn mark_channel_failure(&self, channel_id: i64, message: &str) {
        match channel::mark_failure(&self.db, channel_id, message, now_ms()).await {
            Ok(()) => self.cache.invalidate_channels(),
            Err(error) => tracing::warn!(
                channel_id,
                error = %error,
                "failed to record channel failure"
            ),
        }
    }

    /// Clear failure state after a success. Healthy channels do not change a
    /// row and therefore do not invalidate the cache.
    pub async fn mark_channel_success(&self, channel_id: i64) {
        match channel::mark_success(&self.db, channel_id).await {
            Ok(true) => self.cache.invalidate_channels(),
            Ok(false) => {}
            Err(error) => tracing::warn!(
                channel_id,
                error = %error,
                "failed to record channel success"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(id: i64, consecutive_failures: i64, disabled_until: Option<i64>) -> Channel {
        Channel {
            id,
            name: "test".to_string(),
            kind: "openai_chat".to_string(),
            kinds: r#"["openai_chat"]"#.to_string(),
            fallback_kind: Some("openai_chat".to_string()),
            base_url: "http://localhost".to_string(),
            api_key_enc: String::new(),
            auth_header: "bearer".to_string(),
            models: "[]".to_string(),
            model_mapping: "{}".to_string(),
            extra_headers: "{}".to_string(),
            proxy_url: None,
            priority: 0,
            weight: 1,
            timeout_ms: None,
            enabled: 1,
            last_error: None,
            last_error_at: None,
            disabled_until,
            consecutive_failures,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn healthy_and_cooling_channels_need_no_probe() {
        let probes = Arc::new(ChannelProbes::default());
        let now = 10_000;

        assert!(matches!(
            probes.acquire(&channel(1, 0, None), now),
            ProbeAttempt::NotNeeded
        ));
        assert!(matches!(
            probes.acquire(&channel(2, 2, Some(now + 1)), now),
            ProbeAttempt::NotNeeded
        ));
        assert!(matches!(
            probes.acquire(&channel(3, 0, Some(now)), now),
            ProbeAttempt::NotNeeded
        ));
    }

    #[test]
    fn recovering_channel_allows_single_probe() {
        let probes = Arc::new(ChannelProbes::default());
        let recovering = channel(7, 3, Some(5_000));

        let lease = match probes.acquire(&recovering, 10_000) {
            ProbeAttempt::Acquired(lease) => lease,
            _ => panic!("expected probe lease"),
        };
        assert!(matches!(
            probes.acquire(&recovering, 10_000),
            ProbeAttempt::Busy
        ));
        assert!(matches!(
            probes.acquire(&channel(8, 1, Some(5_000)), 10_000),
            ProbeAttempt::Acquired(_)
        ));

        drop(lease);
        assert!(matches!(
            probes.acquire(&recovering, 10_000),
            ProbeAttempt::Acquired(_)
        ));
    }
}
