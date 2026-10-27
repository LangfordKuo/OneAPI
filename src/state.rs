use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::{config::Config, metrics::LiveMetrics, security::Crypto};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub crypto: Arc<Crypto>,
    pub live: Arc<LiveMetrics>,
    pub started_at: DateTime<Utc>,
}
