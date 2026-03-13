use axum::{extract::State, Extension, Json};
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    budget::BudgetEnforcer,
    error::{AppError, Result},
    models::{
        AuthContext, ChatCompletionRequest, ChatCompletionResponse, ChatMessage,
        Choice, ModelInfo, NexusGateMeta, ProviderRequest, ProviderType, Usage,
    },
    providers::{
        anthropic::AnthropicProvider, gemini::GeminiProvider, openai::OpenAIProvider,
        LlmProvider,
    },
    rate_limit::RateLimiter,
    router::ModelRouter,
    state::AppState,
};

pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<Json<ChatCompletionResponse>> {
    let request_id = Uuid::new_v4().to_string();
    let ng_opts = req.nexusgate.as_ref();

    // ── Token limits ──────────────────────────────────────────────────────────
    let max_tokens = req
        .max_tokens
        .unwrap_or(state.config.default_max_tokens)
        .min(
            auth.max_tokens_per_request
                .map(|t| t as u32)
                .unwrap_or(u32::MAX),
        );

    // ── Budget enforcement ────────────────────────────────────────────────────
    let budget = BudgetEnforcer::new(state.redis.clone(), state.db.clone());

    // Per-request override from nexusgate options
    let per_req_limit_micro = ng_opts
        .and_then(|o| o.max_cost_usd)
        .map(|usd| (usd * 1_000_000.0) as i64);

    // Conservative upper bound: max_tokens * $0.075 per 1K = 75 micro-USD per token
    let estimated_cost = (max_tokens as i64) * 75;

    budget.check(&auth, estimated_cost).await?;

    // ── Model routing ─────────────────────────────────────────────────────────
    let router = ModelRouter::new();
    let rate_limiter = RateLimiter::new(state.redis.clone());

    let preferred_tier = ng_opts.and_then(|o| o.tier.as_deref());
    let allow_fallback = ng_opts.and_then(|o| o.fallback).unwrap_or(true);

    let initial_model = router
        .resolve_initial_model(
            req.model.as_deref(),
            preferred_tier,
            &auth,
            &state.config,
        )
        .ok_or_else(|| {
            AppError::NoProviders(
                "No models available for your tier/provider configuration".into(),
            )
        })?;

    router.log_selection(&initial_model, "initial selection");

    // ── Build fallback chain ──────────────────────────────────────────────────
    let temperature = req.temperature.unwrap_or(0.7);
    let mut models_to_try = vec![initial_model.clone()];

    if allow_fallback {
        // FIX: collect rate-limited providers with proper async awaits
        
        let mut rate_limited: Vec<ProviderType> = Vec::new();
        for p in [ProviderType::OpenAI, ProviderType::Anthropic, ProviderType::Gemini] {
            if rate_limiter.is_rate_limited(&p).await {
                rate_limited.push(p);
            }
        }

        let chain = router.fallback_chain(&initial_model, &state.config, &auth, &rate_limited);
        models_to_try.extend(chain);
    }

    models_to_try.truncate(state.config.max_fallback_attempts);

    let provider_req = ProviderRequest {
        model: initial_model.clone(),
        messages: req.messages.clone(),
        max_tokens,
        temperature,
    };

    let mut last_error: Option<AppError> = None;
    let mut fallback_count: u32 = 0;

    // ── Dispatch with fallback loop ───────────────────────────────────────────
    for (attempt, model) in models_to_try.iter().enumerate() {
        if attempt > 0 {
            fallback_count += 1;
            warn!(
                from = %models_to_try[attempt - 1].model_id,
                to = %model.model_id,
                "Falling back to next model"
            );
        }

        let this_req = ProviderRequest {
            model: model.clone(),
            max_tokens: provider_req.max_tokens.min(model.max_output_tokens),
            ..provider_req.clone()
        };

        let provider: Box<dyn LlmProvider> = build_provider(model, &state)?;

        match provider.complete(&this_req).await {
            Ok(resp) => {
                let cost_micro_usd = model.cost_micro_usd(resp.input_tokens, resp.output_tokens);

                // Enforce per-request cap if set
                if let Some(cap) = per_req_limit_micro {
                    if cost_micro_usd > cap {
                        warn!(cost_micro_usd, cap, "Request exceeded per-request cost cap");
                        return Err(AppError::BudgetExceeded(format!(
                            "Request cost ${:.6} exceeded per-request cap ${:.6}",
                            cost_micro_usd as f64 / 1_000_000.0,
                            cap as f64 / 1_000_000.0,
                        )));
                    }
                }

                // Record cost asynchronously (best-effort, never fail request on this)
                let budget_clone = BudgetEnforcer::new(state.redis.clone(), state.db.clone());
                let key_id = auth.key_id.clone();
                let provider_name = resp.provider.to_string();
                let model_id = resp.model_used.clone();
                let input_tok = resp.input_tokens;
                let output_tok = resp.output_tokens;
                let req_id = request_id.clone();
                let fb = fallback_count > 0;

                tokio::spawn(async move {
                    let _ = budget_clone
                        .record_cost(&key_id, &provider_name, &model_id,
                                     input_tok, output_tok, cost_micro_usd, &req_id, fb)
                        .await;
                });

                // Clear rate limit on success (provider recovered)
                let _ = rate_limiter.clear_rate_limit(&model.provider).await;

                info!(
                    request_id = %request_id,
                    provider = %resp.provider,
                    model = %model.model_id,
                    model_used = %resp.model_used,
                    key_name = %auth.key_name,
                    input_tokens = resp.input_tokens,
                    output_tokens = resp.output_tokens,
                    cost_micro_usd,
                    fallback_count,
                    "Request completed"
                );

                return Ok(Json(build_response(
                    request_id, model, &resp.model_used, &resp.content, &resp.finish_reason,
                    resp.input_tokens, resp.output_tokens, cost_micro_usd, fallback_count,
                )));
            }

            Err(e) => {
                // Mark provider rate-limited if 429 was the cause
                if let AppError::ProviderError { ref message, .. } = e {
                    if message.starts_with("RATE_LIMITED:") {
                        let secs: u64 = message
                            .trim_start_matches("RATE_LIMITED:")
                            .parse()
                            .unwrap_or(60);
                        let _ = rate_limiter.mark_rate_limited(&model.provider, Some(secs)).await;
                    }
                }
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or(AppError::AllProvidersRateLimited))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_provider(model: &ModelInfo, state: &AppState) -> Result<Box<dyn LlmProvider>> {
    let timeout = state.config.request_timeout_secs;
    match model.provider {
        ProviderType::OpenAI => {
            let key = state.config.openai_api_key.clone()
                .ok_or_else(|| AppError::NoProviders("OpenAI API key not configured".into()))?;
            Ok(Box::new(OpenAIProvider::new(key, timeout)))
        }
        ProviderType::Anthropic => {
            let key = state.config.anthropic_api_key.clone()
                .ok_or_else(|| AppError::NoProviders("Anthropic API key not configured".into()))?;
            Ok(Box::new(AnthropicProvider::new(key, timeout)))
        }
        ProviderType::Gemini => {
            let key = state.config.gemini_api_key.clone()
                .ok_or_else(|| AppError::NoProviders("Gemini API key not configured".into()))?;
            Ok(Box::new(GeminiProvider::new(key, timeout)))
        }
    }
}

fn build_response(
    request_id: String,
    model: &ModelInfo,
    model_used: &str,
    content: &str,
    finish_reason: &str,
    input_tokens: u32,
    output_tokens: u32,
    cost_micro_usd: i64,
    fallback_count: u32,
) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id: format!("chatcmpl-{request_id}"),
        object: "chat.completion".into(),
        created: chrono::Utc::now().timestamp(),
        model: model_used.to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".into(),
                content: serde_json::Value::String(content.into()),
                name: None,
            },
            finish_reason: finish_reason.into(),
            logprobs: None,
        }],
        usage: Usage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            total_tokens: input_tokens + output_tokens,
        },
        nexusgate: Some(NexusGateMeta {
            provider: model.provider.to_string(),
            model_used: model_used.to_string(),
            tier: model.tier.to_string(),
            cost_usd: cost_micro_usd as f64 / 1_000_000.0,
            fallback_used: fallback_count > 0,
            fallback_count,
            request_id,
        }),
    }
}
