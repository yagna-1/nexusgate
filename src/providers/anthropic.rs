use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, warn};

use crate::{
    error::{AppError, Result},
    models::{ChatMessage, ProviderRequest, ProviderResponse, ProviderType},
};

use super::LlmProvider;

pub struct AnthropicProvider {
    client: Client,
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String, timeout_secs: u64) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(timeout_secs))
                .build()
                .expect("Failed to build HTTP client"),
            api_key,
        }
    }

    /// Extract system message(s) from message array (Anthropic requires them separately)
    fn extract_system(messages: &[ChatMessage]) -> (Option<String>, Vec<&ChatMessage>) {
        let system = messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| m.text_content())
            .collect::<Vec<_>>()
            .join("\n");

        let non_system: Vec<&ChatMessage> = messages
            .iter()
            .filter(|m| m.role != "system")
            .collect();

        (
            if system.is_empty() { None } else { Some(system) },
            non_system,
        )
    }
}

// ── Request/Response types ────────────────────────────────────────────────────

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    messages: Vec<AnthropicMessage<'a>>,
    max_tokens: u32,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
}

#[derive(Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Deserialize, Debug)]
struct AnthropicResponse {
    model: String,
    content: Vec<AnthropicContent>,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Deserialize, Debug)]
struct AnthropicContent {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
}

#[derive(Deserialize, Debug)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Deserialize, Debug)]
struct AnthropicError {
    error: AnthropicErrorBody,
}

#[derive(Deserialize, Debug)]
struct AnthropicErrorBody {
    message: String,
}

// ── Provider implementation ───────────────────────────────────────────────────

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete(&self, req: &ProviderRequest) -> Result<ProviderResponse> {
        let (system, non_system_msgs) = Self::extract_system(&req.messages);

        let messages: Vec<AnthropicMessage> = non_system_msgs
            .iter()
            .map(|m| AnthropicMessage {
                // Anthropic uses "user" / "assistant" (same as OpenAI)
                role: &m.role,
                content: m.text_content(),
            })
            .collect();

        let body = AnthropicRequest {
            model: &req.model.model_id,
            messages,
            max_tokens: req.max_tokens,
            temperature: req.temperature,
            system,
        };

        debug!(model = %req.model.model_id, "Sending request to Anthropic");

        let http_resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ProviderError {
                provider: "anthropic".into(),
                message: format!("HTTP error: {e}"),
            })?;

        let status = http_resp.status();

        if status == 429 {
            let retry_after = http_resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            warn!("Anthropic rate limit hit");
            return Err(AppError::ProviderError {
                provider: "anthropic".into(),
                message: format!("RATE_LIMITED:{}", retry_after.unwrap_or(60)),
            });
        }

        if !status.is_success() {
            let err_msg = http_resp
                .json::<AnthropicError>()
                .await
                .map(|e| e.error.message)
                .unwrap_or_else(|_| format!("HTTP {status}"));
            return Err(AppError::ProviderError {
                provider: "anthropic".into(),
                message: err_msg,
            });
        }

        let ant_resp = http_resp
            .json::<AnthropicResponse>()
            .await
            .map_err(|e| AppError::ProviderError {
                provider: "anthropic".into(),
                message: format!("Parse error: {e}"),
            })?;

        let content = ant_resp
            .content
            .into_iter()
            .filter(|c| c.content_type == "text")
            .filter_map(|c| c.text)
            .collect::<Vec<_>>()
            .join("");

        Ok(ProviderResponse {
            provider: ProviderType::Anthropic,
            model_used: ant_resp.model,
            content,
            input_tokens: ant_resp.usage.input_tokens,
            output_tokens: ant_resp.usage.output_tokens,
            finish_reason: ant_resp.stop_reason.unwrap_or_else(|| "end_turn".into()),
        })
    }
}
