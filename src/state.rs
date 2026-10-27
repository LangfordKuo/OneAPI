use std::{
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
    models::{channel::Channel, setting::GatewayDefaults},
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
    pub usage_writer: Option<Arc<UsageWriter>>,
    pub started_at: DateTime<Utc>,
}
