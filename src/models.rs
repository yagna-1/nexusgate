use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════════════════
// OpenAI-compatible Request / Response (drop-in replacement API)
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: serde_json::Value, // String or array of content parts
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn text_content(&self) -> String {
        match &self.content {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
pub struct ChatCompletionRequest {
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<serde_json::Value>,
    /// NexusGate-specific routing options (ignored by OpenAI SDK)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nexusgate: Option<NexusGateOptions>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NexusGateOptions {
    /// Preferred tier: "economy" | "standard" | "premium"
    pub tier: Option<String>,
    /// Allow fallback to other providers/tiers on failure (default: true)
    pub fallback: Option<bool>,
    /// Override per-request budget in USD (e.g. 0.05 = 5 cents max)
    pub max_cost_usd: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nexusgate: Option<NexusGateMeta>,
}

#[derive(Debug, Serialize)]
pub struct Choice {
    pub index: i32,
    pub message: ChatMessage,
    pub finish_reason: String,
    pub logprobs: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Appended to every response — tells client exactly what happened
#[derive(Debug, Serialize)]
pub struct NexusGateMeta {
    pub provider: String,
    pub model_used: String,
    pub tier: String,
    pub cost_usd: f64,
    pub fallback_used: bool,
    pub fallback_count: u32,
    pub request_id: String,
}

// ═══════════════════════════════════════════════════════════════
// Provider / Model catalog
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    OpenAI,
    Anthropic,
    Gemini,
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderType::OpenAI => write!(f, "openai"),
            ProviderType::Anthropic => write!(f, "anthropic"),
            ProviderType::Gemini => write!(f, "gemini"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    Economy,
    Standard,
    Premium,
}

impl std::fmt::Display for ModelTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelTier::Economy => write!(f, "economy"),
            ModelTier::Standard => write!(f, "standard"),
            ModelTier::Premium => write!(f, "premium"),
        }
    }
}

impl ModelTier {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "economy" => Some(ModelTier::Economy),
            "standard" => Some(ModelTier::Standard),
            "premium" => Some(ModelTier::Premium),
            _ => None,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// ID to send in provider API call
    pub model_id: String,
    /// Canonical alias users can reference (e.g. "gpt-4o")
    pub alias: Option<String>,
    pub display_name: String,
    pub provider: ProviderType,
    pub tier: ModelTier,
    /// USD per 1M input tokens
    pub input_cost_per_million: f64,
    /// USD per 1M output tokens
    pub output_cost_per_million: f64,
    pub context_window: u32,
    pub max_output_tokens: u32,
}

impl ModelInfo {
    /// Calculate cost in micro-USD for given token counts
    pub fn cost_micro_usd(&self, input_tokens: u32, output_tokens: u32) -> i64 {
        let input_cost = (input_tokens as f64 / 1_000_000.0) * self.input_cost_per_million;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * self.output_cost_per_million;
        ((input_cost + output_cost) * 1_000_000.0) as i64
    }
}

/// Full model catalog — single source of truth for pricing and routing
pub fn model_catalog() -> Vec<ModelInfo> {
    vec![
        // ── Economy ──────────────────────────────────────────────
        ModelInfo {
            model_id: "gpt-4o-mini".into(),
            alias: Some("gpt-4o-mini".into()),
            display_name: "GPT-4o Mini".into(),
            provider: ProviderType::OpenAI,
            tier: ModelTier::Economy,
            input_cost_per_million: 0.15,
            output_cost_per_million: 0.60,
            context_window: 128_000,
            max_output_tokens: 16_384,
        },
        ModelInfo {
            model_id: "claude-haiku-4-5-20251001".into(),
            alias: Some("claude-haiku".into()),
            display_name: "Claude Haiku".into(),
            provider: ProviderType::Anthropic,
            tier: ModelTier::Economy,
            input_cost_per_million: 0.25,
            output_cost_per_million: 1.25,
            context_window: 200_000,
            max_output_tokens: 8_192,
        },
        ModelInfo {
            model_id: "gemini-2.0-flash".into(),
            alias: Some("gemini-flash".into()),
            display_name: "Gemini 2.0 Flash".into(),
            provider: ProviderType::Gemini,
            tier: ModelTier::Economy,
            input_cost_per_million: 0.075,
            output_cost_per_million: 0.30,
            context_window: 1_000_000,
            max_output_tokens: 8_192,
        },
        // ── Standard ─────────────────────────────────────────────
        ModelInfo {
            model_id: "gpt-4o".into(),
            alias: Some("gpt-4o".into()),
            display_name: "GPT-4o".into(),
            provider: ProviderType::OpenAI,
            tier: ModelTier::Standard,
            input_cost_per_million: 2.50,
            output_cost_per_million: 10.0,
            context_window: 128_000,
            max_output_tokens: 16_384,
        },
        ModelInfo {
            model_id: "claude-sonnet-4-6".into(),
            alias: Some("claude-sonnet".into()),
            display_name: "Claude Sonnet".into(),
            provider: ProviderType::Anthropic,
            tier: ModelTier::Standard,
            input_cost_per_million: 3.0,
            output_cost_per_million: 15.0,
            context_window: 200_000,
            max_output_tokens: 8_192,
        },
        ModelInfo {
            model_id: "gemini-2.0-pro-exp".into(),
            alias: Some("gemini-pro".into()),
            display_name: "Gemini 2.0 Pro".into(),
            provider: ProviderType::Gemini,
            tier: ModelTier::Standard,
            input_cost_per_million: 1.25,
            output_cost_per_million: 5.0,
            context_window: 2_000_000,
            max_output_tokens: 8_192,
        },
        // ── Premium ──────────────────────────────────────────────
        ModelInfo {
            model_id: "claude-opus-4-6".into(),
            alias: Some("claude-opus".into()),
            display_name: "Claude Opus".into(),
            provider: ProviderType::Anthropic,
            tier: ModelTier::Premium,
            input_cost_per_million: 15.0,
            output_cost_per_million: 75.0,
            context_window: 200_000,
            max_output_tokens: 8_192,
        },
        ModelInfo {
            model_id: "o3".into(),
            alias: Some("o3".into()),
            display_name: "OpenAI o3".into(),
            provider: ProviderType::OpenAI,
            tier: ModelTier::Premium,
            input_cost_per_million: 10.0,
            output_cost_per_million: 40.0,
            context_window: 200_000,
            max_output_tokens: 100_000,
        },
    ]
}

// ═══════════════════════════════════════════════════════════════
// Internal provider communication types
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub model: ModelInfo,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub provider: ProviderType,
    pub model_used: String,
    pub content: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub finish_reason: String,
}

// ═══════════════════════════════════════════════════════════════
// Database types
// ═══════════════════════════════════════════════════════════════

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
pub struct ApiKey {
    pub id: String,
    pub key_hash: String,
    pub name: String,
    pub budget_total_micro_usd: Option<i64>,
    pub budget_daily_micro_usd: Option<i64>,
    pub budget_monthly_micro_usd: Option<i64>,
    pub max_tokens_per_request: Option<i64>,
    pub allowed_tiers: Option<String>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
pub struct CostRecord {
    pub id: String,
    pub api_key_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_micro_usd: i64,
    pub request_id: String,
    pub fallback_used: bool,
    pub created_at: DateTime<Utc>,
}

// ═══════════════════════════════════════════════════════════════
// Admin API types
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub budget_total_usd: Option<f64>,
    pub budget_daily_usd: Option<f64>,
    pub budget_monthly_usd: Option<f64>,
    pub max_tokens_per_request: Option<i64>,
    pub allowed_tiers: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct CreateApiKeyResponse {
    pub id: String,
    /// Raw key — returned ONCE, never stored
    pub key: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ApiKeyInfo {
    pub id: String,
    pub name: String,
    pub is_active: bool,
    pub budget_total_usd: Option<f64>,
    pub budget_daily_usd: Option<f64>,
    pub budget_monthly_usd: Option<f64>,
    pub max_tokens_per_request: Option<i64>,
    pub allowed_tiers: Option<Vec<String>>,
    pub created_at: String,
    pub last_used_at: Option<String>,
    // Populated from Redis
    pub spent_today_usd: f64,
    pub spent_this_month_usd: f64,
    pub spent_total_usd: f64,
}

#[derive(Debug, Serialize)]
pub struct CostSummary {
    pub period: String,
    pub total_cost_usd: f64,
    pub total_requests: i64,
    pub total_tokens: i64,
    pub by_provider: Vec<ProviderCostBreakdown>,
    pub by_model: Vec<ModelCostBreakdown>,
    pub by_day: Vec<DailyCost>,
}

#[derive(Debug, Serialize)]
pub struct ProviderCostBreakdown {
    pub provider: String,
    pub cost_usd: f64,
    pub requests: i64,
}

#[derive(Debug, Serialize)]
pub struct ModelCostBreakdown {
    pub model: String,
    pub provider: String,
    pub cost_usd: f64,
    pub requests: i64,
    pub avg_input_tokens: i64,
    pub avg_output_tokens: i64,
}

#[derive(Debug, Serialize)]
pub struct DailyCost {
    pub date: String,
    pub cost_usd: f64,
    pub requests: i64,
}

#[derive(Debug, Serialize)]
pub struct ProviderStatus {
    pub provider: String,
    pub configured: bool,
    pub available: bool,
    pub rate_limited: bool,
    pub rate_limit_reset_secs: Option<i64>,
    pub requests_today: i64,
    pub cost_today_usd: f64,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub providers: Vec<ProviderStatus>,
    pub uptime_secs: u64,
}

/// Attached to every request by auth middleware
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub key_id: String,
    pub key_name: String,
    pub budget_total_micro_usd: Option<i64>,
    pub budget_daily_micro_usd: Option<i64>,
    pub budget_monthly_micro_usd: Option<i64>,
    pub max_tokens_per_request: Option<i64>,
    pub allowed_tiers: Option<Vec<String>>,
}
