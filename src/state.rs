use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64},
        Arc, Mutex, RwLock,
    },
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use tokio::sync::Notify;

use crate::{
    admin::LoginLimiter,
    config::Config,
    events::EventBus,
    gateway::usage::UsageWriter,
    metrics::LiveMetrics,
    models::{
        channel::{self, Channel, ChannelHealth, PreparedChannel},
        now_ms,
        setting::GatewayDefaults,
    },
    security::Crypto,
    upstream::ClientCache,
};

const CACHE_TTL: Duration = Duration::from_secs(30);

/// Concurrent `/api/metrics/stream` connections allowed. Each stream parks a
/// socket for its whole lifetime, so a handful is enough for the dashboard and
/// bounds the resources a signed-in client can pin.
pub const METRICS_STREAM_LIMIT: usize = 4;

/// Shared snapshot of the prepared channel list; each element is shared so a
/// routing candidate can hold one channel without cloning it.
type ChannelSnapshot = Arc<Vec<Arc<PreparedChannel>>>;

/// Process-local cache for channel configuration and gateway defaults. Entries
/// are invalidated by the admin write paths; the TTL is a safety net for direct
/// database edits and multi-process deployments.
#[derive(Debug, Default)]
pub struct RuntimeCache {
    channels: RwLock<Option<(Instant, ChannelSnapshot)>>,
    /// Serializes cache reloads so a TTL expiry under concurrent traffic does
    /// not run the channel query and JSON pre-parse once per waiting request.
    channels_reload: tokio::sync::Mutex<()>,
    /// Routing overrides written by failure accounting. They are layered over
    /// the cached channel rows so a single failure does not force a full
    /// reload and re-parse of every channel.
    health: RwLock<HashMap<i64, (Instant, ChannelHealth)>>,
    defaults: RwLock<Option<(Instant, GatewayDefaults)>>,
    site_domain: RwLock<Option<(Instant, String)>>,
    site_name: RwLock<Option<(Instant, String)>>,
    client_ip_header: RwLock<Option<(Instant, String)>>,
    recent_models: RwLock<Option<(Instant, Vec<String>)>>,
}

impl RuntimeCache {
    /// Cached channels with their protocol configuration already parsed. The
    /// outer `Arc` is shared per request; each element is also `Arc`-wrapped so
    /// routing candidates can hold one channel without cloning it.
    pub async fn channels(&self, db: &SqlitePool) -> sqlx::Result<ChannelSnapshot> {
        if let Some((loaded_at, channels)) =
            self.channels.read().ok().and_then(|guard| guard.clone())
        {
            if loaded_at.elapsed() < CACHE_TTL {
                return Ok(channels);
            }
        }

        // Single-flight the reload: the first task refreshes, the rest wait and
        // then reuse the snapshot instead of issuing their own query.
        let _reload = self.channels_reload.lock().await;
        if let Some((loaded_at, channels)) =
            self.channels.read().ok().and_then(|guard| guard.clone())
        {
            if loaded_at.elapsed() < CACHE_TTL {
                return Ok(channels);
            }
        }

        let reload_started = Instant::now();
        let channels = crate::models::channel::list(db).await?;
        let prepared: ChannelSnapshot = Arc::new(
            channels
                .into_iter()
                .map(|channel| Arc::new(PreparedChannel::new(channel)))
                .collect(),
        );
        if let Ok(mut guard) = self.channels.write() {
            *guard = Some((Instant::now(), prepared.clone()));
        }
        // Rows read above include every failure recorded before this reload
        // started; overrides recorded later are newer than the snapshot.
        if let Ok(mut health) = self.health.write() {
            health.retain(|_, (recorded_at, _)| *recorded_at > reload_started);
        }
        Ok(prepared)
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
        // Admin edits make the database authoritative again.
        if let Ok(mut health) = self.health.write() {
            health.clear();
        }
    }

    pub fn record_channel_failure(&self, channel_id: i64, health: ChannelHealth) {
        if let Ok(mut guard) = self.health.write() {
            guard.insert(channel_id, (Instant::now(), health));
        }
    }

    pub fn record_channel_success(&self, channel_id: i64) {
        if let Ok(mut guard) = self.health.write() {
            guard.remove(&channel_id);
        }
    }

    /// Snapshot for the routing hot path. Only failed channels have entries, so
    /// this is small even with many configured channels.
    pub fn channel_health(&self) -> HashMap<i64, ChannelHealth> {
        self.health
            .read()
            .map(|guard| {
                guard
                    .iter()
                    .map(|(id, (_, health))| (*id, *health))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether a failure has been recorded since the channel rows were cached.
    /// Cheaper than [`channel_health`] because it does not build a snapshot.
    pub fn channel_has_failure_override(&self, channel_id: i64) -> bool {
        self.health
            .read()
            .map(|guard| guard.contains_key(&channel_id))
            .unwrap_or(false)
    }

    pub fn invalidate_defaults(&self) {
        if let Ok(mut guard) = self.defaults.write() {
            *guard = None;
        }
    }

    /// Public site domain used by the CSRF guard after a reverse proxy. Cached
    /// alongside the other runtime settings because it is read on every
    /// mutating admin request.
    pub async fn site_domain(&self, db: &SqlitePool) -> sqlx::Result<String> {
        if let Some((loaded_at, domain)) =
            self.site_domain.read().ok().and_then(|guard| guard.clone())
        {
            if loaded_at.elapsed() < CACHE_TTL {
                return Ok(domain);
            }
        }

        let domain = crate::models::setting::get(db, "site_domain")
            .await?
            .unwrap_or_default();
        if let Ok(mut guard) = self.site_domain.write() {
            *guard = Some((Instant::now(), domain.clone()));
        }
        Ok(domain)
    }

    pub fn invalidate_site_domain(&self) {
        if let Ok(mut guard) = self.site_domain.write() {
            *guard = None;
        }
    }

    /// Public branding name, read by `/api/site` before authentication.
    pub async fn site_name(&self, db: &SqlitePool) -> sqlx::Result<String> {
        if let Some((loaded_at, name)) = self.site_name.read().ok().and_then(|guard| guard.clone())
        {
            if loaded_at.elapsed() < CACHE_TTL {
                return Ok(name);
            }
        }

        let name = crate::models::setting::get(db, "site_name")
            .await?
            .unwrap_or_else(|| "OneAPI".to_string());
        if let Ok(mut guard) = self.site_name.write() {
            *guard = Some((Instant::now(), name.clone()));
        }
        Ok(name)
    }

    pub fn invalidate_site_name(&self) {
        if let Ok(mut guard) = self.site_name.write() {
            *guard = None;
        }
    }

    /// Header used to resolve the client IP; read by the login limiter on every
    /// attempt, so it goes through the same 30 second cache as the other
    /// settings instead of a database round trip per request.
    pub async fn client_ip_header(&self, db: &SqlitePool) -> sqlx::Result<String> {
        if let Some((loaded_at, header)) = self
            .client_ip_header
            .read()
            .ok()
            .and_then(|guard| guard.clone())
        {
            if loaded_at.elapsed() < CACHE_TTL {
                return Ok(header);
            }
        }

        let header = crate::models::setting::get(db, crate::client_ip::SETTING_KEY)
            .await?
            .unwrap_or_default();
        if let Ok(mut guard) = self.client_ip_header.write() {
            *guard = Some((Instant::now(), header.clone()));
        }
        Ok(header)
    }

    pub fn invalidate_client_ip_header(&self) {
        if let Ok(mut guard) = self.client_ip_header.write() {
            *guard = None;
        }
    }

    /// Models observed in the last 30 days, used to fill the catalog for
    /// wildcard channels. Aggregating on every model-list request is wasteful.
    pub async fn recent_models(&self, db: &SqlitePool) -> sqlx::Result<Vec<String>> {
        if let Some((loaded_at, models)) = self
            .recent_models
            .read()
            .ok()
            .and_then(|guard| guard.clone())
        {
            if loaded_at.elapsed() < CACHE_TTL {
                return Ok(models);
            }
        }

        let since = now_ms() - 30 * 24 * 60 * 60 * 1000;
        let models = crate::models::usage_log::recent_models(db, since, 200).await?;
        if let Ok(mut guard) = self.recent_models.write() {
            *guard = Some((Instant::now(), models.clone()));
        }
        Ok(models)
    }
}

#[derive(Default)]
pub struct UpdateState {
    pub in_progress: AtomicBool,
    pub restart_requested: AtomicBool,
    pub restart_exe: Mutex<Option<PathBuf>>,
    pub shutdown: Notify,
}

const AUTH_FAILURE_WINDOW: Duration = Duration::from_secs(60);
const AUTH_FAILURE_PER_IP_LIMIT: usize = 20;
/// Cap the per-peer vector so concurrent failures cannot grow it without bound.
const AUTH_FAILURE_MAX_TRACKED: usize = 128;

/// Sliding-window limiter for failed API key authentication. Keys are the TCP
/// peer address from `ConnectInfo`, never a forwarded header, so a spoofed
/// `X-Forwarded-For` cannot consume another client's budget.
#[derive(Debug, Default)]
pub struct AuthFailureLimiter {
    state: Mutex<AuthFailureState>,
}

#[derive(Debug, Default)]
struct AuthFailureState {
    per_ip: HashMap<IpAddr, Vec<Instant>>,
}

impl AuthFailureLimiter {
    fn lock(&self) -> std::sync::MutexGuard<'_, AuthFailureState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether the peer may still attempt authentication. Only the peer's own
    /// failures count: a global gate would let one attacker lock every tenant
    /// out of the gateway.
    pub fn check(&self, peer: Option<IpAddr>) -> bool {
        let now = Instant::now();
        let mut state = self.lock();
        state.retain_recent(now);

        match peer {
            Some(peer) => {
                let mut expired = false;
                let allowed = match state.per_ip.get_mut(&peer) {
                    Some(failures) => {
                        retain_recent(failures, now);
                        expired = failures.is_empty();
                        failures.len() < AUTH_FAILURE_PER_IP_LIMIT
                    }
                    None => true,
                };
                if expired {
                    state.per_ip.remove(&peer);
                }
                allowed
            }
            None => true,
        }
    }

    pub fn record_failure(&self, peer: Option<IpAddr>) {
        let now = Instant::now();
        let mut state = self.lock();
        state.retain_recent(now);
        if let Some(peer) = peer {
            let failures = state.per_ip.entry(peer).or_default();
            retain_recent(failures, now);
            if failures.len() < AUTH_FAILURE_MAX_TRACKED {
                failures.push(now);
            }
        }
    }

    pub fn record_success(&self, peer: Option<IpAddr>) {
        if let Some(peer) = peer {
            self.lock().per_ip.remove(&peer);
        }
    }
}

impl AuthFailureState {
    fn retain_recent(&mut self, now: Instant) {
        // Drop fully expired peers, but only scan the whole map when it grows
        // large so the common case stays cheap.
        if self.per_ip.len() > 10_000 {
            self.per_ip.retain(|_, failures| {
                retain_recent(failures, now);
                !failures.is_empty()
            });
        }
    }
}

fn retain_recent(attempts: &mut Vec<Instant>, now: Instant) {
    attempts.retain(|attempt| now.saturating_duration_since(*attempt) < AUTH_FAILURE_WINDOW);
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

        self.try_acquire(channel.id)
            .map(ProbeAttempt::Acquired)
            .unwrap_or(ProbeAttempt::Busy)
    }

    /// Claim the single probe slot for a channel that is still cooling down.
    /// Shared with client half-open probes so a background recovery check and
    /// a client request never probe the same channel at the same time.
    pub fn try_acquire_recovery(self: &Arc<Self>, channel_id: i64) -> Option<ProbeLease> {
        self.try_acquire(channel_id)
    }

    fn try_acquire(self: &Arc<Self>, channel_id: i64) -> Option<ProbeLease> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active.insert(channel_id) {
            Some(ProbeLease {
                probes: self.clone(),
                channel_id,
            })
        } else {
            None
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub crypto: Arc<Crypto>,
    pub live: Arc<LiveMetrics>,
    pub events: Arc<EventBus>,
    pub updates: Arc<UpdateState>,
    pub login_limiter: Arc<LoginLimiter>,
    pub auth_failure_limiter: Arc<AuthFailureLimiter>,
    pub clients: Arc<ClientCache>,
    pub cache: Arc<RuntimeCache>,
    pub probes: Arc<ChannelProbes>,
    pub usage_writer: Option<Arc<UsageWriter>>,
    pub started_at: DateTime<Utc>,
    /// Serializes database snapshots. The admin endpoint uses `try_lock` so a
    /// second request is rejected instead of queued behind a slow `VACUUM
    /// INTO`.
    pub backup_lock: Arc<tokio::sync::Mutex<()>>,
    /// Serializes the Responses storage quota check, write and accounting so
    /// concurrent requests cannot both pass the same preimage check.
    pub response_store_lock: Arc<tokio::sync::Mutex<()>>,
    /// Responses objects rejected by the quota or ownership check since start.
    pub response_store_rejected: Arc<AtomicU64>,
    /// Start of the most recent snapshot attempt, used to rate-limit manual
    /// backups. Ordinary `std::sync::Mutex` because it is held only for a
    /// short check and never across an await.
    pub last_backup_at: Arc<Mutex<Option<Instant>>>,
    /// Flips to `true` when the process starts shutting down. Long-lived
    /// handlers (metrics SSE) subscribe and end their streams so graceful
    /// shutdown and self-update restarts are never blocked by an open socket.
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// Bounds concurrent metrics SSE connections to
    /// [`METRICS_STREAM_LIMIT`]; each stream holds one permit until it ends.
    pub metrics_streams: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    /// Persist a failed attempt and update only this channel's routing view.
    /// Reloading the whole channel list on every failure turns an outage into
    /// a database and parsing storm, so the health overlay is authoritative
    /// until the row is reloaded or the channel succeeds again.
    pub async fn mark_channel_failure(&self, channel_id: i64, message: &str) {
        match channel::mark_failure(&self.db, channel_id, message, now_ms()).await {
            Ok(Some(health)) => self.cache.record_channel_failure(channel_id, health),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                channel_id,
                error = %error,
                "failed to record channel failure"
            ),
        }
    }

    /// Clear failure state after a success. Healthy channels do not change a
    /// row and therefore do not touch the overlay.
    pub async fn mark_channel_success(&self, channel_id: i64) {
        match channel::mark_success(&self.db, channel_id).await {
            Ok(true) => self.cache.record_channel_success(channel_id),
            Ok(false) => {}
            Err(error) => tracing::warn!(
                channel_id,
                error = %error,
                "failed to record channel success"
            ),
        }
    }

    /// Clear failure state only when the channel may have any. A zero-row
    /// UPDATE still opens a SQLite write transaction and takes the single
    /// writer lock, so healthy channels must skip it entirely.
    pub async fn mark_channel_success_if_needed(&self, channel_id: i64, may_have_failure: bool) {
        if may_have_failure {
            self.mark_channel_success(channel_id).await;
        }
    }

    /// A background recovery probe failed. Keep the channel out of rotation
    /// until `retry_at` without counting another traffic failure, so the
    /// exponential backoff reflects real client traffic rather than probes.
    pub async fn defer_channel_recovery(&self, channel_id: i64, message: &str, retry_at: i64) {
        match channel::defer_recovery(&self.db, channel_id, message, now_ms(), retry_at).await {
            Ok(Some(health)) => self.cache.record_channel_failure(channel_id, health),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                channel_id,
                error = %error,
                "failed to defer channel recovery"
            ),
        }
    }
}
