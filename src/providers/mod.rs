pub mod anthropic;
pub mod gemini;
pub mod openai;

use async_trait::async_trait;

use crate::{
    error::Result,
    models::{ProviderRequest, ProviderResponse},
};

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, request: &ProviderRequest) -> Result<ProviderResponse>;
}
