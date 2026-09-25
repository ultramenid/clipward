//! Provider gateway: OpenRouter / Anthropic / OpenAI HTTP and logged-in AI CLIs.
//! TODO(planner agent): port llm_gateway.py + openrouter.py. Signature is fixed; other modules call it.
use crate::error::Result;
use crate::types::AiSettings;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Default)]
pub struct LlmRequest {
    pub system: String,
    pub user: String,
    /// JPEG bytes, sent as images when the provider supports them.
    pub images: Vec<Vec<u8>>,
    /// JSON schema the reply must match; None for free text.
    pub json_schema: Option<serde_json::Value>,
    pub max_tokens: u32,
    pub temperature: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct LlmResponse {
    pub text: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cost_usd: f64,
    pub cost_incomplete: bool,
}

/// CLI providers read text only.
pub fn supports_images(provider: &str) -> bool {
    !provider.starts_with("cli:")
}

pub async fn complete(_ai: &AiSettings, _req: LlmRequest, _cancel: &CancellationToken) -> Result<LlmResponse> {
    todo!("planner agent")
}
