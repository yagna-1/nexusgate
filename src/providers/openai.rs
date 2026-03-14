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

pub struct OpenAIProvider {
    client: Client,
    api_key: String,
    base_url: String,
}

impl OpenAIProvider {
    pub fn new(api_key: String, timeout_secs: u64) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(timeout_secs))
                .build()
                .expect("Failed to build HTTP client"),
            api_key,
            base_url: "https://api.openai.com".into(),
        }
    }
}

// ── Request/Response types ────────────────────────────────────────────────────

#[derive(Serialize)]
struct OpenAIRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    max_tokens: u32,
    temperature: f32,
}

#[derive(Deserialize, Debug)]
struct OpenAIResponse {
    choices: Vec<OAIChoice>,
    usage: OAIUsage,
    model: String,
}

#[derive(Deserialize, Debug)]
struct OAIChoice {
    message: OAIMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
struct OAIMessage {
    content: Option<String>,
}

#[derive(Deserialize, Debug)]
struct OAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

#[derive(Deserialize, Debug)]
struct OAIError {
    error: OAIErrorBody,
}

#[derive(Deserialize, Debug)]
struct OAIErrorBody {
    message: String,
}

// ── Provider implementation ───────────────────────────────────────────────────

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn complete(&self, req: &ProviderRequest) -> Result<ProviderResponse> {
        let body = OpenAIRequest {
            model: &req.model.model_id,
            messages: &req.messages,
            max_tokens: req.max_tokens,
            temperature: req.temperature,
        };

        debug!(model = %req.model.model_id, "Sending request to OpenAI");

        let http_resp = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ProviderError {
                provider: "openai".into(),
                message: format!("HTTP error: {e}"),
            })?;

        let status = http_resp.status();

        // Rate limit
        if status == 429 {
            let retry_after = http_resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            warn!(
                retry_after_secs = ?retry_after,
                "OpenAI rate limit hit"
            );
            return Err(AppError::ProviderError {
                provider: "openai".into(),
                message: format!("RATE_LIMITED:{}", retry_after.unwrap_or(60)),
            });
        }

        if !status.is_success() {
            let err_body = http_resp
                .json::<OAIError>()
                .await
                .map(|e| e.error.message)
                .unwrap_or_else(|_| format!("HTTP {status}"));
            return Err(AppError::ProviderError {
                provider: "openai".into(),
                message: err_body,
            });
        }

        let oai_resp =
            http_resp
                .json::<OpenAIResponse>()
                .await
                .map_err(|e| AppError::ProviderError {
                    provider: "openai".into(),
                    message: format!("Failed to parse response: {e}"),
                })?;

        let choice =
            oai_resp
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| AppError::ProviderError {
                    provider: "openai".into(),
                    message: "Empty choices in response".into(),
                })?;

        Ok(ProviderResponse {
            provider: ProviderType::OpenAI,
            model_used: oai_resp.model,
            content: choice.message.content.unwrap_or_default(),
            input_tokens: oai_resp.usage.prompt_tokens,
            output_tokens: oai_resp.usage.completion_tokens,
            finish_reason: choice.finish_reason.unwrap_or_else(|| "stop".into()),
        })
    }
}
