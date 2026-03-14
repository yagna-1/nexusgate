use chrono::Utc;
use redis::AsyncCommands;
use sqlx::Row;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::warn;

use crate::{
    error::{AppError, Result},
    models::AuthContext,
};

const BUDGET_PREFIX: &str = "ng:budget:";

pub struct BudgetEnforcer {
    redis: Arc<Mutex<redis::aio::ConnectionManager>>,
    db: sqlx::SqlitePool,
}

impl BudgetEnforcer {
    pub fn new(redis: Arc<Mutex<redis::aio::ConnectionManager>>, db: sqlx::SqlitePool) -> Self {
        Self { redis, db }
    }

    // ── Redis key helpers ─────────────────────────────────────────────────────

    fn daily_key(scope: &str, id: &str) -> String {
        let today = Utc::now().format("%Y-%m-%d");
        format!("{}{scope}:{id}:daily:{today}", BUDGET_PREFIX)
    }

    fn monthly_key(scope: &str, id: &str) -> String {
        let ym = Utc::now().format("%Y-%m");
        format!("{}{scope}:{id}:monthly:{ym}", BUDGET_PREFIX)
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Check all budget limits before dispatching to a provider.
    /// estimated_micro_usd is a conservative upper bound for the request.
    pub async fn check(&self, ctx: &AuthContext, estimated_micro_usd: i64) -> Result<()> {
        // 1. Daily limit
        if let Some(daily_limit) = ctx.budget_daily_micro_usd {
            let spent = self.get_daily_spend("key", &ctx.key_id).await;
            if spent + estimated_micro_usd > daily_limit {
                let limit_usd = daily_limit as f64 / 1_000_000.0;
                let spent_usd = spent as f64 / 1_000_000.0;
                warn!(
                    key_id = %ctx.key_id,
                    spent_usd,
                    limit_usd,
                    "Daily budget exceeded"
                );
                return Err(AppError::BudgetExceeded(format!(
                    "Daily budget of ${:.4} exceeded (spent ${:.4})",
                    limit_usd, spent_usd
                )));
            }
        }

        // 2. Monthly limit
        if let Some(monthly_limit) = ctx.budget_monthly_micro_usd {
            let spent = self.get_monthly_spend("key", &ctx.key_id).await;
            if spent + estimated_micro_usd > monthly_limit {
                let limit_usd = monthly_limit as f64 / 1_000_000.0;
                let spent_usd = spent as f64 / 1_000_000.0;
                return Err(AppError::BudgetExceeded(format!(
                    "Monthly budget of ${:.4} exceeded (spent ${:.4})",
                    limit_usd, spent_usd
                )));
            }
        }

        // 3. Total lifetime limit (from SQLite — authoritative)
        if let Some(total_limit) = ctx.budget_total_micro_usd {
            let spent = self.get_total_spend_from_db(&ctx.key_id).await?;
            if spent + estimated_micro_usd > total_limit {
                return Err(AppError::BudgetExceeded(format!(
                    "Total budget of ${:.4} exceeded",
                    total_limit as f64 / 1_000_000.0
                )));
            }
        }

        Ok(())
    }

    /// Check workflow-scoped daily/monthly limits.
    pub async fn check_workflow(
        &self,
        workflow_id: &str,
        daily_limit: Option<i64>,
        monthly_limit: Option<i64>,
        estimated_micro_usd: i64,
    ) -> Result<()> {
        if let Some(limit) = daily_limit {
            let spent = self.get_daily_spend("workflow", workflow_id).await;
            if spent + estimated_micro_usd > limit {
                let limit_usd = limit as f64 / 1_000_000.0;
                let spent_usd = spent as f64 / 1_000_000.0;
                return Err(AppError::BudgetExceeded(format!(
                    "Workflow daily budget of ${:.4} exceeded for workflow '{}' (spent ${:.4})",
                    limit_usd, workflow_id, spent_usd
                )));
            }
        }

        if let Some(limit) = monthly_limit {
            let spent = self.get_monthly_spend("workflow", workflow_id).await;
            if spent + estimated_micro_usd > limit {
                let limit_usd = limit as f64 / 1_000_000.0;
                let spent_usd = spent as f64 / 1_000_000.0;
                return Err(AppError::BudgetExceeded(format!(
                    "Workflow monthly budget of ${:.4} exceeded for workflow '{}' (spent ${:.4})",
                    limit_usd, workflow_id, spent_usd
                )));
            }
        }

        Ok(())
    }

    /// Record actual cost after a successful request (Redis counters + SQLite record)
    pub async fn record_cost(
        &self,
        key_id: &str,
        provider: &str,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        cost_micro_usd: i64,
        request_id: &str,
        fallback_used: bool,
    ) -> Result<()> {
        self.record_cost_for(
            key_id,
            None,
            provider,
            model,
            input_tokens,
            output_tokens,
            cost_micro_usd,
            request_id,
            fallback_used,
        )
        .await
    }

    /// Record cost and optionally attribute it to a workflow ID.
    pub async fn record_cost_for(
        &self,
        key_id: &str,
        workflow_id: Option<&str>,
        provider: &str,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        cost_micro_usd: i64,
        request_id: &str,
        fallback_used: bool,
    ) -> Result<()> {
        // Update Redis counters atomically (fast, for budget checking)
        {
            let mut conn = self.redis.lock().await;
            let daily_key = Self::daily_key("key", key_id);
            let monthly_key = Self::monthly_key("key", key_id);

            // INCRBY + set TTL if key is new
            let _: i64 = conn.incr(&daily_key, cost_micro_usd).await.unwrap_or(0);
            // 26 hours TTL so the key definitely outlasts the day
            let _: () = conn.expire(&daily_key, 26 * 3600).await.unwrap_or(());

            let _: i64 = conn.incr(&monthly_key, cost_micro_usd).await.unwrap_or(0);
            // 32 days TTL
            let _: () = conn
                .expire(&monthly_key, 32 * 24 * 3600)
                .await
                .unwrap_or(());

            if let Some(workflow_id) = workflow_id {
                let workflow_daily = Self::daily_key("workflow", workflow_id);
                let workflow_monthly = Self::monthly_key("workflow", workflow_id);
                let _: i64 = conn
                    .incr(&workflow_daily, cost_micro_usd)
                    .await
                    .unwrap_or(0);
                let _: () = conn.expire(&workflow_daily, 26 * 3600).await.unwrap_or(());
                let _: i64 = conn
                    .incr(&workflow_monthly, cost_micro_usd)
                    .await
                    .unwrap_or(0);
                let _: () = conn
                    .expire(&workflow_monthly, 32 * 24 * 3600)
                    .await
                    .unwrap_or(());
            }
        }

        // Write durable record to SQLite
        let record_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            r#"
            INSERT INTO cost_records
                (id, api_key_id, provider, model, input_tokens, output_tokens,
                 cost_micro_usd, workflow_id, request_id, fallback_used)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&record_id)
        .bind(key_id)
        .bind(provider)
        .bind(model)
        .bind(input_tokens as i64)
        .bind(output_tokens as i64)
        .bind(cost_micro_usd)
        .bind(workflow_id)
        .bind(request_id)
        .bind(fallback_used as i64)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    async fn get_daily_spend(&self, scope: &str, id: &str) -> i64 {
        let key = Self::daily_key(scope, id);
        let mut conn = self.redis.lock().await;
        conn.get::<_, Option<i64>>(&key)
            .await
            .unwrap_or(None)
            .unwrap_or(0)
    }

    async fn get_monthly_spend(&self, scope: &str, id: &str) -> i64 {
        let key = Self::monthly_key(scope, id);
        let mut conn = self.redis.lock().await;
        conn.get::<_, Option<i64>>(&key)
            .await
            .unwrap_or(None)
            .unwrap_or(0)
    }

    async fn get_total_spend_from_db(&self, key_id: &str) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COALESCE(SUM(cost_micro_usd), 0) as total FROM cost_records WHERE api_key_id = ?",
        )
        .bind(key_id)
        .fetch_one(&self.db)
        .await?;

        Ok(row.try_get::<i64, _>("total").unwrap_or(0))
    }

    /// Get spend summaries for a key (used in admin API)
    pub async fn get_spend_summary(&self, key_id: &str) -> Result<(f64, f64, f64)> {
        // today, this month, total
        let today = self.get_daily_spend("key", key_id).await;
        let month = self.get_monthly_spend("key", key_id).await;
        let total = self.get_total_spend_from_db(key_id).await?;

        Ok((
            today as f64 / 1_000_000.0,
            month as f64 / 1_000_000.0,
            total as f64 / 1_000_000.0,
        ))
    }

    pub async fn get_workflow_spend_summary(&self, workflow_id: &str) -> (f64, f64) {
        let today = self.get_daily_spend("workflow", workflow_id).await;
        let month = self.get_monthly_spend("workflow", workflow_id).await;
        (today as f64 / 1_000_000.0, month as f64 / 1_000_000.0)
    }
}
