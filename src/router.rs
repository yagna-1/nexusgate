use tracing::{debug, info, warn};

use crate::{
    config::Config,
    models::{model_catalog, AuthContext, ModelInfo, ModelTier, ProviderType},
};

pub struct ModelRouter {
    catalog: Vec<ModelInfo>,
}

impl ModelRouter {
    pub fn new() -> Self {
        Self {
            catalog: model_catalog(),
        }
    }

    /// Resolve the initial model to use for a request.
    ///
    /// Priority order:
    ///  1. Explicitly requested model (by ID or alias)
    ///  2. Tier preference from NexusGate options
    ///  3. Default: economy tier, cheapest first
    pub fn resolve_initial_model(
        &self,
        requested_model: Option<&str>,
        preferred_tier: Option<&str>,
        ctx: &AuthContext,
        config: &Config,
    ) -> Option<ModelInfo> {
        // Collect configured providers
        let configured = self.configured_providers(config);

        // If a specific model was requested, find it
        if let Some(model_name) = requested_model {
            if let Some(m) = self.find_model(model_name, &configured, ctx) {
                return Some(m.clone());
            }
            warn!(model = model_name, "Requested model not found, falling back to tier routing");
        }

        // Tier-based selection
        let tier = preferred_tier
            .and_then(ModelTier::from_str)
            .unwrap_or(ModelTier::Economy);

        self.best_model_for_tier(&tier, &configured, ctx)
    }

    /// Build an ordered fallback chain for a given starting model.
    /// Falls back within the same tier first (different providers), then drops to cheaper tier.
    pub fn fallback_chain(
        &self,
        starting_model: &ModelInfo,
        config: &Config,
        ctx: &AuthContext,
        rate_limited: &[ProviderType],
    ) -> Vec<ModelInfo> {
        let configured = self.configured_providers(config);
        let mut chain: Vec<ModelInfo> = vec![];

        // 1. Same tier, different provider, not rate limited
        let same_tier_alternatives: Vec<_> = self
            .catalog
            .iter()
            .filter(|m| {
                m.tier == starting_model.tier
                    && m.model_id != starting_model.model_id
                    && configured.contains(&m.provider)
                    && !rate_limited.contains(&m.provider)
                    && self.tier_allowed(m, ctx)
            })
            // cheapest first within same tier
            .collect();

        chain.extend(same_tier_alternatives.into_iter().cloned());

        // 2. Cheaper tier (Economy is the floor)
        if starting_model.tier != ModelTier::Economy {
            let cheaper: Vec<_> = self
                .catalog
                .iter()
                .filter(|m| {
                    m.tier == ModelTier::Economy
                        && configured.contains(&m.provider)
                        && !rate_limited.contains(&m.provider)
                        && self.tier_allowed(m, ctx)
                })
                .collect();
            chain.extend(cheaper.into_iter().cloned());
        }

        debug!(
            starting = %starting_model.model_id,
            chain_len = chain.len(),
            "Built fallback chain"
        );
        chain
    }

    /// Log which model was selected and why
    pub fn log_selection(&self, model: &ModelInfo, reason: &str) {
        info!(
            model = %model.model_id,
            provider = %model.provider,
            tier = %model.tier,
            reason,
            "Model selected"
        );
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn find_model<'a>(
        &'a self,
        name: &str,
        configured: &[ProviderType],
        ctx: &AuthContext,
    ) -> Option<&'a ModelInfo> {
        self.catalog.iter().find(|m| {
            (m.model_id == name || m.alias.as_deref() == Some(name))
                && configured.contains(&m.provider)
                && self.tier_allowed(m, ctx)
        })
    }

    fn best_model_for_tier(
        &self,
        tier: &ModelTier,
        configured: &[ProviderType],
        ctx: &AuthContext,
    ) -> Option<ModelInfo> {
        // Sort by ascending cost within tier, pick cheapest available
        let mut candidates: Vec<_> = self
            .catalog
            .iter()
            .filter(|m| {
                m.tier == *tier
                    && configured.contains(&m.provider)
                    && self.tier_allowed(m, ctx)
            })
            .collect();

        candidates.sort_by(|a, b| {
            a.input_cost_per_million
                .partial_cmp(&b.input_cost_per_million)
                .unwrap()
        });

        candidates.first().map(|m| (*m).clone())
    }

    fn configured_providers(&self, config: &Config) -> Vec<ProviderType> {
        let mut providers = vec![];
        if config.openai_api_key.is_some() {
            providers.push(ProviderType::OpenAI);
        }
        if config.anthropic_api_key.is_some() {
            providers.push(ProviderType::Anthropic);
        }
        if config.gemini_api_key.is_some() {
            providers.push(ProviderType::Gemini);
        }
        providers
    }

    fn tier_allowed(&self, model: &ModelInfo, ctx: &AuthContext) -> bool {
        match &ctx.allowed_tiers {
            None => true, // No restriction
            Some(tiers) => tiers.contains(&model.tier.to_string()),
        }
    }
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::new()
    }
}
