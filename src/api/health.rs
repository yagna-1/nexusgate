use axum::{extract::State, Json};
use sqlx::Row;
use std::sync::Arc;

use crate::{
    error::Result,
    models::{HealthResponse, ProviderStatus},
    rate_limit::RateLimiter,
    state::AppState,
};

pub async fn health(State(state): State<Arc<AppState>>) -> Result<Json<HealthResponse>> {
    let rate_limiter = RateLimiter::new(state.redis.clone());
    let rl_statuses = rate_limiter.all_statuses().await;

    // Count today's requests per provider
    let provider_rows = sqlx::query(
        r#"
        SELECT provider, COUNT(*) as count, COALESCE(SUM(cost_micro_usd), 0) as cost
        FROM cost_records
        WHERE created_at >= date('now')
        GROUP BY provider
        "#,
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let provider_data: std::collections::HashMap<String, (i64, f64)> = provider_rows
        .iter()
        .map(|r| {
            (
                r.get::<String, _>("provider"),
                (
                    r.try_get::<i64, _>("count").unwrap_or(0),
                    r.try_get::<i64, _>("cost").unwrap_or(0) as f64 / 1_000_000.0,
                ),
            )
        })
        .collect();

    let providers: Vec<ProviderStatus> = rl_statuses
        .iter()
        .map(|(name, rate_limited, reset_secs)| {
            let configured = match name.as_str() {
                "openai" => state.config.openai_api_key.is_some(),
                "anthropic" => state.config.anthropic_api_key.is_some(),
                "gemini" => state.config.gemini_api_key.is_some(),
                _ => false,
            };
            let (requests_today, cost_today_usd) = provider_data
                .get(name.as_str())
                .copied()
                .unwrap_or((0, 0.0));

            ProviderStatus {
                provider: name.clone(),
                configured,
                available: configured && !rate_limited,
                rate_limited: *rate_limited,
                rate_limit_reset_secs: *reset_secs,
                requests_today,
                cost_today_usd,
            }
        })
        .collect();

    Ok(Json(HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        providers,
        uptime_secs: state.uptime_secs(),
    }))
}
