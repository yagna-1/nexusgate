use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    Extension, Json,
};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

use crate::{
    budget::BudgetEnforcer,
    error::{AppError, Result},
    models::AuthContext,
    state::AppState,
};

pub async fn mcp_tools_call(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<(StatusCode, Json<Value>)> {
    let workflow_id = extract_workflow_id(&headers, &payload);
    let estimated_cost = state.config.mcp_default_estimated_cost_micro_usd.max(0);

    let budget = BudgetEnforcer::new(state.redis.clone(), state.db.clone());
    budget.check(&auth, estimated_cost).await?;

    if let Some(ref wf) = workflow_id {
        budget
            .check_workflow(
                wf,
                auth.budget_workflow_daily_micro_usd,
                auth.budget_workflow_monthly_micro_usd,
                estimated_cost,
            )
            .await?;
    }

    let upstream_url = state
        .config
        .mcp_upstream_url
        .as_deref()
        .ok_or_else(|| AppError::NoProviders("MCP_UPSTREAM_URL is not configured".into()))?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(state.config.request_timeout_secs))
        .build()
        .map_err(|e| AppError::Internal(format!("Failed to build HTTP client: {e}")))?;

    let mut req_builder = client
        .post(resolve_mcp_tools_call_url(upstream_url))
        .json(&payload);

    if let Some(token) = state.config.mcp_upstream_bearer_token.as_deref() {
        req_builder = req_builder.bearer_auth(token);
    }

    if let Some(ref wf) = workflow_id {
        req_builder = req_builder.header("X-Workflow-ID", wf);
    }

    req_builder = req_builder.header("X-NexusGate-Key-ID", &auth.key_id);

    if let Some(v) = headers.get("X-AstraGraph-Policy-ID") {
        req_builder = req_builder.header("X-AstraGraph-Policy-ID", v.clone());
    }

    let response = req_builder
        .send()
        .await
        .map_err(|e| AppError::ProviderError {
            provider: "mcp".into(),
            message: format!("Upstream MCP request failed: {e}"),
        })?;

    let status = response.status();
    let body_text = response.text().await.map_err(|e| AppError::ProviderError {
        provider: "mcp".into(),
        message: format!("Failed to read MCP upstream response: {e}"),
    })?;

    let body_json = serde_json::from_str::<Value>(&body_text).unwrap_or_else(|_| {
        serde_json::json!({
            "raw": body_text
        })
    });

    if !status.is_success() {
        return Err(AppError::ProviderError {
            provider: "mcp".into(),
            message: format!(
                "Upstream MCP returned {status}: {}",
                truncate(&body_text, 512)
            ),
        });
    }

    let request_id = Uuid::new_v4().to_string();
    let tool_name = payload
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("mcp.tools.call")
        .to_string();
    let key_id = auth.key_id.clone();
    let wf = workflow_id.clone();
    let budget_clone = BudgetEnforcer::new(state.redis.clone(), state.db.clone());
    tokio::spawn(async move {
        let _ = budget_clone
            .record_cost_for(
                &key_id,
                wf.as_deref(),
                "mcp",
                &tool_name,
                0,
                0,
                estimated_cost,
                &request_id,
                false,
            )
            .await;
    });

    let axum_status = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK);
    Ok((axum_status, Json(body_json)))
}

fn resolve_mcp_tools_call_url(base: &str) -> String {
    if base.ends_with("/mcp/tools/call") {
        return base.to_string();
    }
    format!("{}/mcp/tools/call", base.trim_end_matches('/'))
}

fn extract_workflow_id(headers: &HeaderMap, payload: &Value) -> Option<String> {
    if let Some(value) = header_to_string(headers.get("X-Workflow-ID")) {
        if !value.is_empty() {
            return Some(value);
        }
    }

    let from_params = payload
        .get("params")
        .and_then(|p| p.get("arguments"))
        .and_then(|a| a.get("workflow_id"))
        .and_then(|v| v.as_str());
    if let Some(value) = from_params {
        if !value.trim().is_empty() {
            return Some(value.trim().to_string());
        }
    }

    payload
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn header_to_string(value: Option<&HeaderValue>) -> Option<String> {
    value
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().to_string())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars().take(max) {
        out.push(c);
    }
    out.push_str("...");
    out
}
