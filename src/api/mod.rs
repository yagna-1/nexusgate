pub mod admin;
pub mod chat;
pub mod health;
pub mod mcp;

use axum::{
    extract::State,
    http::HeaderValue,
    middleware,
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::json;
use std::sync::Arc;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};

use crate::{auth, state::AppState};

pub fn router(state: Arc<AppState>) -> Router {
    let cors_origins = state.config.cors_origins.trim();
    let cors = if cors_origins == "*" {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let origins: Vec<HeaderValue> = cors_origins
            .split(',')
            .filter_map(|origin| HeaderValue::from_str(origin.trim()).ok())
            .collect();

        if origins.is_empty() {
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
        } else {
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods(Any)
                .allow_headers(Any)
        }
    };

    // OpenAI-compatible completions (requires a valid NexusGate API key)
    let api_routes = Router::new()
        .route("/v1/chat/completions", post(chat::chat_completions))
        .route("/mcp/tools/call", post(mcp::mcp_tools_call))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_api_key,
        ));

    // Admin management routes (requires admin JWT)
    let admin_routes = Router::new()
        .route("/admin/keys", get(admin::list_keys).post(admin::create_key))
        .route("/admin/keys/:id/revoke", post(admin::revoke_key))
        .route("/admin/keys/:id", delete(admin::delete_key))
        .route("/admin/costs", get(admin::get_costs))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_admin,
        ));

    // Token issuance (unprotected — takes admin secret in body)
    let auth_routes = Router::new().route("/admin/token", post(admin::get_admin_token));

    // Health (public)
    let health_routes = Router::new()
        .route("/health", get(health::health))
        .route("/.well-known/agent.json", get(agent_card));

    // Dashboard (public)
    let dashboard_routes = Router::new().route(
        "/",
        get(|| async {
            let html = std::fs::read_to_string("/app/dashboard/index.html")
                .or_else(|_| std::fs::read_to_string("./dashboard/index.html"))
                .unwrap_or_else(|_| "<h1>NexusGate</h1><p>Dashboard not found</p>".into());
            axum::response::Html(html)
        }),
    );

    Router::new()
        .merge(api_routes)
        .merge(admin_routes)
        .merge(auth_routes)
        .merge(health_routes)
        .merge(dashboard_routes)
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

async fn agent_card(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({
        "name": "nexusgate",
        "description": "NexusGate MCP/LLM routing gateway",
        "url": state.config.public_base_url,
        "capabilities": {
            "mcp_proxy": true,
            "chat_completions": true,
            "workflow_budget_tracking": true
        },
        "endpoints": {
            "chat_completions": "/v1/chat/completions",
            "mcp_tools_call": "/mcp/tools/call",
            "health": "/health"
        }
    }))
}
