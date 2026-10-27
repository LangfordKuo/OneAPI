use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, Mutex},
};

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use tokio::sync::Notify;

use crate::{admin::LoginLimiter, config::Config, metrics::LiveMetrics, security::Crypto};

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
    pub started_at: DateTime<Utc>,
}
