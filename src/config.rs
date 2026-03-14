use anyhow::{anyhow, Context};

#[derive(Clone, Debug)]
pub struct Config {
    // Server
    pub host: String,
    pub port: u16,

    // Storage
    pub database_url: String,
    pub redis_url: String,

    // Security
    pub admin_jwt_secret: String,

    // Provider API keys (optional — only routes to configured providers)
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub gemini_api_key: Option<String>,

    // Behaviour
    pub default_max_tokens: u32,
    pub request_timeout_secs: u64,
    pub max_fallback_attempts: usize,
    pub mcp_default_estimated_cost_micro_usd: i64,

    // CORS origins (comma-separated, * = all)
    pub cors_origins: String,

    // MCP routing extension
    pub mcp_upstream_url: Option<String>,
    pub mcp_upstream_bearer_token: Option<String>,
    pub public_base_url: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let admin_jwt_secret =
            std::env::var("ADMIN_JWT_SECRET").context("ADMIN_JWT_SECRET must be set")?;

        if admin_jwt_secret.len() < 32 {
            return Err(anyhow!("ADMIN_JWT_SECRET must be at least 32 characters"));
        }

        Ok(Config {
            host: std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: std::env::var("PORT")
                .unwrap_or_else(|_| "8080".into())
                .parse()
                .context("PORT must be a valid port number")?,

            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite:/app/data/nexusgate.db".into()),
            redis_url: std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://redis:6379".into()),

            admin_jwt_secret,

            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            gemini_api_key: std::env::var("GEMINI_API_KEY").ok(),

            default_max_tokens: std::env::var("DEFAULT_MAX_TOKENS")
                .unwrap_or_else(|_| "4096".into())
                .parse()
                .context("DEFAULT_MAX_TOKENS must be a number")?,
            request_timeout_secs: std::env::var("REQUEST_TIMEOUT_SECS")
                .unwrap_or_else(|_| "60".into())
                .parse()
                .context("REQUEST_TIMEOUT_SECS must be a number")?,
            max_fallback_attempts: std::env::var("MAX_FALLBACK_ATTEMPTS")
                .unwrap_or_else(|_| "3".into())
                .parse()
                .context("MAX_FALLBACK_ATTEMPTS must be a number")?,
            mcp_default_estimated_cost_micro_usd: std::env::var(
                "MCP_DEFAULT_ESTIMATED_COST_MICRO_USD",
            )
            .unwrap_or_else(|_| "2500".into())
            .parse()
            .context("MCP_DEFAULT_ESTIMATED_COST_MICRO_USD must be a number")?,

            cors_origins: std::env::var("CORS_ORIGINS").unwrap_or_else(|_| "*".into()),
            mcp_upstream_url: std::env::var("MCP_UPSTREAM_URL").ok(),
            mcp_upstream_bearer_token: std::env::var("MCP_UPSTREAM_BEARER_TOKEN").ok(),
            public_base_url: std::env::var("PUBLIC_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:8080".into()),
        })
    }

    /// Check how many providers are configured
    pub fn configured_providers(&self) -> Vec<&'static str> {
        let mut providers = vec![];
        if self.openai_api_key.is_some() {
            providers.push("openai");
        }
        if self.anthropic_api_key.is_some() {
            providers.push("anthropic");
        }
        if self.gemini_api_key.is_some() {
            providers.push("gemini");
        }
        providers
    }
}
