use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::sync::Arc;

use crate::{
    error::{AppError, Result},
    models::AuthContext,
    state::AppState,
};

/// Middleware: validate Bearer token, inject AuthContext into request extensions
pub async fn require_api_key(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Result<Response> {
    let raw_key = extract_bearer(request.headers())?;
    let key_hash = hash_key(raw_key);

    let row = sqlx::query(
        r#"
        SELECT id, name, budget_total_micro_usd, budget_daily_micro_usd,
               budget_monthly_micro_usd, max_tokens_per_request,
               allowed_tiers, is_active
        FROM api_keys
        WHERE key_hash = ?
        "#,
    )
    .bind(&key_hash)
    .fetch_optional(&state.db)
    .await?;

    let row = row.ok_or_else(|| AppError::Unauthorized("Invalid API key".into()))?;

    let is_active: bool = row.get::<bool, _>("is_active");
    if !is_active {
        return Err(AppError::Unauthorized("API key is disabled".into()));
    }

    let allowed_tiers: Option<Vec<String>> = row
        .try_get::<Option<String>, _>("allowed_tiers")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok());

    let ctx = AuthContext {
        key_id: row.get("id"),
        key_name: row.get("name"),
        budget_total_micro_usd: row.try_get("budget_total_micro_usd").ok().flatten(),
        budget_daily_micro_usd: row.try_get("budget_daily_micro_usd").ok().flatten(),
        budget_monthly_micro_usd: row.try_get("budget_monthly_micro_usd").ok().flatten(),
        max_tokens_per_request: row.try_get("max_tokens_per_request").ok().flatten(),
        allowed_tiers,
    };

    // Update last_used_at asynchronously (best-effort, never block the request)
    let db = state.db.clone();
    let key_id = ctx.key_id.clone();
    tokio::spawn(async move {
        let _ = sqlx::query(
            "UPDATE api_keys SET last_used_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?"
        )
        .bind(&key_id)
        .execute(&db)
        .await;
    });

    request.extensions_mut().insert(ctx);
    Ok(next.run(request).await)
}

fn extract_bearer(headers: &axum::http::HeaderMap) -> Result<&str> {
    headers
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(|| AppError::Unauthorized("Missing or invalid Authorization header".into()))
}

pub fn hash_key(raw_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}

// ── Admin JWT ────────────────────────────────────────────────────────────────

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminClaims {
    pub sub: String,
    pub role: String,
    pub iat: i64,
    pub exp: i64,
}

pub fn issue_admin_token(secret: &str) -> Result<String> {
    let now = chrono::Utc::now();
    let claims = AdminClaims {
        sub: "admin".into(),
        role: "admin".into(),
        iat: now.timestamp(),
        exp: (now + chrono::Duration::days(365)).timestamp(),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("Token creation failed: {e}")))
}

pub fn verify_admin_token(token: &str, secret: &str) -> Result<AdminClaims> {
    // FIX: was calling set_audience twice — second call overwrote first to empty vec.
    // Now explicitly disable audience validation (we don't set aud in claims).
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_aud = false;
    decode::<AdminClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|td| td.claims)
    .map_err(|e| AppError::Unauthorized(format!("Invalid admin token: {e}")))
}

/// Middleware for admin routes
pub async fn require_admin(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response> {
    let token = extract_bearer(request.headers())?;
    verify_admin_token(token, &state.config.admin_jwt_secret)?;
    Ok(next.run(request).await)
}

/// Generate a cryptographically random API key with a recognisable prefix
pub fn generate_api_key() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let random_bytes: Vec<u8> = (0..32).map(|_| rng.gen::<u8>()).collect();
    format!("ng-{}", hex::encode(random_bytes))
}
