use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::{config::Config, security::Crypto};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub crypto: Arc<Crypto>,
    pub started_at: DateTime<Utc>,
}
