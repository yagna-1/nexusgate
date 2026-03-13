use redis::AsyncCommands;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::{error::Result, models::ProviderType};

const RATE_LIMIT_KEY_PREFIX: &str = "ng:rl:";
const DEFAULT_BACKOFF_SECS: u64 = 60;
const MAX_BACKOFF_SECS: u64 = 300;

pub struct RateLimiter {
    redis: Arc<Mutex<redis::aio::ConnectionManager>>,
}

impl RateLimiter {
    pub fn new(redis: Arc<Mutex<redis::aio::ConnectionManager>>) -> Self {
        Self { redis }
    }

    fn key(provider: &ProviderType) -> String {
        format!("{}{}", RATE_LIMIT_KEY_PREFIX, provider)
    }

    fn backoff_key(provider: &ProviderType) -> String {
        format!("{}{}:backoff", RATE_LIMIT_KEY_PREFIX, provider)
    }

    /// Returns true if this provider is currently rate limited
    pub async fn is_rate_limited(&self, provider: &ProviderType) -> bool {
        let key = Self::key(provider);
        let mut conn = self.redis.lock().await;
        let result: Option<i32> = conn.get(&key).await.unwrap_or(None);
        result.is_some()
    }

    /// Returns seconds until rate limit resets, or None if not rate limited
    pub async fn rate_limit_reset_secs(&self, provider: &ProviderType) -> Option<i64> {
        let key = Self::key(provider);
        let mut conn = self.redis.lock().await;
        conn.ttl::<_, i64>(&key).await.ok().filter(|&t| t > 0)
    }

    /// Mark a provider as rate limited. retry_after_secs comes from the provider response header.
    pub async fn mark_rate_limited(
        &self,
        provider: &ProviderType,
        retry_after_secs: Option<u64>,
    ) -> Result<()> {
        let backoff_secs = self.calculate_backoff(provider, retry_after_secs).await;
        let key = Self::key(provider);
        let mut conn = self.redis.lock().await;

        // SETEX key ttl value
        let _: () = conn
            .set_ex(&key, 1i32, backoff_secs)
            .await
            .unwrap_or(());

        warn!(
            provider = %provider,
            backoff_secs,
            "Provider marked as rate limited"
        );
        Ok(())
    }

    /// Clear rate limit (e.g. after a successful request proves it's back)
    pub async fn clear_rate_limit(&self, provider: &ProviderType) -> Result<()> {
        let key = Self::key(provider);
        let mut conn = self.redis.lock().await;
        let _: () = conn.del(&key).await.unwrap_or(());
        info!(provider = %provider, "Rate limit cleared");
        Ok(())
    }

    /// Exponential backoff: doubles each consecutive rate limit hit, capped at MAX
    async fn calculate_backoff(
        &self,
        provider: &ProviderType,
        explicit_retry_after: Option<u64>,
    ) -> u64 {
        // If the provider told us when to retry, use that
        if let Some(secs) = explicit_retry_after {
            return secs.min(MAX_BACKOFF_SECS);
        }

        // Otherwise exponential backoff based on consecutive hits
        let backoff_key = Self::backoff_key(provider);
        let mut conn = self.redis.lock().await;

        let current: u64 = conn
            .get::<_, Option<u64>>(&backoff_key)
            .await
            .unwrap_or(None)
            .unwrap_or(DEFAULT_BACKOFF_SECS / 2);

        let next = (current * 2).min(MAX_BACKOFF_SECS);
        let _: () = conn
            .set_ex(&backoff_key, next, MAX_BACKOFF_SECS * 2)
            .await
            .unwrap_or(());

        next
    }

    /// Get status for all providers
    pub async fn all_statuses(&self) -> Vec<(String, bool, Option<i64>)> {
        let providers = [ProviderType::OpenAI, ProviderType::Anthropic, ProviderType::Gemini];
        let mut statuses = vec![];

        for provider in &providers {
            let is_limited = self.is_rate_limited(provider).await;
            let reset_secs = if is_limited {
                self.rate_limit_reset_secs(provider).await
            } else {
                None
            };
            statuses.push((provider.to_string(), is_limited, reset_secs));
        }

        statuses
    }
}
