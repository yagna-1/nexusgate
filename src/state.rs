use std::sync::Arc;
use std::time::Instant;

use crate::config::Config;

/// Shared across all request handlers via Arc
pub struct AppState {
    pub config: Config,
    pub db: sqlx::SqlitePool,
    pub redis: Arc<tokio::sync::Mutex<redis::aio::ConnectionManager>>,
    pub started_at: Instant,
}

impl AppState {
    pub fn new(config: Config, db: sqlx::SqlitePool, redis: redis::aio::ConnectionManager) -> Self {
        AppState {
            config,
            db,
            redis: Arc::new(tokio::sync::Mutex::new(redis)),
            started_at: Instant::now(),
        }
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}
