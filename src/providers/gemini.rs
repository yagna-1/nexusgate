use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, warn};

use crate::{
    error::{AppError, Result},
    models::{ProviderRequest, ProviderResponse, ProviderType},
};

use super::LlmProvider;

pub struct GeminiProvider {
    client: Client,
    api_key: String,
}

impl GeminiProvider {
    pub fn new(api_key: String, timeout_secs: u64) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(timeout_secs))
                .build()
                .expect("Failed to build HTTP client"),
            api_key,
        }
    }

    fn endpoint(&self, model_id: &str) -> String {
        format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model_id, self.api_key
        )
    }
}

// ── Request/Response types ────────────────────────────────────────────────────

#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiSystemInstruction>,
    #[serde(rename = "generationConfig")]
    generation_config: GeminiGenerationConfig,
}

#[derive(Serialize)]
struct GeminiContent {
    // Gemini uses "user" / "model" (not "assistant")
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize)]
struct GeminiPart {
    text: String,
}

#[derive(Serialize)]
struct GeminiSystemInstruction {
    parts: Vec<GeminiPart>,
}

#[derive(Serialize)]
struct GeminiGenerationConfig {
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: u32,
    temperature: f32,
}

#[derive(Deserialize, Debug)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsage>,
    #[serde(rename = "modelVersion")]
    model_version: Option<String>,
}

#[derive(Deserialize, Debug)]
struct GeminiCandidate {
    content: GeminiResponseContent,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
struct GeminiResponseContent {
    parts: Vec<GeminiResponsePart>,
}

#[derive(Deserialize, Debug)]
struct GeminiResponsePart {
    text: Option<String>,
}

#[derive(Deserialize, Debug)]
struct GeminiUsage {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<u32>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<u32>,
}

// ── Provider implementation ───────────────────────────────────────────────────

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn complete(&self, req: &ProviderRequest) -> Result<ProviderResponse> {
        // Separate system from conversation messages
        let system_text: Option<String> = {
            let sys: Vec<String> = req
                .messages
                .iter()
                .filter(|m| m.role == "system")
                .map(|m| m.text_content())
                .collect();
            if sys.is_empty() { None } else { Some(sys.join("\n")) }
        };

        let contents: Vec<GeminiContent> = req
            .messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| GeminiContent {
                // Gemini calls the assistant role "model"
                role: if m.role == "assistant" {
                    "model".into()
                } else {
                    "user".into()
                },
                parts: vec![GeminiPart {
                    text: m.text_content(),
                }],
            })
            .collect();

        let body = GeminiRequest {
            contents,
            system_instruction: system_text.map(|text| GeminiSystemInstruction {
                parts: vec![GeminiPart { text }],
            }),
            generation_config: GeminiGenerationConfig {
                max_output_tokens: req.max_tokens,
                temperature: req.temperature,
            },
        };

        debug!(model = %req.model.model_id, "Sending request to Gemini");

        let http_resp = self
            .client
            .post(self.endpoint(&req.model.model_id))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::ProviderError {
                provider: "gemini".into(),
                message: format!("HTTP error: {e}"),
            })?;

        let status = http_resp.status();

        if status == 429 {
            let retry_after = http_resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            warn!("Gemini rate limit hit");
            return Err(AppError::ProviderError {
                provider: "gemini".into(),
                message: format!("RATE_LIMITED:{}", retry_after.unwrap_or(60)),
            });
        }

        if !status.is_success() {
            let body_text = http_resp
                .text()
                .await
                .unwrap_or_else(|_| format!("HTTP {status}"));
            return Err(AppError::ProviderError {
                provider: "gemini".into(),
                message: body_text,
            });
        }

        let gem_resp = http_resp
            .json::<GeminiResponse>()
            .await
            .map_err(|e| AppError::ProviderError {
                provider: "gemini".into(),
                message: format!("Parse error: {e}"),
            })?;

        let candidate = gem_resp
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| AppError::ProviderError {
                provider: "gemini".into(),
                message: "Empty candidates in response".into(),
            })?;

        let content = candidate
            .content
            .parts
            .into_iter()
            .filter_map(|p| p.text)
            .collect::<Vec<_>>()
            .join("");

        let usage = gem_resp.usage_metadata.unwrap_or(GeminiUsage {
            prompt_token_count: None,
            candidates_token_count: None,
        });

        Ok(ProviderResponse {
            provider: ProviderType::Gemini,
            model_used: gem_resp
                .model_version
                .unwrap_or_else(|| req.model.model_id.clone()),
            content,
            input_tokens: usage.prompt_token_count.unwrap_or(0),
            output_tokens: usage.candidates_token_count.unwrap_or(0),
            finish_reason: candidate
                .finish_reason
                .unwrap_or_else(|| "STOP".into()),
        })
    }
}
