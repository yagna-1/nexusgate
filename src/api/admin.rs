use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;

use crate::{
    auth::{generate_api_key, hash_key},
    budget::BudgetEnforcer,
    error::{AppError, Result},
    models::{
        ApiKeyInfo, CostSummary, CreateApiKeyRequest, CreateApiKeyResponse,
        DailyCost, ModelCostBreakdown, ProviderCostBreakdown,
    },
    state::AppState,
};

// ── API Keys ─────────────────────────────────────────────────────────────────

pub async fn list_keys(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ApiKeyInfo>>> {
    let rows = sqlx::query(
        r#"
        SELECT id, name, is_active, budget_total_micro_usd, budget_daily_micro_usd,
               budget_monthly_micro_usd, max_tokens_per_request, allowed_tiers,
               created_at, last_used_at
        FROM api_keys
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(&state.db)
    .await?;

    let budget = BudgetEnforcer::new(state.redis.clone(), state.db.clone());
    let mut keys = vec![];

    for row in rows {
        let key_id: String = row.get("id");
        let (spent_today, spent_month, spent_total) =
            budget.get_spend_summary(&key_id).await.unwrap_or((0.0, 0.0, 0.0));

        let allowed_tiers: Option<Vec<String>> = row
            .try_get::<Option<String>, _>("allowed_tiers")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok());

        keys.push(ApiKeyInfo {
            id: key_id,
            name: row.get("name"),
            is_active: row.get::<bool, _>("is_active"),
            budget_total_usd: row
                .try_get::<Option<i64>, _>("budget_total_micro_usd")
                .ok()
                .flatten()
                .map(|v| v as f64 / 1_000_000.0),
            budget_daily_usd: row
                .try_get::<Option<i64>, _>("budget_daily_micro_usd")
                .ok()
                .flatten()
                .map(|v| v as f64 / 1_000_000.0),
            budget_monthly_usd: row
                .try_get::<Option<i64>, _>("budget_monthly_micro_usd")
                .ok()
                .flatten()
                .map(|v| v as f64 / 1_000_000.0),
            max_tokens_per_request: row.try_get("max_tokens_per_request").ok().flatten(),
            allowed_tiers,
            created_at: row.get("created_at"),
            last_used_at: row.try_get("last_used_at").ok().flatten(),
            spent_today_usd: spent_today,
            spent_this_month_usd: spent_month,
            spent_total_usd: spent_total,
        });
    }

    Ok(Json(keys))
}

pub async fn create_key(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateApiKeyRequest>,
) -> Result<Json<CreateApiKeyResponse>> {
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("Key name cannot be empty".into()));
    }

    let raw_key = generate_api_key();
    let key_hash = hash_key(&raw_key);
    let id = uuid::Uuid::new_v4().to_string();

    let budget_total = req.budget_total_usd.map(|v| (v * 1_000_000.0) as i64);
    let budget_daily = req.budget_daily_usd.map(|v| (v * 1_000_000.0) as i64);
    let budget_monthly = req.budget_monthly_usd.map(|v| (v * 1_000_000.0) as i64);
    let allowed_tiers_json = req
        .allowed_tiers
        .as_ref()
        .map(|t| serde_json::to_string(t).unwrap());

    sqlx::query(
        r#"
        INSERT INTO api_keys
            (id, key_hash, name, budget_total_micro_usd, budget_daily_micro_usd,
             budget_monthly_micro_usd, max_tokens_per_request, allowed_tiers)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(&key_hash)
    .bind(&req.name)
    .bind(budget_total)
    .bind(budget_daily)
    .bind(budget_monthly)
    .bind(req.max_tokens_per_request)
    .bind(allowed_tiers_json)
    .execute(&state.db)
    .await?;

    tracing::info!(key_id = %id, name = %req.name, "API key created");

    Ok(Json(CreateApiKeyResponse {
        id,
        key: raw_key,
        name: req.name,
        created_at: chrono::Utc::now(),
    }))
}

pub async fn revoke_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let result = sqlx::query("UPDATE api_keys SET is_active = 0 WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Key {id} not found")));
    }

    tracing::info!(key_id = %id, "API key revoked");
    Ok(Json(serde_json::json!({ "id": id, "status": "revoked" })))
}

pub async fn delete_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let result = sqlx::query("DELETE FROM api_keys WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("Key {id} not found")));
    }

    Ok(Json(serde_json::json!({ "id": id, "status": "deleted" })))
}

// ── Cost Analytics ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CostQuery {
    pub days: Option<i64>,
    pub api_key_id: Option<String>,
}

pub async fn get_costs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<CostQuery>,
) -> Result<Json<CostSummary>> {
    let days = q.days.unwrap_or(30).max(1).min(365);
    let period = format!("Last {days} days");

    // Dynamic filter for key_id
    let key_filter = q
        .api_key_id
        .as_deref()
        .map(|_| " AND api_key_id = ?")
        .unwrap_or("");

    // Totals
    let total_row = sqlx::query(&format!(
        r#"
        SELECT
            COALESCE(SUM(cost_micro_usd), 0) as total_cost,
            COUNT(*) as total_requests,
            COALESCE(SUM(input_tokens + output_tokens), 0) as total_tokens
        FROM cost_records
        WHERE created_at >= datetime('now', '-{days} days')
        {key_filter}
        "#
    ))
    .fetch_one(&state.db)
    .await?;

    let total_cost_usd: f64 =
        total_row.try_get::<i64, _>("total_cost").unwrap_or(0) as f64 / 1_000_000.0;
    let total_requests: i64 = total_row.try_get("total_requests").unwrap_or(0);
    let total_tokens: i64 = total_row.try_get("total_tokens").unwrap_or(0);

    // By provider
    let provider_rows = sqlx::query(&format!(
        r#"
        SELECT provider,
               SUM(cost_micro_usd) as cost,
               COUNT(*) as requests
        FROM cost_records
        WHERE created_at >= datetime('now', '-{days} days')
        {key_filter}
        GROUP BY provider
        ORDER BY cost DESC
        "#
    ))
    .fetch_all(&state.db)
    .await?;

    let by_provider = provider_rows
        .iter()
        .map(|r| ProviderCostBreakdown {
            provider: r.get("provider"),
            cost_usd: r.try_get::<i64, _>("cost").unwrap_or(0) as f64 / 1_000_000.0,
            requests: r.try_get("requests").unwrap_or(0),
        })
        .collect();

    // By model
    let model_rows = sqlx::query(&format!(
        r#"
        SELECT model, provider,
               SUM(cost_micro_usd) as cost,
               COUNT(*) as requests,
               AVG(input_tokens) as avg_input,
               AVG(output_tokens) as avg_output
        FROM cost_records
        WHERE created_at >= datetime('now', '-{days} days')
        {key_filter}
        GROUP BY model, provider
        ORDER BY cost DESC
        "#
    ))
    .fetch_all(&state.db)
    .await?;

    let by_model = model_rows
        .iter()
        .map(|r| ModelCostBreakdown {
            model: r.get("model"),
            provider: r.get("provider"),
            cost_usd: r.try_get::<i64, _>("cost").unwrap_or(0) as f64 / 1_000_000.0,
            requests: r.try_get("requests").unwrap_or(0),
            avg_input_tokens: r.try_get::<f64, _>("avg_input").unwrap_or(0.0) as i64,
            avg_output_tokens: r.try_get::<f64, _>("avg_output").unwrap_or(0.0) as i64,
        })
        .collect();

    // Daily breakdown
    let daily_rows = sqlx::query(&format!(
        r#"
        SELECT strftime('%Y-%m-%d', created_at) as date,
               SUM(cost_micro_usd) as cost,
               COUNT(*) as requests
        FROM cost_records
        WHERE created_at >= datetime('now', '-{days} days')
        {key_filter}
        GROUP BY date
        ORDER BY date ASC
        "#
    ))
    .fetch_all(&state.db)
    .await?;

    let by_day = daily_rows
        .iter()
        .map(|r| DailyCost {
            date: r.get("date"),
            cost_usd: r.try_get::<i64, _>("cost").unwrap_or(0) as f64 / 1_000_000.0,
            requests: r.try_get("requests").unwrap_or(0),
        })
        .collect();

    Ok(Json(CostSummary {
        period,
        total_cost_usd,
        total_requests,
        total_tokens,
        by_provider,
        by_model,
        by_day,
    }))
}

// ── Token endpoint ────────────────────────────────────────────────────────────

pub async fn get_admin_token(
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>> {
    let provided_secret = req
        .get("secret")
        .and_then(|s| s.as_str())
        .ok_or_else(|| AppError::BadRequest("Missing 'secret' field".into()))?;

    if provided_secret != state.config.admin_jwt_secret {
        return Err(AppError::Unauthorized("Invalid admin secret".into()));
    }

    let token = crate::auth::issue_admin_token(&state.config.admin_jwt_secret)?;
    Ok(Json(serde_json::json!({ "token": token })))
}
