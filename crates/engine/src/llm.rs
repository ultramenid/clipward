//! Provider gateway: OpenRouter / Anthropic / OpenAI HTTP, the user's own local
//! OpenAI-compatible server (Ollama, LM Studio, llama.cpp, vLLM) and logged-in AI CLIs.
//!
//! Port of `services/llm_gateway.py` + `services/openrouter.py`. Callers build one
//! request and read one reply, so the planner and layout vision don't know which
//! provider answered. CLIs are text only: image parts are dropped.
//!
//! Error messages carry the marker phrases `pipeline::describe_failure` maps
//! ("cli is not logged in", "cli was not found", "cli could not be started",
//! "cli timed out", "local ai server is not reachable", "local model not found",
//! "local model context is too small", "out of credits", "(401)", "invalid api key",
//! ...). They never
//! contain API keys, response bodies or CLI output.
use crate::error::{EngineError, Result};
use crate::types::AiSettings;
use base64::Engine as _;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// Default OpenRouter model (config.py PLANNER_MODEL) when `ai.model` is empty.
pub const DEFAULT_OPENROUTER_MODEL: &str = "anthropic/claude-opus-5.5";
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-opus-5";
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-6-sol";

/// OpenAI-compatible chat bodies are small JSON; never buffer more than this.
pub const MAX_CHAT_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
/// Anthropic streams SSE (several bytes of framing per token) and CLIs print JSON.
pub const MAX_STREAM_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_CLI_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_CLI_STDERR_BYTES: usize = 256 * 1024;
/// Planning a multi-hour transcript through an agent CLI can take many minutes.
pub const CLI_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Reasoning over a multi-hour transcript can take minutes.
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(600);
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP statuses worth retrying (rate limits, provider outages, timeouts).
pub const RETRYABLE_STATUS_CODES: [u16; 6] = [408, 429, 500, 502, 503, 504];

/// id -> label. Detection and paths live in the desktop app.
pub const CLI_PROVIDERS: &[(&str, &str)] = &[
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("agy", "Antigravity"),
    ("grok", "Grok"),
    ("cursor-agent", "Cursor"),
    ("opencode", "OpenCode"),
    ("gemini", "Gemini CLI"),
];

/// Per-token list prices, used only when a provider reports no billed cost.
pub const MODEL_PRICING: &[(&str, f64, f64)] = &[
    ("anthropic/claude-opus-5.5", 4.00e-6, 20.0e-6),
    ("z-ai/glm-5.3-flash", 0.075e-6, 0.25e-6),
    ("google/gemini-3.8-flash", 0.75e-6, 3.75e-6),
    ("openai/gpt-6-sol", 2.00e-6, 10.0e-6),
];
/// Conservative fallback when the model is unknown.
pub const DEFAULT_PRICING: (f64, f64) = (2.00e-6, 12.0e-6);

/// (input, output) USD per token for `model`.
pub fn model_pricing(model: &str) -> (f64, f64) {
    MODEL_PRICING
        .iter()
        .find(|(m, _, _)| *m == model)
        .map(|(_, i, o)| (*i, *o))
        .unwrap_or(DEFAULT_PRICING)
}

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
    /// True when the provider reported no billed cost and `cost_usd` is a list-price estimate.
    pub cost_incomplete: bool,
}

/// Whether `provider` takes images regardless of model. CLI providers read text
/// only; a local server depends on its model, so it counts as text only here: use
/// [`ai_supports_images`] when the settings are at hand.
pub fn supports_images(provider: &str) -> bool {
    !provider.starts_with("cli:") && provider != "local"
}

/// Whether requests through `ai` may carry images: a local model only when the
/// user said it accepts them (Settings → "Model accepts images").
pub fn ai_supports_images(ai: &AiSettings) -> bool {
    if ai.provider == "local" {
        ai.local_images
    } else {
        supports_images(&ai.provider)
    }
}

/// Simple one-shot call used by modules that don't need routing options.
/// OpenRouter uses `ai.model` (or [`DEFAULT_OPENROUTER_MODEL`]); a caller wanting a
/// different OpenRouter model passes a clone of `AiSettings` with `model` set.
/// Single attempt: callers retry as they see fit (see [`chat`] for `retryable`).
pub async fn complete(
    ai: &AiSettings,
    req: LlmRequest,
    cancel: &CancellationToken,
) -> Result<LlmResponse> {
    let mut parts = vec![ContentPart::Text(req.user)];
    if ai_supports_images(ai) {
        for image in req.images {
            parts.push(ContentPart::Image {
                mime: "image/jpeg".into(),
                base64: base64::engine::general_purpose::STANDARD.encode(image),
            });
        }
    }
    let chat_req = ChatRequest {
        model: if ai.model.is_empty() {
            DEFAULT_OPENROUTER_MODEL.into()
        } else {
            ai.model.clone()
        },
        system: (!req.system.is_empty()).then_some(req.system),
        user: parts,
        max_tokens: req.max_tokens,
        require_parameters: req.json_schema.is_some(),
        schema: req.json_schema.map(|s| ("response".to_string(), s)),
        temperature: req.temperature,
        ..Default::default()
    };
    let reply = chat(ai, &chat_req, cancel).await?;
    let (cost_usd, cost_incomplete) = match reply.cost {
        Some(cost) => (cost, false),
        None => {
            let (input, output) = model_pricing(&reply.model);
            (
                reply.prompt_tokens as f64 * input + reply.completion_tokens as f64 * output,
                true,
            )
        }
    };
    Ok(LlmResponse {
        text: reply.text.unwrap_or_default(),
        prompt_tokens: reply.prompt_tokens,
        completion_tokens: reply.completion_tokens,
        cost_usd,
        cost_incomplete,
    })
}

// ---------------------------------------------------------------------------
// Full request / reply (planner and anything needing OpenRouter routing)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ContentPart {
    Text(String),
    /// Base64 image data with its MIME type.
    Image {
        mime: String,
        base64: String,
    },
}

/// One chat request in OpenRouter shape. Direct providers ignore the routing extras.
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    /// OpenRouter model slug. Direct providers use `ai.model` or their default.
    pub model: String,
    /// OpenRouter `models` failover chain.
    pub fallback_models: Vec<String>,
    pub system: Option<String>,
    /// User message parts, in order.
    pub user: Vec<ContentPart>,
    pub max_tokens: u32,
    /// (name, schema) for strict JSON-schema structured output.
    pub schema: Option<(String, Value)>,
    /// OpenRouter reasoning effort. "none" sends `temperature` (default 0.2) instead.
    pub reasoning_effort: Option<String>,
    pub temperature: Option<f64>,
    /// OpenRouter `response-healing` plugin.
    pub response_healing: bool,
    /// OpenRouter `provider.require_parameters`.
    pub require_parameters: bool,
    /// `image_url.detail` ("low" keeps token counts down).
    pub image_detail: Option<String>,
    /// Overrides the provider base URL (tests).
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatReply {
    pub text: Option<String>,
    pub finish_reason: Option<String>,
    /// The model that actually served the request.
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    /// Billed cost in USD; None when the provider did not report it.
    pub cost: Option<f64>,
}

/// A provider call failed. `retryable` marks transient failures.
#[derive(Debug, Clone, PartialEq)]
pub struct LlmError {
    pub message: String,
    pub retryable: bool,
    pub cancelled: bool,
}

impl LlmError {
    fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            cancelled: false,
        }
    }
    fn retry(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
            cancelled: false,
        }
    }
    fn cancelled() -> Self {
        Self {
            message: "cancelled".into(),
            retryable: false,
            cancelled: true,
        }
    }
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<LlmError> for EngineError {
    fn from(e: LlmError) -> Self {
        if e.cancelled {
            EngineError::Cancelled
        } else {
            EngineError::Failed(e.message)
        }
    }
}

type LlmResult<T> = std::result::Result<T, LlmError>;

/// Sends `req` to the provider in `ai`.
pub async fn chat(
    ai: &AiSettings,
    req: &ChatRequest,
    cancel: &CancellationToken,
) -> LlmResult<ChatReply> {
    if cancel.is_cancelled() {
        return Err(LlmError::cancelled());
    }
    let provider = ai.provider.as_str();
    match provider {
        "openrouter" => {
            let key = ai.openrouter_api_key.as_deref().filter(|k| !k.is_empty());
            let Some(key) = key else {
                return Err(LlmError::fatal(
                    "OpenRouter API key is missing (invalid api key)",
                ));
            };
            let base = req.base_url.as_deref().unwrap_or(OPENROUTER_BASE_URL);
            let headers = vec![
                ("Authorization", format!("Bearer {key}")),
                (
                    "HTTP-Referer",
                    "https://github.com/clipward/clipward".to_string(),
                ),
                ("X-Title", "Clipward AI Clipping Agent".to_string()),
            ];
            let payload = openrouter_payload(req);
            with_cancel(
                cancel,
                post_chat_completion(base, &headers, &payload, "OpenRouter"),
            )
            .await
        }
        "openai" => {
            let Some(key) = ai.openai_api_key.as_deref().filter(|k| !k.is_empty()) else {
                return Err(LlmError::fatal(
                    "OpenAI API key is missing (invalid api key)",
                ));
            };
            let base = req.base_url.as_deref().unwrap_or(OPENAI_BASE_URL);
            let headers = vec![("Authorization", format!("Bearer {key}"))];
            let payload = openai_payload(req, &ai.model);
            with_cancel(
                cancel,
                post_chat_completion(base, &headers, &payload, "OpenAI"),
            )
            .await
        }
        "anthropic" => {
            let Some(key) = ai.anthropic_api_key.as_deref().filter(|k| !k.is_empty()) else {
                return Err(LlmError::fatal(
                    "Anthropic API key is missing (invalid api key)",
                ));
            };
            let base = req.base_url.as_deref().unwrap_or(ANTHROPIC_BASE_URL);
            let payload = anthropic_payload(req, &ai.model);
            with_cancel(cancel, anthropic_messages(base, key, &payload, cancel)).await
        }
        "local" => local_chat(ai, req, cancel).await,
        p if p.starts_with("cli:") => cli_chat(&p[4..], ai, req, cancel).await,
        other => Err(LlmError::fatal(format!("Unknown AI provider: {other}"))),
    }
}

async fn with_cancel<T>(
    cancel: &CancellationToken,
    fut: impl std::future::Future<Output = LlmResult<T>>,
) -> LlmResult<T> {
    tokio::select! {
        r = fut => r,
        _ = cancel.cancelled() => Err(LlmError::cancelled()),
    }
}

fn http_client() -> LlmResult<reqwest::Client> {
    reqwest::Client::builder()
        // Never follow a redirect: it could carry the request (and key) elsewhere.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .read_timeout(HTTP_READ_TIMEOUT)
        .build()
        .map_err(|_| LlmError::fatal("HTTP client could not be created"))
}

// ---------------------------------------------------------------------------
// OpenAI-compatible chat completions (OpenRouter, OpenAI)
// ---------------------------------------------------------------------------

fn openai_messages(req: &ChatRequest) -> Vec<Value> {
    let mut messages = Vec::new();
    if let Some(system) = &req.system {
        messages.push(json!({"role": "system", "content": system}));
    }
    let content: Vec<Value> = req
        .user
        .iter()
        .map(|part| match part {
            ContentPart::Text(text) => json!({"type": "text", "text": text}),
            ContentPart::Image { mime, base64 } => {
                let mut image_url = Map::new();
                image_url.insert(
                    "url".into(),
                    Value::String(format!("data:{mime};base64,{base64}")),
                );
                if let Some(detail) = &req.image_detail {
                    image_url.insert("detail".into(), Value::String(detail.clone()));
                }
                json!({"type": "image_url", "image_url": image_url})
            }
        })
        .collect();
    messages.push(json!({"role": "user", "content": content}));
    messages
}

/// `response_format` for strict JSON-schema structured output.
pub fn json_schema_format(name: &str, schema: &Value) -> Value {
    json!({"type": "json_schema", "json_schema": {"name": name, "strict": true, "schema": schema}})
}

/// The OpenRouter chat payload for `req`.
pub fn openrouter_payload(req: &ChatRequest) -> Value {
    let mut payload = Map::new();
    payload.insert("model".into(), Value::String(req.model.clone()));
    payload.insert("messages".into(), Value::Array(openai_messages(req)));
    payload.insert("max_tokens".into(), json!(req.max_tokens));
    if let Some((name, schema)) = &req.schema {
        payload.insert("response_format".into(), json_schema_format(name, schema));
    }
    if req.response_healing {
        // Repairs near-miss JSON (trailing commas, stray fences) server-side.
        payload.insert("plugins".into(), json!([{"id": "response-healing"}]));
    }
    if req.require_parameters {
        // Only route to endpoints that honour every parameter, so a provider
        // can't silently drop the schema or reasoning settings.
        payload.insert("provider".into(), json!({"require_parameters": true}));
    }
    if !req.fallback_models.is_empty() {
        payload.insert("models".into(), json!(req.fallback_models));
    }
    // Reasoning models ignore or reject temperature, so it is only sent with effort "none".
    match req.reasoning_effort.as_deref() {
        Some("none") => {
            payload.insert("temperature".into(), json!(req.temperature.unwrap_or(0.2)));
        }
        Some(effort) => {
            payload.insert(
                "reasoning".into(),
                json!({"effort": effort, "exclude": true}),
            );
        }
        None => {
            if let Some(t) = req.temperature {
                payload.insert("temperature".into(), json!(t));
            }
        }
    }
    Value::Object(payload)
}

/// OpenAI body: OpenRouter-only routing keys removed, `max_completion_tokens`.
fn openai_payload(req: &ChatRequest, model: &str) -> Value {
    let mut payload = openrouter_payload(req);
    let obj = payload.as_object_mut().expect("object");
    for key in ["plugins", "provider", "models", "reasoning"] {
        obj.remove(key);
    }
    let model = if model.is_empty() {
        DEFAULT_OPENAI_MODEL
    } else {
        model
    };
    obj.insert("model".into(), Value::String(model.to_string()));
    if let Some(max) = obj.remove("max_tokens") {
        obj.insert("max_completion_tokens".into(), max);
    }
    payload
}

/// Reads at most `limit` bytes of a response body. Never trusts Content-Length.
async fn read_limited(
    response: &mut reqwest::Response,
    limit: usize,
    label: &str,
) -> LlmResult<Vec<u8>> {
    let mut content = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if chunk.len() > limit - content.len() {
                    return Err(LlmError::fatal(format!(
                        "{label} response exceeds the size limit"
                    )));
                }
                content.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(content),
            Err(_) => return Err(LlmError::retry(format!("{label} request failed"))),
        }
    }
}

fn identity_encoding(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .map(|v| {
            v.to_str()
                .map(|s| s.trim().eq_ignore_ascii_case("identity"))
                .unwrap_or(false)
        })
        .unwrap_or(true)
}

/// Python truthiness for `body.get("error")`.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn as_count(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f.max(0.0) as u64))
            .unwrap_or(0),
        _ => 0,
    }
}

fn as_cost(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// (content, finish_reason) of the first choice.
pub fn message_text(body: &Value) -> (Option<String>, Option<String>) {
    let choice = body
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first());
    let Some(choice) = choice else {
        return (None, None);
    };
    let content = choice.get("message").and_then(|m| m.get("content"));
    let text = match content {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(parts)) => Some(
            parts
                .iter()
                .filter_map(|p| p.as_object())
                .map(|p| p.get("text").and_then(|t| t.as_str()).unwrap_or(""))
                .collect(),
        ),
        _ => None,
    };
    let finish = choice
        .get("finish_reason")
        .and_then(|f| f.as_str())
        .map(str::to_string);
    (text, finish)
}

/// POST an OpenAI-compatible `/chat/completions` request.
pub(crate) async fn post_chat_completion(
    base_url: &str,
    headers: &[(&str, String)],
    payload: &Value,
    label: &str,
) -> LlmResult<ChatReply> {
    let client = http_client()?;
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut request = client
        .post(url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(
            serde_json::to_vec(payload)
                .map_err(|_| LlmError::fatal(format!("{label} request could not be encoded")))?,
        );
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| LlmError::retry(format!("{label} request failed")))?;
    // Do not hand attacker-controlled compressed bodies to an unbounded decompressor.
    if !identity_encoding(&response) {
        return Err(LlmError::fatal(format!(
            "{label} returned an unsupported response encoding"
        )));
    }
    let status = response.status().as_u16();
    let content = read_limited(&mut response, MAX_CHAT_RESPONSE_BYTES, label).await?;
    drop(response);

    if status == 402 {
        return Err(LlmError::fatal(if label == "OpenRouter" {
            "OpenRouter account is out of credits. Add credits at openrouter.ai/credits."
                .to_string()
        } else {
            format!("{label} account is out of credits.")
        }));
    }
    if status != 200 {
        let message = format!("{label} API error ({status})");
        return Err(if RETRYABLE_STATUS_CODES.contains(&status) {
            LlmError::retry(message)
        } else {
            LlmError::fatal(message)
        });
    }
    let body = chat_body(&content, label)?;
    // OpenRouter can return 200 with an upstream provider error in the body.
    if body.get("error").map(truthy).unwrap_or(false) {
        return Err(LlmError::retry(format!("{label} provider error")));
    }
    Ok(chat_reply(&body, payload, label))
}

/// A chat-completions response body, which must be a JSON object.
fn chat_body(content: &[u8], label: &str) -> LlmResult<Value> {
    let body: Value = serde_json::from_slice(content)
        .map_err(|_| LlmError::fatal(format!("{label} returned invalid JSON")))?;
    if !body.is_object() {
        return Err(LlmError::fatal(format!(
            "{label} returned an invalid response"
        )));
    }
    Ok(body)
}

/// Text, finish reason, served model and usage of a chat-completions body.
fn chat_reply(body: &Value, payload: &Value, label: &str) -> ChatReply {
    let usage = body.get("usage").filter(|u| u.is_object());
    let get = |k: &str| usage.and_then(|u| u.get(k));
    let (text, finish_reason) = message_text(body);
    let requested = payload.get("model").and_then(|m| m.as_str()).unwrap_or("");
    let model = body
        .get("model")
        .and_then(|m| m.as_str())
        .filter(|m| !m.is_empty())
        .unwrap_or(requested)
        .to_string();
    let reply = ChatReply {
        text,
        finish_reason,
        model,
        prompt_tokens: as_count(get("prompt_tokens")),
        completion_tokens: as_count(get("completion_tokens")),
        total_tokens: as_count(get("total_tokens")),
        cost: as_cost(get("cost")),
    };
    let reasoning =
        as_count(get("completion_tokens_details").and_then(|d| d.get("reasoning_tokens")));
    tracing::info!(
        "{label} usage ({}): {} prompt, {} completion ({} reasoning), cost={}",
        reply.model,
        reply.prompt_tokens,
        reply.completion_tokens,
        reasoning,
        reply
            .cost
            .map(|c| format!("${c}"))
            .unwrap_or_else(|| "n/a".into())
    );
    reply
}

// ---------------------------------------------------------------------------
// Local AI: the user's own OpenAI-compatible server (Ollama, LM Studio,
// llama.cpp server, vLLM, ...). Planning traffic goes only to that server.
// ---------------------------------------------------------------------------

/// Ollama's OpenAI-compatible endpoint.
pub const DEFAULT_LOCAL_BASE_URL: &str = "http://localhost:11434/v1";
pub const MAX_LOCAL_BASE_URL_CHARS: usize = 512;
/// A server on this machine or the LAN answers the TCP handshake at once.
pub const LOCAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Local models are slow: loading one and reading a long transcript can take many minutes.
pub const LOCAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Listing the models a server offers.
pub const LOCAL_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Output caps above this are left to the server: a fixed 32k cap makes servers that
/// check prompt + max_tokens against the context (vLLM, llama.cpp) refuse every
/// prompt on a 32k-context model, while they all stop at the context anyway.
pub const LOCAL_MAX_OUTPUT_TOKENS: u32 = 8192;
const MAX_LOCAL_MODELS: usize = 500;
/// Error bodies are only inspected for marker words, never echoed.
const MAX_LOCAL_ERROR_SCAN_BYTES: usize = 64 * 1024;

const LOCAL_LABEL: &str = "Local AI server";
const LOCAL_UNREACHABLE: &str = "Local AI server is not reachable";
const LOCAL_MODEL_NOT_FOUND: &str = "Local model not found on the server";
const LOCAL_CONTEXT_TOO_SMALL: &str =
    "Local model context is too small for this prompt (the server reported a context overflow)";
const LOCAL_OUT_OF_MEMORY: &str = "Local model does not fit in memory (local model is too large)";

/// The base URL as saved, or why it is refused: http(s), a host, no credentials,
/// query or fragment, bounded length. Loopback, LAN and `.local` hosts are allowed:
/// this is the user's own server. A trailing slash is dropped.
pub fn normalize_local_base_url(value: &str) -> std::result::Result<String, &'static str> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("Enter the local AI server URL");
    }
    if trimmed.chars().count() > MAX_LOCAL_BASE_URL_CHARS
        || trimmed.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("The local AI server URL is not valid");
    }
    let url = url::Url::parse(trimmed).map_err(|_| "The local AI server URL is not valid")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("The local AI server URL must start with http:// or https://");
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("The local AI server URL needs a host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Put the server key in the API key field, not in the URL");
    }
    if url.query().is_some() || url.fragment().is_some() || trimmed.contains(['?', '#']) {
        return Err("The local AI server URL cannot have a query or fragment");
    }
    Ok(trimmed.to_string())
}

/// Hosts on this machine or the local network: requests to them skip any proxy.
pub fn is_local_host(host: &url::Host<&str>) -> bool {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    fn v4(ip: Ipv4Addr) -> bool {
        let [a, b, ..] = ip.octets();
        // 100.64/10 is carrier-grade NAT, also used by Tailscale.
        ip.is_loopback()
            || ip.is_private()
            || ip.is_link_local()
            || ip.is_unspecified()
            || (a == 100 && (64..=127).contains(&b))
    }
    fn v6(ip: Ipv6Addr) -> bool {
        let first = ip.segments()[0];
        ip.is_loopback()
            || ip.is_unspecified()
            || (first & 0xfe00) == 0xfc00
            || (first & 0xffc0) == 0xfe80
            || ip.to_ipv4_mapped().is_some_and(v4)
    }
    match host {
        url::Host::Ipv4(ip) => v4(*ip),
        url::Host::Ipv6(ip) => v6(*ip),
        url::Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            if let Ok(ip) = name.parse::<IpAddr>() {
                return match ip {
                    IpAddr::V4(ip) => v4(ip),
                    IpAddr::V6(ip) => v6(ip),
                };
            }
            name == "localhost"
                || !name.contains('.')
                || [".localhost", ".local", ".lan", ".home.arpa", ".internal"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
        }
    }
}

fn base_is_local(base: &str) -> bool {
    url::Url::parse(base)
        .ok()
        .and_then(|url| url.host().map(|host| is_local_host(&host)))
        .unwrap_or(false)
}

fn local_http_client(base: &str, timeout: Duration) -> LlmResult<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(LOCAL_CONNECT_TIMEOUT)
        .timeout(timeout);
    if base_is_local(base) {
        // A system proxy cannot reach this machine or the LAN on the user's behalf.
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|_| LlmError::fatal("HTTP client could not be created"))
}

fn local_base(ai: &AiSettings, req_base: Option<&str>) -> LlmResult<String> {
    let base = req_base
        .or(ai.base_url.as_deref())
        .filter(|b| !b.trim().is_empty())
        .unwrap_or(DEFAULT_LOCAL_BASE_URL);
    normalize_local_base_url(base).map_err(|_| {
        LlmError::fatal(format!(
            "{LOCAL_UNREACHABLE}: the base URL in Settings is not valid"
        ))
    })
}

fn local_headers(ai: &AiSettings) -> Vec<(&'static str, String)> {
    ai.local_api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|key| vec![("Authorization", format!("Bearer {key}"))])
        .unwrap_or_default()
}

/// How the reply is held to the JSON schema. Servers differ: Ollama and vLLM take
/// both formats, LM Studio only `json_schema`, llama.cpp refuses schemas its grammar
/// converter can't handle. Each rejection steps down one level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalFormat {
    JsonSchema,
    JsonObject,
    Prompt,
}

impl LocalFormat {
    fn fallback(self) -> Option<Self> {
        match self {
            Self::JsonSchema => Some(Self::JsonObject),
            Self::JsonObject => Some(Self::Prompt),
            Self::Prompt => None,
        }
    }
}

/// The format each (server, model) last accepted, so retries and later requests
/// don't probe again.
static LOCAL_FORMATS: LazyLock<Mutex<HashMap<(String, String), LocalFormat>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The chat payload for a local server. Same messages as the hosted providers;
/// routing and reasoning keys dropped; the JSON schema sent as `format` allows.
pub(crate) fn local_payload(req: &ChatRequest, model: &str, format: LocalFormat) -> Value {
    let mut payload = openrouter_payload(req);
    let obj = payload.as_object_mut().expect("object");
    for key in [
        "plugins",
        "provider",
        "models",
        "reasoning",
        "response_format",
    ] {
        obj.remove(key);
    }
    obj.insert("model".into(), Value::String(model.to_string()));
    obj.insert("stream".into(), Value::Bool(false));
    if req.max_tokens > LOCAL_MAX_OUTPUT_TOKENS {
        obj.remove("max_tokens");
    }
    if let Some(messages) = obj.get_mut("messages").and_then(Value::as_array_mut) {
        // Text-only content as one string: every OpenAI-compatible server reads that.
        if let Some(user) = messages.last_mut() {
            let parts = user["content"].as_array().cloned().unwrap_or_default();
            if parts.iter().all(|p| p["type"] == "text") {
                let text: Vec<&str> = parts.iter().filter_map(|p| p["text"].as_str()).collect();
                user["content"] = Value::String(text.join("\n\n"));
            }
        }
        if let Some((_, schema)) = &req.schema {
            if format != LocalFormat::JsonSchema {
                let instruction = format!(
                    "Reply with only one JSON object that matches this JSON schema, with no other text and no code fences:\n{schema}"
                );
                match messages.first_mut() {
                    Some(first) if first["role"] == "system" => {
                        let system = first["content"].as_str().unwrap_or("").to_string();
                        first["content"] = Value::String(format!("{system}\n\n{instruction}"));
                    }
                    _ => messages.insert(0, json!({"role": "system", "content": instruction})),
                }
            }
        }
    }
    if let Some((name, schema)) = &req.schema {
        match format {
            LocalFormat::JsonSchema => {
                obj.insert("response_format".into(), json_schema_format(name, schema));
            }
            LocalFormat::JsonObject => {
                obj.insert("response_format".into(), json!({"type": "json_object"}));
            }
            LocalFormat::Prompt => {}
        }
    }
    payload
}

/// A failed local request: either the server refused the structured-output format
/// (try the next one) or a real error.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LocalFailure {
    FormatRejected,
    Error(LlmError),
}

/// Classify an error response by its status and marker words in its body. The
/// body is never copied into the message.
pub(crate) fn classify_local_error(status: u16, body: &[u8], structured: bool) -> LocalFailure {
    let scan = &body[..body.len().min(MAX_LOCAL_ERROR_SCAN_BYTES)];
    let text = String::from_utf8_lossy(scan).to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
    if status == 401 || status == 403 {
        return LocalFailure::Error(LlmError::fatal(format!(
            "{LOCAL_LABEL} API error ({status}): the server refused the key (invalid api key)"
        )));
    }
    if has(&[
        "context length",
        "context_length",
        "context window",
        "context size",
        "maximum context",
        "exceed_context",
        "exceeds the available context",
        "n_ctx",
        "too many tokens",
        "prompt is too long",
        "input is too long",
    ]) {
        return LocalFailure::Error(LlmError::fatal(LOCAL_CONTEXT_TOO_SMALL));
    }
    let model_missing = has(&[
        "model_not_found",
        "no models loaded",
        "try pulling",
        "no such model",
        "unknown model",
    ]) || (has(&["model"])
        && (status == 404 || has(&["not found", "does not exist", "not loaded"])));
    if model_missing {
        return LocalFailure::Error(LlmError::fatal(LOCAL_MODEL_NOT_FOUND));
    }
    if has(&[
        "more system memory",
        "out of memory",
        "insufficient memory",
        "not enough memory",
        "cudamalloc failed",
    ]) {
        return LocalFailure::Error(LlmError::fatal(LOCAL_OUT_OF_MEMORY));
    }
    if status == 404 {
        return LocalFailure::Error(LlmError::fatal(format!(
            "Local AI endpoint was not found ({status}); the base URL usually ends in /v1"
        )));
    }
    if structured && matches!(status, 400 | 415 | 422 | 500 | 501) {
        let about_format = has(&[
            "response_format",
            "json_schema",
            "json_object",
            "schema",
            "grammar",
            "structured",
            "format",
        ]);
        // An unexplained bad request may be the schema too: a plainer format decides.
        if about_format || matches!(status, 400 | 422) {
            return LocalFailure::FormatRejected;
        }
    }
    let message = format!("{LOCAL_LABEL} API error ({status})");
    LocalFailure::Error(if RETRYABLE_STATUS_CODES.contains(&status) {
        LlmError::retry(message)
    } else {
        LlmError::fatal(message)
    })
}

fn local_transport_error(error: &reqwest::Error) -> LlmError {
    if error.is_timeout() {
        LlmError::fatal(format!(
            "Local AI server timed out after {} minutes (local ai server timed out)",
            LOCAL_REQUEST_TIMEOUT.as_secs() / 60
        ))
    } else if error.is_connect() {
        LlmError::fatal(LOCAL_UNREACHABLE)
    } else {
        // Dropped mid-request, often a server that crashed loading the model.
        LlmError::retry(format!("{LOCAL_UNREACHABLE} (the connection failed)"))
    }
}

/// Status and body (at most `limit` bytes) of one request.
async fn local_send(request: reqwest::RequestBuilder, limit: usize) -> LlmResult<(u16, Vec<u8>)> {
    let mut response = request
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|e| local_transport_error(&e))?;
    if !identity_encoding(&response) {
        return Err(LlmError::fatal(format!(
            "{LOCAL_LABEL} returned an unsupported response encoding"
        )));
    }
    let status = response.status().as_u16();
    let mut content = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if chunk.len() > limit - content.len() {
                    return Err(LlmError::fatal(format!(
                        "{LOCAL_LABEL} response exceeds the size limit"
                    )));
                }
                content.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok((status, content)),
            Err(e) => return Err(local_transport_error(&e)),
        }
    }
}

async fn post_local(
    client: &reqwest::Client,
    base: &str,
    headers: &[(&str, String)],
    payload: &Value,
    structured: bool,
) -> std::result::Result<ChatReply, LocalFailure> {
    let body = serde_json::to_vec(payload).map_err(|_| {
        LocalFailure::Error(LlmError::fatal("Local AI request could not be encoded"))
    })?;
    let mut request = client
        .post(format!("{base}/chat/completions"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body);
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let (status, content) = local_send(request, MAX_CHAT_RESPONSE_BYTES)
        .await
        .map_err(LocalFailure::Error)?;
    if status != 200 {
        return Err(classify_local_error(status, &content, structured));
    }
    let body = chat_body(&content, LOCAL_LABEL).map_err(LocalFailure::Error)?;
    if body.get("error").map(truthy).unwrap_or(false) {
        return Err(classify_local_error(500, &content, structured));
    }
    Ok(chat_reply(&body, payload, LOCAL_LABEL))
}

/// Rough prompt size (4 characters per token) for the truncation warning.
fn estimated_prompt_tokens(req: &ChatRequest) -> u64 {
    let text: usize = req.system.as_deref().map_or(0, |s| s.chars().count())
        + req
            .user
            .iter()
            .map(|part| match part {
                ContentPart::Text(t) => t.chars().count(),
                ContentPart::Image { .. } => 0,
            })
            .sum::<usize>();
    (text / 4) as u64
}

/// The JSON a local model wrapped in reasoning, code fences or prose, when the
/// reply is not JSON as it stands.
pub(crate) fn extract_local_json(text: &str) -> String {
    let mut rest = text.trim();
    // Reasoning models (Qwen3, DeepSeek-R1) may think out loud first.
    while let Some(end) = rest.find("</think>") {
        rest = rest[end + "</think>".len()..].trim_start();
    }
    let rest = rest.trim();
    if serde_json::from_str::<Value>(rest).is_ok() {
        return rest.to_string();
    }
    let unfenced = rest
        .strip_prefix("```json")
        .or_else(|| rest.strip_prefix("```"))
        .and_then(|r| r.trim_end().strip_suffix("```"))
        .map(str::trim);
    if let Some(inner) = unfenced.filter(|i| serde_json::from_str::<Value>(i).is_ok()) {
        return inner.to_string();
    }
    match (rest.find('{'), rest.rfind('}')) {
        (Some(start), Some(end)) if start < end => {
            let candidate = &rest[start..=end];
            if serde_json::from_str::<Value>(candidate).is_ok() {
                return candidate.to_string();
            }
            rest.to_string()
        }
        _ => rest.to_string(),
    }
}

async fn local_chat(
    ai: &AiSettings,
    req: &ChatRequest,
    cancel: &CancellationToken,
) -> LlmResult<ChatReply> {
    let model = ai.model.trim();
    if model.is_empty() {
        return Err(LlmError::fatal(format!(
            "{LOCAL_MODEL_NOT_FOUND}: no model is selected"
        )));
    }
    let base = local_base(ai, req.base_url.as_deref())?;
    let client = local_http_client(&base, LOCAL_REQUEST_TIMEOUT)?;
    let headers = local_headers(ai);
    let cache_key = (base.clone(), model.to_string());
    let structured = req.schema.is_some();
    let mut format = LOCAL_FORMATS
        .lock()
        .ok()
        .and_then(|formats| formats.get(&cache_key).copied())
        .unwrap_or(LocalFormat::JsonSchema);
    loop {
        let payload = local_payload(req, model, format);
        let attempt = post_local(&client, &base, &headers, &payload, structured);
        let outcome = tokio::select! {
            r = attempt => r,
            _ = cancel.cancelled() => return Err(LlmError::cancelled()),
        };
        match outcome {
            Ok(mut reply) => {
                if structured {
                    if let Ok(mut formats) = LOCAL_FORMATS.lock() {
                        formats.insert(cache_key, format);
                    }
                    reply.text = reply.text.map(|t| extract_local_json(&t));
                }
                let estimate = estimated_prompt_tokens(req);
                if reply.prompt_tokens > 0 && estimate >= 6000 && reply.prompt_tokens * 2 < estimate
                {
                    // Ollama cuts prompts to its context length without an error. Its
                    // prompt cache also shrinks this count, so it only warns.
                    tracing::warn!(
                        "Local model read {} prompt tokens for a ~{estimate}-token prompt; the server may have truncated it (raise its context length)",
                        reply.prompt_tokens
                    );
                }
                // The user's own hardware: nothing is billed.
                reply.cost = Some(0.0);
                return Ok(reply);
            }
            Err(LocalFailure::FormatRejected) => match format.fallback() {
                Some(next) => {
                    tracing::info!("Local AI server refused {format:?} output; trying {next:?}");
                    format = next;
                }
                None => {
                    return Err(LlmError::fatal(format!(
                        "{LOCAL_LABEL} API error (400): the server refused the request"
                    )))
                }
            },
            Err(LocalFailure::Error(error)) => return Err(error),
        }
    }
}

/// Models a local server offers: `GET {base}/models` (OpenAI format), plus Ollama's
/// `/api/tags` when the URL looks like Ollama. Errors carry the same markers as chat.
pub async fn list_local_models(
    base_url: &str,
    api_key: Option<&str>,
) -> std::result::Result<Vec<String>, LlmError> {
    let base = normalize_local_base_url(base_url).map_err(LlmError::fatal)?;
    let client = local_http_client(&base, LOCAL_PROBE_TIMEOUT)?;
    let ai = AiSettings {
        local_api_key: api_key.map(str::to_string),
        ..Default::default()
    };
    let headers = local_headers(&ai);
    let get = |url: String| {
        let mut request = client.get(url);
        for (name, value) in &headers {
            request = request.header(*name, value);
        }
        local_send(request, MAX_CHAT_RESPONSE_BYTES)
    };
    let mut models: Vec<String> = Vec::new();
    let mut failure = None;
    let mut reached = false;
    match get(format!("{base}/models")).await {
        Ok((200, body)) => {
            reached = true;
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let items = body["data"].as_array().cloned().unwrap_or_default();
            models.extend(
                items
                    .iter()
                    .filter_map(|m| m["id"].as_str().map(str::to_string)),
            );
        }
        Ok((status, body)) => {
            failure = Some(match classify_local_error(status, &body, false) {
                LocalFailure::Error(e) => e,
                LocalFailure::FormatRejected => LlmError::fatal("Local AI server error"),
            })
        }
        Err(e) => failure = Some(e),
    }
    if looks_like_ollama(&base) {
        let root = base.strip_suffix("/v1").unwrap_or(&base);
        if let Ok((200, body)) = get(format!("{root}/api/tags")).await {
            reached = true;
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let items = body["models"].as_array().cloned().unwrap_or_default();
            models.extend(items.iter().filter_map(|m| {
                m["name"]
                    .as_str()
                    .or(m["model"].as_str())
                    .map(str::to_string)
            }));
        }
    }
    if !reached {
        return Err(failure.unwrap_or_else(|| LlmError::fatal(LOCAL_UNREACHABLE)));
    }
    let mut seen = std::collections::HashSet::new();
    models.retain(|m| {
        !m.is_empty()
            && m.chars().count() <= 200
            && !m.chars().any(char::is_control)
            && seen.insert(m.clone())
    });
    models.truncate(MAX_LOCAL_MODELS);
    Ok(models)
}

/// Ollama's default port, or a host named for it.
fn looks_like_ollama(base: &str) -> bool {
    url::Url::parse(base).is_ok_and(|url| {
        url.port() == Some(11434) || url.host_str().is_some_and(|h| h.contains("ollama"))
    })
}

// ---------------------------------------------------------------------------
// Anthropic (BYOK), streamed so long transcripts and 32k output don't time out
// ---------------------------------------------------------------------------

fn anthropic_payload(req: &ChatRequest, model: &str) -> Value {
    let blocks: Vec<Value> = req
        .user
        .iter()
        .map(|part| match part {
            ContentPart::Text(text) => json!({"type": "text", "text": text}),
            ContentPart::Image { mime, base64 } => json!({
                "type": "image",
                "source": {"type": "base64", "media_type": if mime.is_empty() { "image/jpeg" } else { mime }, "data": base64},
            }),
        })
        .collect();
    let mut request = Map::new();
    let model = if model.is_empty() {
        DEFAULT_ANTHROPIC_MODEL
    } else {
        model
    };
    request.insert("model".into(), Value::String(model.to_string()));
    request.insert("max_tokens".into(), json!(req.max_tokens.max(16000)));
    request.insert(
        "messages".into(),
        json!([{"role": "user", "content": blocks}]),
    );
    if let Some(system) = req.system.as_deref().filter(|s| !s.is_empty()) {
        request.insert("system".into(), Value::String(system.to_string()));
    }
    if let Some((_, schema)) = &req.schema {
        request.insert(
            "output_config".into(),
            json!({"format": {"type": "json_schema", "schema": schema}}),
        );
    }
    request.insert("stream".into(), Value::Bool(true));
    Value::Object(request)
}

/// Status codes the Anthropic SDK retries (max_retries=2).
fn anthropic_should_retry(status: u16) -> bool {
    matches!(status, 408 | 409 | 429) || status >= 500
}

fn anthropic_status_error(status: u16) -> LlmError {
    match status {
        401 => LlmError::fatal("Anthropic rejected the API key (invalid api key)"),
        429 => LlmError::retry("Anthropic rate limit reached"),
        s => {
            let message = format!("Anthropic API error ({s})");
            if s >= 500 {
                LlmError::retry(message)
            } else {
                LlmError::fatal(message)
            }
        }
    }
}

async fn anthropic_messages(
    base_url: &str,
    key: &str,
    payload: &Value,
    cancel: &CancellationToken,
) -> LlmResult<ChatReply> {
    const MAX_RETRIES: u32 = 2;
    let client = http_client()?;
    let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
    let body = serde_json::to_vec(payload)
        .map_err(|_| LlmError::fatal("Anthropic request could not be encoded"))?;
    let mut attempt = 0;
    let mut response = loop {
        let sent = client
            .post(&url)
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .body(body.clone())
            .send()
            .await;
        let (error, retry_after) = match sent {
            Ok(response) if response.status().as_u16() == 200 => break response,
            Ok(response) => {
                let status = response.status().as_u16();
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .filter(|s| s.is_finite() && (0.0..=60.0).contains(s));
                drop(response);
                if !anthropic_should_retry(status) || attempt >= MAX_RETRIES {
                    return Err(anthropic_status_error(status));
                }
                (anthropic_status_error(status), retry_after)
            }
            Err(_) => {
                if attempt >= MAX_RETRIES {
                    return Err(LlmError::retry("Anthropic connection failed"));
                }
                (LlmError::retry("Anthropic connection failed"), None)
            }
        };
        let delay = retry_after.unwrap_or_else(|| (0.5 * 2f64.powi(attempt as i32)).min(8.0));
        tracing::warn!("{error}; retrying in {delay:.1}s");
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs_f64(delay)) => {}
            _ = cancel.cancelled() => return Err(LlmError::cancelled()),
        }
        attempt += 1;
    };
    if !identity_encoding(&response) {
        return Err(LlmError::fatal(
            "Anthropic returned an unsupported response encoding",
        ));
    }

    let mut sse = SseParser::default();
    let mut state = AnthropicStream::default();
    let mut total = 0usize;
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                total += chunk.len();
                if total > MAX_STREAM_RESPONSE_BYTES {
                    return Err(LlmError::fatal("Anthropic response exceeds the size limit"));
                }
                for data in sse.push(&chunk) {
                    state.event(&data)?;
                }
                if state.stopped {
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => return Err(LlmError::retry("Anthropic connection failed")),
        }
    }
    state.finish()
}

/// Splits an SSE byte stream into `data:` payloads.
#[derive(Default)]
struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if let Some(data) = line.strip_prefix("data:") {
                events.push(data.trim_start().to_string());
            }
        }
        events
    }
}

#[derive(Default)]
struct AnthropicStream {
    model: String,
    text: String,
    stop_reason: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    stopped: bool,
}

impl AnthropicStream {
    fn event(&mut self, data: &str) -> LlmResult<()> {
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            return Ok(());
        };
        match event.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "message_start" => {
                let message = event.get("message");
                if let Some(model) = message
                    .and_then(|m| m.get("model"))
                    .and_then(|m| m.as_str())
                {
                    self.model = model.to_string();
                }
                let usage = message.and_then(|m| m.get("usage"));
                self.input_tokens = as_count(usage.and_then(|u| u.get("input_tokens")));
                self.output_tokens = as_count(usage.and_then(|u| u.get("output_tokens")));
            }
            "content_block_start" => {
                let block = event.get("content_block");
                if block.and_then(|b| b.get("type")).and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = block.and_then(|b| b.get("text")).and_then(|t| t.as_str()) {
                        self.text.push_str(text);
                    }
                }
            }
            "content_block_delta" => {
                let delta = event.get("delta");
                if delta.and_then(|d| d.get("type")).and_then(|t| t.as_str()) == Some("text_delta")
                {
                    if let Some(text) = delta.and_then(|d| d.get("text")).and_then(|t| t.as_str()) {
                        self.text.push_str(text);
                    }
                }
            }
            "message_delta" => {
                if let Some(reason) = event
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|r| r.as_str())
                {
                    self.stop_reason = Some(reason.to_string());
                }
                if let Some(usage) = event.get("usage") {
                    if usage.get("output_tokens").is_some() {
                        self.output_tokens = as_count(usage.get("output_tokens"));
                    }
                    if usage
                        .get("input_tokens")
                        .map(|v| !v.is_null())
                        .unwrap_or(false)
                    {
                        self.input_tokens = as_count(usage.get("input_tokens"));
                    }
                }
            }
            "message_stop" => self.stopped = true,
            "error" => {
                let kind = event
                    .get("error")
                    .and_then(|e| e.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                return Err(match kind {
                    "overloaded_error" | "api_error" => {
                        LlmError::retry("Anthropic API error (overloaded)")
                    }
                    "rate_limit_error" => LlmError::retry("Anthropic rate limit reached"),
                    "authentication_error" => {
                        LlmError::fatal("Anthropic rejected the API key (invalid api key)")
                    }
                    _ => LlmError::fatal("Anthropic API error (stream)"),
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> LlmResult<ChatReply> {
        if !self.stopped {
            return Err(LlmError::retry("Anthropic connection failed"));
        }
        if self.stop_reason.as_deref() == Some("refusal") {
            return Err(LlmError::fatal("The model declined this request"));
        }
        let finish = if self.stop_reason.as_deref() == Some("max_tokens") {
            "length"
        } else {
            "stop"
        };
        Ok(ChatReply {
            text: Some(self.text),
            finish_reason: Some(finish.into()),
            model: self.model,
            prompt_tokens: self.input_tokens,
            completion_tokens: self.output_tokens,
            total_tokens: self.input_tokens + self.output_tokens,
            cost: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Logged-in CLIs
// ---------------------------------------------------------------------------

fn cli_label(cli_id: &str) -> Option<&'static str> {
    CLI_PROVIDERS
        .iter()
        .find(|(id, _)| *id == cli_id)
        .map(|(_, label)| *label)
}

/// All text of the request, system first, joined by blank lines.
fn prompt_text(req: &ChatRequest) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(system) = &req.system {
        parts.push(system);
    }
    for part in &req.user {
        if let ContentPart::Text(text) = part {
            parts.push(text);
        }
    }
    parts.join("\n\n")
}

/// Python `json.dumps` spacing (", " and ": "), which the CLIs receive verbatim.
fn python_json(value: &Value) -> String {
    fn write(value: &Value, out: &mut String) {
        match value {
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    write(item, out);
                }
                out.push(']');
            }
            Value::Object(map) => {
                out.push('{');
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&ascii_json_string(k));
                    out.push_str(": ");
                    write(v, out);
                }
                out.push('}');
            }
            Value::String(s) => out.push_str(&ascii_json_string(s)),
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

/// JSON string with non-ASCII escaped as \uXXXX (Python's ensure_ascii).
fn ascii_json_string(s: &str) -> String {
    let encoded = Value::String(s.to_string()).to_string();
    let mut out = String::with_capacity(encoded.len());
    for ch in encoded.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            let mut buf = [0u16; 2];
            for unit in ch.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

/// Command line and stdin for one non-interactive, tool-free call.
fn cli_command(
    cli_id: &str,
    path: &Path,
    prompt: String,
    schema: Option<&Value>,
    model: &str,
    work: &Path,
) -> std::io::Result<(Vec<String>, Option<String>)> {
    let schema_json = schema.map(python_json);
    let path = path.to_string_lossy().into_owned();
    let s = |v: &str| v.to_string();
    match cli_id {
        "claude" => {
            let mut cmd = vec![
                path,
                s("--print"),
                s("--output-format"),
                s("json"),
                s("--tools"),
                s(""),
                s("--no-session-persistence"),
            ];
            if let Some(schema) = schema_json {
                cmd.extend([s("--json-schema"), schema]);
            }
            if !model.is_empty() {
                cmd.extend([s("--model"), s(model)]);
            }
            return Ok((cmd, Some(prompt)));
        }
        "codex" => {
            let last = work.join("last.txt").to_string_lossy().into_owned();
            let mut cmd = vec![
                path,
                s("exec"),
                s("--skip-git-repo-check"),
                s("--ephemeral"),
                s("-s"),
                s("read-only"),
                s("-o"),
                last,
            ];
            if let Some(schema) = schema_json {
                let schema_path = work.join("schema.json");
                std::fs::write(&schema_path, schema)?;
                cmd.extend([
                    s("--output-schema"),
                    schema_path.to_string_lossy().into_owned(),
                ]);
            }
            if !model.is_empty() {
                cmd.extend([s("-m"), s(model)]);
            }
            cmd.push(s("-"));
            return Ok((cmd, Some(prompt)));
        }
        "agy" => {
            // agy reads the prompt only from argv; Windows caps a command line at
            // ~32k chars, so long transcripts need macOS/Linux there.
            let mut cmd = vec![
                path,
                format!("-p={prompt}"),
                s("--output-format"),
                s("json"),
            ];
            if let Some(schema) = schema_json {
                cmd.extend([s("--json-schema"), schema]);
            }
            if !model.is_empty() {
                cmd.extend([s("--model"), s(model)]);
            }
            return Ok((cmd, None));
        }
        "grok" => {
            let prompt_path = work.join("prompt.txt");
            std::fs::write(&prompt_path, prompt)?;
            let mut cmd = vec![
                path,
                s("--prompt-file"),
                prompt_path.to_string_lossy().into_owned(),
                s("--output-format"),
                s("json"),
                s("--no-subagents"),
                s("--disable-web-search"),
            ];
            if let Some(schema) = schema_json {
                cmd.extend([s("--json-schema"), schema]);
            }
            if !model.is_empty() {
                cmd.extend([s("-m"), s(model)]);
            }
            return Ok((cmd, None));
        }
        _ => {}
    }
    // The remaining CLIs have no schema flag; the schema goes in the prompt.
    let mut prompt = prompt;
    if let Some(schema) = schema_json {
        prompt.push_str(&format!(
            "\n\nRespond with a single JSON object that matches this JSON Schema, and nothing else:\n{schema}"
        ));
    }
    let model_args = |flag: &str| {
        if model.is_empty() {
            vec![]
        } else {
            vec![s(flag), s(model)]
        }
    };
    match cli_id {
        "cursor-agent" => Ok((
            vec![
                path,
                s("-p"),
                s("--output-format"),
                s("json"),
                s("--mode"),
                s("ask"),
                s("--trust"),
                s("--model"),
                s(if model.is_empty() { "auto" } else { model }),
            ],
            Some(prompt),
        )),
        "opencode" => {
            let mut cmd = vec![path, s("run"), s("--format"), s("json")];
            cmd.extend(model_args("-m"));
            Ok((cmd, Some(prompt)))
        }
        "gemini" => {
            let mut cmd = vec![path, s("-p"), s(""), s("--output-format"), s("json")];
            cmd.extend(model_args("-m"));
            Ok((cmd, Some(prompt)))
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "unsupported cli",
        )),
    }
}

/// The CLI's final answer text. Errors carry no CLI output.
fn parse_cli_output(
    cli_id: &str,
    stdout: &str,
    work: &Path,
) -> std::result::Result<String, &'static str> {
    if cli_id == "codex" {
        return std::fs::read_to_string(work.join("last.txt")).map_err(|_| "missing output file");
    }
    if cli_id == "opencode" {
        let mut text = String::new();
        for line in stdout.lines() {
            let Ok(event) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if event.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = event
                    .get("part")
                    .and_then(|p| p.get("text"))
                    .and_then(|t| t.as_str())
                {
                    text.push_str(t);
                }
            }
        }
        return Ok(text);
    }
    let data: Value = serde_json::from_str(stdout).map_err(|_| "invalid json")?;
    let Some(obj) = data.as_object() else {
        return Err("invalid json");
    };
    if cli_id == "claude" && obj.get("is_error").map(truthy).unwrap_or(false) {
        return Err("cli reported an error");
    }
    if let Some(structured) = obj
        .get("structured_output")
        .or_else(|| obj.get("structuredOutput"))
        .filter(|v| !v.is_null())
    {
        return Ok(python_json(structured));
    }
    for key in ["result", "response", "text"] {
        if let Some(v) = obj.get(key).filter(|v| truthy(v)) {
            return Ok(match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
        }
    }
    Ok(String::new())
}

/// Variables a logged-in CLI needs beyond the media allowlist (`media::media_command`):
/// user identity and config dirs, proxies and CA bundles, Windows app-data paths.
/// Everything else, including provider API keys, is withheld.
const CLI_EXTRA_ENV: &[&str] = &[
    "USER",
    "LOGNAME",
    "SHELL",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "COMSPEC",
    "PATHEXT",
    "HOMEDRIVE",
    "HOMEPATH",
];

/// PATH for a CLI: its own directory first (node shims live next to it), then the
/// inherited PATH plus common install locations.
fn cli_path_env(binary: &Path) -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = binary.parent() {
        dirs.push(dir.to_path_buf());
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    #[cfg(unix)]
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        dirs.push(PathBuf::from(extra));
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

async fn read_capped<R: AsyncRead + Unpin>(mut reader: R, limit: usize) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 65536];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => return (out, false),
            Ok(n) => {
                if n > limit - out.len() {
                    return (out, true);
                }
                out.extend_from_slice(&buf[..n]);
            }
        }
    }
}

async fn cli_chat(
    cli_id: &str,
    ai: &AiSettings,
    req: &ChatRequest,
    cancel: &CancellationToken,
) -> LlmResult<ChatReply> {
    let label = cli_label(cli_id).unwrap_or(cli_id);
    let path = ai.cli_path.as_deref().filter(|p| p.is_file());
    let (Some(path), Some(_)) = (path, cli_label(cli_id)) else {
        return Err(LlmError::fatal(format!(
            "{label} CLI was not found. Check Settings → AI provider."
        )));
    };
    let schema = req.schema.as_ref().map(|(_, s)| s);
    let prompt = prompt_text(req);
    let work = tempfile::Builder::new()
        .prefix("clipward-cli-")
        .tempdir()
        .map_err(|_| LlmError::fatal(format!("{label} CLI could not be started")))?;
    let (cmd, stdin) = cli_command(cli_id, path, prompt.clone(), schema, &ai.model, work.path())
        .map_err(|_| LlmError::fatal(format!("{label} CLI could not be started")))?;
    tracing::info!(
        "Calling {label} CLI for planning ({} prompt chars)",
        prompt.chars().count()
    );

    let mut command = crate::media::media_command(Path::new(&cmd[0]));
    command
        .args(&cmd[1..])
        .current_dir(work.path())
        .env("PATH", cli_path_env(path))
        .env("NO_COLOR", "1")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in std::env::vars_os() {
        let upper = key.to_string_lossy().to_uppercase();
        if CLI_EXTRA_ENV.contains(&upper.as_str()) {
            command.env(key, value);
        }
    }
    #[cfg(windows)]
    {
        // Keep Windows from flashing a console for each call.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|_| LlmError::fatal(format!("{label} CLI could not be started")))?;

    let stdin_pipe = child.stdin.take();
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let input = stdin.unwrap_or_default().into_bytes();
    let writer = async move {
        if let Some(mut pipe) = stdin_pipe {
            // A CLI that exits early closes the pipe; its exit status tells the story.
            let _ = pipe.write_all(&input).await;
            let _ = pipe.shutdown().await;
        }
    };
    let run = async {
        let ((out, oversized), (err, _), ()) = tokio::join!(
            read_capped(stdout, MAX_CLI_OUTPUT_BYTES),
            read_capped(stderr, MAX_CLI_STDERR_BYTES),
            writer,
        );
        if oversized {
            return Err(LlmError::fatal(format!(
                "{label} CLI output exceeds the size limit"
            )));
        }
        let status = child
            .wait()
            .await
            .map_err(|_| LlmError::retry(format!("{label} CLI failed")))?;
        Ok((status, out, err))
    };
    let outcome = tokio::select! {
        r = tokio::time::timeout(CLI_TIMEOUT, run) => r,
        _ = cancel.cancelled() => return Err(LlmError::cancelled()),
    };
    let (status, out, err) = match outcome {
        Err(_) => return Err(LlmError::retry(format!("{label} CLI timed out"))),
        Ok(r) => r?,
    };
    if !status.success() {
        // stderr can hold account details; log only its size.
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        tracing::warn!(
            "{label} CLI exited with {code} ({} stderr bytes)",
            err.len()
        );
        let mut all = out.clone();
        all.extend_from_slice(&err);
        let lowered = String::from_utf8_lossy(&all).to_lowercase();
        if ["not logged in", "login", "unauthorized", "authenticate"]
            .iter()
            .any(|m| lowered.contains(m))
        {
            return Err(LlmError::fatal(format!(
                "{label} CLI is not logged in (unauthorized)"
            )));
        }
        return Err(LlmError::retry(format!("{label} CLI failed (exit {code})")));
    }
    let text =
        parse_cli_output(cli_id, &String::from_utf8_lossy(&out), work.path()).map_err(|kind| {
            LlmError::retry(format!("{label} CLI returned an unusable response: {kind}"))
        })?;
    // CLIs bill the user's own plan; their reported costs are list-price estimates.
    let model = if ai.model.is_empty() {
        format!("{cli_id} cli")
    } else {
        format!("{cli_id} cli ({})", ai.model)
    };
    Ok(ChatReply {
        text: Some(text),
        finish_reason: Some("stop".into()),
        model,
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        cost: Some(0.0),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod test_server {
    //! A tiny HTTP/1.1 server for offline provider tests.
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A canned reply: raw head (status line + headers) and body chunks.
    #[derive(Clone)]
    pub struct Reply {
        pub status: u16,
        pub headers: Vec<(String, String)>,
        pub chunks: Vec<Vec<u8>>,
        /// Close the connection without answering (a transport error).
        pub drop_connection: bool,
    }

    impl Reply {
        pub fn json(status: u16, body: &serde_json::Value) -> Self {
            Self::raw(status, vec![body.to_string().into_bytes()])
        }
        pub fn raw(status: u16, chunks: Vec<Vec<u8>>) -> Self {
            Self {
                status,
                headers: vec![],
                chunks,
                drop_connection: false,
            }
        }
        pub fn header(mut self, name: &str, value: &str) -> Self {
            self.headers.push((name.into(), value.into()));
            self
        }
        pub fn dropped() -> Self {
            Self {
                status: 0,
                headers: vec![],
                chunks: vec![],
                drop_connection: true,
            }
        }
    }

    #[derive(Debug, Clone)]
    pub struct Seen {
        pub head: String,
        pub body: Vec<u8>,
    }

    impl Seen {
        pub fn header(&self, name: &str) -> Option<String> {
            self.head.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim()
                    .eq_ignore_ascii_case(name)
                    .then(|| v.trim().to_string())
            })
        }
        pub fn json(&self) -> serde_json::Value {
            serde_json::from_slice(&self.body).unwrap()
        }
    }

    pub struct Server {
        pub url: String,
        pub seen: Arc<Mutex<Vec<Seen>>>,
        /// Body chunks the server managed to write, per request.
        pub written: Arc<Mutex<Vec<usize>>>,
    }

    pub async fn serve(replies: Vec<Reply>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let written = Arc::new(Mutex::new(Vec::new()));
        let (seen2, written2) = (seen.clone(), written.clone());
        tokio::spawn(async move {
            for reply in replies {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let mut data = Vec::new();
                let mut buf = vec![0u8; 65536];
                let head_end = loop {
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    data.extend_from_slice(&buf[..n]);
                    if let Some(p) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(p + 4);
                    }
                };
                let Some(head_end) = head_end else { continue };
                let head = String::from_utf8_lossy(&data[..head_end]).to_string();
                let len = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.trim()
                            .eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                while data.len() < head_end + len {
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                }
                seen2.lock().unwrap().push(Seen {
                    head,
                    body: data[head_end..].to_vec(),
                });
                if reply.drop_connection {
                    drop(sock);
                    written2.lock().unwrap().push(0);
                    continue;
                }
                let mut out = format!("HTTP/1.1 {} X\r\nconnection: close\r\n", reply.status);
                let explicit_length = reply
                    .headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("content-length"));
                for (k, v) in &reply.headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                if !explicit_length {
                    out.push_str("transfer-encoding: chunked\r\n");
                }
                out.push_str("\r\n");
                let mut count = 0;
                if sock.write_all(out.as_bytes()).await.is_ok() {
                    for chunk in &reply.chunks {
                        let framed = if explicit_length {
                            chunk.clone()
                        } else {
                            let mut f = format!("{:x}\r\n", chunk.len()).into_bytes();
                            f.extend_from_slice(chunk);
                            f.extend_from_slice(b"\r\n");
                            f
                        };
                        if sock.write_all(&framed).await.is_err() || sock.flush().await.is_err() {
                            break;
                        }
                        count += 1;
                        // Let the client react before the next chunk.
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                    if !explicit_length {
                        let _ = sock.write_all(b"0\r\n\r\n").await;
                    }
                }
                written2.lock().unwrap().push(count);
            }
        });
        Server { url, seen, written }
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::{serve, Reply};
    use super::*;

    fn openrouter(key: &str) -> AiSettings {
        AiSettings {
            provider: "openrouter".into(),
            openrouter_api_key: Some(key.into()),
            ..Default::default()
        }
    }

    fn req(base: &str) -> ChatRequest {
        ChatRequest {
            model: "test".into(),
            user: vec![ContentPart::Text("hi".into())],
            max_tokens: 10,
            base_url: Some(base.into()),
            ..Default::default()
        }
    }

    async fn call(replies: Vec<Reply>) -> (LlmResult<ChatReply>, test_server::Server) {
        let server = serve(replies).await;
        let result = chat(
            &openrouter("sk-secret-key"),
            &req(&server.url),
            &CancellationToken::new(),
        )
        .await;
        (result, server)
    }

    #[tokio::test]
    async fn valid_chunked_reply_and_usage() {
        let raw = json!({"choices": [], "usage": {"cost": 0, "prompt_tokens": 12}})
            .to_string()
            .into_bytes();
        let (result, server) = call(vec![Reply::raw(
            200,
            vec![raw[..7].to_vec(), raw[7..].to_vec()],
        )])
        .await;
        let reply = result.unwrap();
        assert_eq!(reply.cost, Some(0.0));
        assert_eq!(reply.prompt_tokens, 12);
        assert_eq!(reply.text, None);
        let seen = server.seen.lock().unwrap()[0].clone();
        assert_eq!(seen.header("accept-encoding").as_deref(), Some("identity"));
        assert_eq!(
            seen.header("authorization").as_deref(),
            Some("Bearer sk-secret-key")
        );
        assert!(seen.head.starts_with("POST /chat/completions "));
    }

    #[tokio::test]
    async fn oversized_body_stops_reading() {
        for status in [200u16, 400, 500] {
            let chunk = vec![b'x'; 1024 * 1024 + 10];
            let reply = Reply::raw(
                status,
                vec![
                    chunk.clone(),
                    chunk.clone(),
                    chunk.clone(),
                    b"never read".to_vec(),
                ],
            );
            let (result, server) = call(vec![reply]).await;
            let err = result.unwrap_err();
            assert!(err.message.contains("size limit"), "{err:?}");
            tokio::time::sleep(Duration::from_millis(150)).await;
            // The client hung up long before the server got to the last chunk.
            assert!(server.written.lock().unwrap()[0] < 4);
        }
    }

    #[tokio::test]
    async fn oversized_content_length_is_rejected() {
        let body = vec![b'x'; MAX_CHAT_RESPONSE_BYTES + 100];
        let reply =
            Reply::raw(200, vec![body.clone()]).header("content-length", &body.len().to_string());
        let (result, _) = call(vec![reply]).await;
        assert!(result.unwrap_err().message.contains("size limit"));
    }

    #[tokio::test]
    async fn compressed_response_is_rejected_before_reading() {
        let reply =
            Reply::raw(200, vec![vec![0x1f, 0x8b, 0, 0]]).header("content-encoding", "gzip");
        let (result, _) = call(vec![reply]).await;
        assert!(result.unwrap_err().message.contains("encoding"));
    }

    #[tokio::test]
    async fn invalid_json_and_shape_are_sanitized() {
        for raw in [&b"not-json private-token"[..], b"[]", b"null"] {
            let (result, _) = call(vec![Reply::raw(200, vec![raw.to_vec()])]).await;
            let err = result.unwrap_err();
            assert!(!err.message.contains("private-token"));
            assert!(err.message.starts_with("OpenRouter returned"));
        }
    }

    #[tokio::test]
    async fn provider_error_and_redirect_are_not_followed() {
        for status in [302u16, 429, 500] {
            let reply = Reply::raw(status, vec![b"private-provider-detail".to_vec()])
                .header("location", "https://attacker.invalid");
            let (result, server) = call(vec![reply]).await;
            let err = result.unwrap_err();
            assert!(!err.message.contains("private-provider-detail"));
            assert_eq!(err.message, format!("OpenRouter API error ({status})"));
            assert_eq!(err.retryable, status == 429 || status == 500);
            assert_eq!(server.seen.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn status_messages_carry_failure_markers() {
        let (r, _) = call(vec![Reply::raw(402, vec![b"{}".to_vec()])]).await;
        assert!(r.unwrap_err().message.contains("out of credits"));
        let (r, _) = call(vec![Reply::raw(401, vec![b"sk-secret-key".to_vec()])]).await;
        let err = r.unwrap_err();
        assert!(
            err.message.contains("(401)") && !err.retryable && !err.message.contains("sk-secret")
        );
        let (r, _) = call(vec![Reply::json(
            200,
            &json!({"error": {"message": "upstream"}}),
        )])
        .await;
        let err = r.unwrap_err();
        assert_eq!(err.message, "OpenRouter provider error");
        assert!(err.retryable);
        let (r, _) = call(vec![Reply::dropped()]).await;
        let err = r.unwrap_err();
        assert!(err.retryable && err.message == "OpenRouter request failed");
    }

    #[tokio::test]
    async fn missing_key_is_reported_without_a_request() {
        let ai = AiSettings {
            provider: "openrouter".into(),
            ..Default::default()
        };
        let err = chat(&ai, &req("http://127.0.0.1:1"), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.message.contains("invalid api key"));
        let ai = AiSettings {
            provider: "bogus".into(),
            ..Default::default()
        };
        assert!(
            chat(&ai, &req("http://127.0.0.1:1"), &CancellationToken::new())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn complete_estimates_missing_cost_and_sends_images() {
        let body = json!({
            "model": "openai/gpt-6-sol",
            "choices": [{"message": {"content": [{"type": "text", "text": "{\"a\":"}, {"type": "text", "text": "1}"}]}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1000, "completion_tokens": 100, "total_tokens": 1100},
        });
        let server = serve(vec![Reply::json(200, &body)]).await;
        // `complete` has no base-URL override, so exercise its body via chat() and
        // its accounting via the same pricing helper.
        let mut request = req(&server.url);
        request.user.push(ContentPart::Image {
            mime: "image/jpeg".into(),
            base64: "AAAA".into(),
        });
        request.schema = Some(("response".into(), json!({"type": "object"})));
        request.require_parameters = true;
        let reply = chat(&openrouter("k"), &request, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(reply.text.as_deref(), Some("{\"a\":1}"));
        assert_eq!(reply.cost, None);
        let (i, o) = model_pricing(&reply.model);
        assert!((1000.0 * i + 100.0 * o - 0.003).abs() < 1e-12);
        let sent = server.seen.lock().unwrap()[0].json();
        assert_eq!(
            sent["messages"][0]["content"][1]["image_url"]["url"],
            "data:image/jpeg;base64,AAAA"
        );
        assert_eq!(sent["response_format"]["json_schema"]["strict"], true);
        assert_eq!(sent["provider"]["require_parameters"], true);
        assert!(sent.get("temperature").is_none());
    }

    #[test]
    fn openai_payload_drops_routing_keys() {
        let mut r = req("x");
        r.fallback_models = vec!["a".into()];
        r.response_healing = true;
        r.require_parameters = true;
        r.reasoning_effort = Some("high".into());
        let p = openai_payload(&r, "");
        for key in ["plugins", "provider", "models", "reasoning", "max_tokens"] {
            assert!(p.get(key).is_none(), "{key}");
        }
        assert_eq!(p["model"], DEFAULT_OPENAI_MODEL);
        assert_eq!(p["max_completion_tokens"], 10);
    }

    #[tokio::test]
    async fn openai_uses_its_own_key_and_labels_errors() {
        let server = serve(vec![Reply::raw(401, vec![b"{}".to_vec()])]).await;
        let ai = AiSettings {
            provider: "openai".into(),
            openai_api_key: Some("sk-oa".into()),
            ..Default::default()
        };
        let err = chat(&ai, &req(&server.url), &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.message, "OpenAI API error (401)");
        let seen = server.seen.lock().unwrap()[0].clone();
        assert_eq!(
            seen.header("authorization").as_deref(),
            Some("Bearer sk-oa")
        );
        assert!(seen.json().get("max_completion_tokens").is_some());
    }

    fn sse(events: &[Value]) -> Vec<Vec<u8>> {
        events
            .iter()
            .map(|e| {
                format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e).into_bytes()
            })
            .collect()
    }

    fn anthropic_ai() -> AiSettings {
        AiSettings {
            provider: "anthropic".into(),
            anthropic_api_key: Some("sk-ant-secret".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn anthropic_stream_is_assembled() {
        let events = sse(&[
            json!({"type": "message_start", "message": {"model": "claude-opus-5", "usage": {"input_tokens": 50, "output_tokens": 1}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "{\"clips\""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": ": []}"}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "max_tokens"}, "usage": {"output_tokens": 7}}),
            json!({"type": "message_stop"}),
        ]);
        let server = serve(vec![
            Reply::raw(200, events).header("content-type", "text/event-stream")
        ])
        .await;
        let mut request = req(&server.url);
        request.system = Some("sys".into());
        request.schema = Some(("clip_plan".into(), json!({"type": "object"})));
        request.user.push(ContentPart::Image {
            mime: "image/png".into(),
            base64: "QUJD".into(),
        });
        let reply = chat(&anthropic_ai(), &request, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(reply.text.as_deref(), Some("{\"clips\": []}"));
        assert_eq!(reply.finish_reason.as_deref(), Some("length"));
        assert_eq!(
            (
                reply.prompt_tokens,
                reply.completion_tokens,
                reply.total_tokens
            ),
            (50, 7, 57)
        );
        assert_eq!(reply.model, "claude-opus-5");
        let seen = server.seen.lock().unwrap()[0].clone();
        assert_eq!(seen.header("x-api-key").as_deref(), Some("sk-ant-secret"));
        assert_eq!(
            seen.header("anthropic-version").as_deref(),
            Some("2023-06-01")
        );
        let body = seen.json();
        assert_eq!(body["max_tokens"], 16000);
        assert_eq!(body["system"], "sys");
        assert_eq!(body["model"], DEFAULT_ANTHROPIC_MODEL);
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(
            body["messages"][0]["content"][1]["source"]["media_type"],
            "image/png"
        );
    }

    #[tokio::test]
    async fn anthropic_errors_are_classified_and_retried() {
        let server = serve(vec![Reply::raw(401, vec![b"sk-ant-secret".to_vec()])]).await;
        let err = chat(
            &anthropic_ai(),
            &req(&server.url),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(err.message.contains("invalid api key") && !err.message.contains("sk-ant"));
        assert!(!err.retryable);

        let refusal = sse(&[
            json!({"type": "message_start", "message": {"model": "m", "usage": {"input_tokens": 1}}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "refusal"}, "usage": {"output_tokens": 0}}),
            json!({"type": "message_stop"}),
        ]);
        let server = serve(vec![
            Reply::raw(529, vec![]).header("retry-after", "0"),
            Reply::raw(200, refusal),
        ])
        .await;
        let err = chat(
            &anthropic_ai(),
            &req(&server.url),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.message, "The model declined this request");
        assert_eq!(server.seen.lock().unwrap().len(), 2);

        let server = serve(vec![Reply::raw(400, vec![])]).await;
        let err = chat(
            &anthropic_ai(),
            &req(&server.url),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.message, "Anthropic API error (400)");
    }

    #[test]
    fn prompt_text_joins_system_and_text_parts() {
        let mut r = req("x");
        r.system = Some("S".into());
        r.user.push(ContentPart::Image {
            mime: "image/jpeg".into(),
            base64: "x".into(),
        });
        r.user.push(ContentPart::Text("end".into()));
        assert_eq!(prompt_text(&r), "S\n\nhi\n\nend");
    }

    #[test]
    fn python_json_matches_json_dumps() {
        let v = json!({"a": [1, 2.5, "é"], "b": {"c": null, "d": true}});
        assert_eq!(
            python_json(&v),
            "{\"a\": [1, 2.5, \"\\u00e9\"], \"b\": {\"c\": null, \"d\": true}}"
        );
    }

    #[test]
    fn cli_commands_match_the_python_gateway() {
        let work = tempfile::tempdir().unwrap();
        let schema = json!({"type": "object"});
        let (cmd, stdin) = cli_command(
            "claude",
            Path::new("/bin/claude"),
            "P".into(),
            Some(&schema),
            "sonnet",
            work.path(),
        )
        .unwrap();
        assert_eq!(
            cmd,
            [
                "/bin/claude",
                "--print",
                "--output-format",
                "json",
                "--tools",
                "",
                "--no-session-persistence",
                "--json-schema",
                r#"{"type": "object"}"#,
                "--model",
                "sonnet"
            ]
        );
        assert_eq!(stdin.as_deref(), Some("P"));
        let (cmd, _) = cli_command(
            "codex",
            Path::new("/bin/codex"),
            "P".into(),
            Some(&schema),
            "",
            work.path(),
        )
        .unwrap();
        assert_eq!(cmd.last().unwrap(), "-");
        assert!(work.path().join("schema.json").is_file());
        let (cmd, stdin) = cli_command(
            "agy",
            Path::new("/bin/agy"),
            "P".into(),
            None,
            "",
            work.path(),
        )
        .unwrap();
        assert_eq!(cmd[1], "-p=P");
        assert!(stdin.is_none());
        let (_, stdin) = cli_command(
            "gemini",
            Path::new("/bin/g"),
            "P".into(),
            Some(&schema),
            "",
            work.path(),
        )
        .unwrap();
        assert!(stdin
            .unwrap()
            .ends_with("and nothing else:\n{\"type\": \"object\"}"));
        let (cmd, _) = cli_command(
            "cursor-agent",
            Path::new("/bin/c"),
            "P".into(),
            None,
            "",
            work.path(),
        )
        .unwrap();
        assert_eq!(cmd.last().unwrap(), "auto");
        assert!(cli_command("nope", Path::new("/x"), "P".into(), None, "", work.path()).is_err());
    }

    #[test]
    fn cli_output_parsing() {
        let work = tempfile::tempdir().unwrap();
        let p = work.path();
        assert_eq!(
            parse_cli_output("claude", r#"{"structured_output": {"a": 1}}"#, p).unwrap(),
            r#"{"a": 1}"#
        );
        assert_eq!(
            parse_cli_output("gemini", r#"{"response": "x"}"#, p).unwrap(),
            "x"
        );
        assert!(
            parse_cli_output("claude", r#"{"is_error": true, "result": "secret"}"#, p).is_err()
        );
        assert!(parse_cli_output("claude", "not json", p).is_err());
        let lines = "{\"type\":\"text\",\"part\":{\"text\":\"a\"}}\nnoise\n{\"type\":\"text\",\"part\":{\"text\":\"b\"}}";
        assert_eq!(parse_cli_output("opencode", lines, p).unwrap(), "ab");
        std::fs::write(p.join("last.txt"), "final").unwrap();
        assert_eq!(parse_cli_output("codex", "", p).unwrap(), "final");
    }

    #[cfg(unix)]
    fn script(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("claude");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn cli_ai(path: Option<PathBuf>) -> AiSettings {
        AiSettings {
            provider: "cli:claude".into(),
            cli_path: path,
            openrouter_api_key: Some("sk-leak".into()),
            ..Default::default()
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cli_runs_with_minimal_env_and_stdin_prompt() {
        let dir = tempfile::tempdir().unwrap();
        // Echo stdin length and whether a secret env var leaked through.
        let path = script(
            dir.path(),
            r#"input=$(cat); printf '{"result": "%s|%s|%s"}' "${#input}" "${OPENROUTER_API_KEY:-none}" "${NO_COLOR:-}""#,
        );
        std::env::set_var("OPENROUTER_API_KEY", "sk-env-secret");
        let mut r = req("x");
        r.system = Some("SYS".into());
        let reply = chat(&cli_ai(Some(path)), &r, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(reply.text.as_deref(), Some("7|none|1"));
        assert_eq!(reply.cost, Some(0.0));
        assert_eq!(reply.model, "claude cli");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cli_failures_carry_markers() {
        let dir = tempfile::tempdir().unwrap();
        let err = chat(&cli_ai(None), &req("x"), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.message.to_lowercase().contains("cli was not found"));

        let path = script(dir.path(), "echo 'Please run /login' >&2; exit 1");
        let err = chat(&cli_ai(Some(path)), &req("x"), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.message.to_lowercase().contains("cli is not logged in"));
        assert!(!err.message.contains("/login"));

        let path = script(dir.path(), "echo 'account 1234 boom' >&2; exit 3");
        let err = chat(&cli_ai(Some(path)), &req("x"), &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.message, "Claude Code CLI failed (exit 3)");
        assert!(err.retryable);

        let path = script(dir.path(), "cat >/dev/null; echo 'private-output'");
        let err = chat(&cli_ai(Some(path)), &req("x"), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.message.contains("unusable response") && !err.message.contains("private"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cli_is_killed_on_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "sleep 30");
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            c2.cancel();
        });
        let started = std::time::Instant::now();
        let err = chat(&cli_ai(Some(path)), &req("x"), &cancel)
            .await
            .unwrap_err();
        assert!(err.cancelled);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(matches!(EngineError::from(err), EngineError::Cancelled));
    }

    // ---- local AI (OpenAI-compatible server on this machine) ----

    fn local_ai(base: &str) -> AiSettings {
        AiSettings {
            provider: "local".into(),
            model: "qwen2.5:14b".into(),
            base_url: Some(base.into()),
            ..Default::default()
        }
    }

    fn structured_req() -> ChatRequest {
        ChatRequest {
            model: DEFAULT_OPENROUTER_MODEL.into(),
            system: Some("Plan clips.".into()),
            user: vec![ContentPart::Text("transcript".into())],
            max_tokens: 32000,
            schema: Some(("clip_plan".into(), json!({"type": "object"}))),
            reasoning_effort: Some("medium".into()),
            response_healing: true,
            require_parameters: true,
            fallback_models: vec!["x/y".into()],
            ..Default::default()
        }
    }

    fn ok_body(content: &str) -> Value {
        json!({
            "model": "qwen2.5:14b",
            "choices": [{"message": {"content": content}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120},
        })
    }

    fn ui_message(message: &str) -> String {
        crate::pipeline::describe_failure(&EngineError::Failed(message.into())).0
    }

    #[tokio::test]
    async fn local_sends_a_json_schema_request_and_costs_nothing() {
        let server = serve(vec![Reply::json(200, &ok_body("{\"clips\": []}"))]).await;
        let mut ai = local_ai(&format!("{}/v1/", server.url));
        ai.local_api_key = Some("lm-key".into());
        let reply = chat(&ai, &structured_req(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(reply.text.as_deref(), Some("{\"clips\": []}"));
        assert_eq!(reply.cost, Some(0.0));
        assert_eq!(reply.model, "qwen2.5:14b");
        let seen = server.seen.lock().unwrap()[0].clone();
        assert!(seen.head.starts_with("POST /v1/chat/completions "));
        assert_eq!(
            seen.header("authorization").as_deref(),
            Some("Bearer lm-key")
        );
        let sent = seen.json();
        assert_eq!(sent["model"], "qwen2.5:14b");
        assert_eq!(sent["stream"], false);
        assert_eq!(sent["response_format"]["type"], "json_schema");
        assert_eq!(sent["response_format"]["json_schema"]["name"], "clip_plan");
        // Text-only content goes as a plain string; the system prompt is unchanged.
        assert_eq!(sent["messages"][1]["content"], "transcript");
        assert_eq!(sent["messages"][0]["content"], "Plan clips.");
        for key in ["plugins", "provider", "models", "reasoning", "max_tokens"] {
            assert!(sent.get(key).is_none(), "{key}");
        }
    }

    #[tokio::test]
    async fn local_steps_down_when_the_server_rejects_schemas() {
        let server = serve(vec![
            Reply::json(
                400,
                &json!({"error": {"message": "response_format json_schema is not supported"}}),
            ),
            Reply::json(
                422,
                &json!({"error": "'response_format.type' must be 'json_schema'"}),
            ),
            Reply::json(
                200,
                &ok_body("<think>{maybe}</think>\n```json\n{\"clips\": [1]}\n```"),
            ),
            Reply::json(200, &ok_body("Sure! {\"clips\": [2]} Hope that helps.")),
        ])
        .await;
        let ai = local_ai(&server.url);
        let reply = chat(&ai, &structured_req(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(reply.text.as_deref(), Some("{\"clips\": [1]}"));
        // The accepted format is remembered: the next request skips the probing.
        let again = chat(&ai, &structured_req(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(again.text.as_deref(), Some("{\"clips\": [2]}"));
        let seen = server.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 4);
        assert_eq!(seen[0].json()["response_format"]["type"], "json_schema");
        assert_eq!(seen[1].json()["response_format"]["type"], "json_object");
        for request in &seen[2..] {
            let sent = request.json();
            assert!(sent.get("response_format").is_none());
            let system = sent["messages"][0]["content"].as_str().unwrap();
            assert!(system.starts_with("Plan clips.") && system.contains("JSON schema"));
        }
    }

    #[tokio::test]
    async fn local_reports_an_unreachable_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        drop(listener);
        let err = chat(
            &local_ai(&url),
            &structured_req(),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(!err.retryable);
        assert_eq!(
            ui_message(&err.message),
            "The local AI server is not reachable."
        );
        let err = list_local_models(&url, None).await.unwrap_err();
        assert_eq!(
            ui_message(&err.message),
            "The local AI server is not reachable."
        );
    }

    #[tokio::test]
    async fn local_reports_a_missing_model_without_echoing_the_body() {
        let server = serve(vec![Reply::json(
            404,
            &json!({"error": {"message": "model \"qwen2.5:14b\" not found, try pulling it first"}}),
        )])
        .await;
        let err = chat(
            &local_ai(&server.url),
            &structured_req(),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(!err.retryable && !err.message.contains("pulling"));
        assert_eq!(
            ui_message(&err.message),
            "The local AI server does not have the selected model."
        );
        // One request: a missing model is not a schema problem.
        assert_eq!(server.seen.lock().unwrap().len(), 1);
        let no_model = AiSettings {
            model: String::new(),
            ..local_ai(&server.url)
        };
        let err = chat(&no_model, &structured_req(), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.message.to_lowercase().contains("local model not found"));
    }

    #[tokio::test]
    async fn local_reports_a_context_overflow() {
        let server = serve(vec![
            Reply::json(
                400,
                &json!({"error": {"code": 400, "message": "the request exceeds the available context size, try increasing it", "type": "exceed_context_size_error"}}),
            ),
            Reply::json(
                400,
                &json!({"object": "error", "message": "This model's maximum context length is 8192 tokens. However, you requested 40000 tokens."}),
            ),
        ])
        .await;
        for _ in 0..2 {
            let err = chat(
                &local_ai(&server.url),
                &structured_req(),
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
            assert!(!err.retryable);
            assert_eq!(
                ui_message(&err.message),
                "The local model's context window is too small for this video's transcript."
            );
        }
        assert_eq!(server.seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn local_classifies_other_statuses() {
        let err = |status, body: &str| match classify_local_error(status, body.as_bytes(), true) {
            LocalFailure::Error(e) => e,
            LocalFailure::FormatRejected => panic!("format rejected"),
        };
        assert_eq!(
            ui_message(&err(401, "").message),
            "An API key was rejected."
        );
        assert_eq!(
            ui_message(&err(404, "404 page not found").message),
            "The local AI server has no chat endpoint at that URL."
        );
        assert_eq!(
            ui_message(
                &err(
                    500,
                    "model requires more system memory (30 GiB) than is available"
                )
                .message
            ),
            "The local model does not fit in this computer's memory."
        );
        assert!(err(503, "busy").retryable);
        match classify_local_error(400, b"bad", false) {
            LocalFailure::Error(e) => assert!(e.message.contains("(400)")),
            LocalFailure::FormatRejected => panic!("unstructured requests have no format"),
        }
        assert_eq!(
            classify_local_error(400, b"bad", true),
            LocalFailure::FormatRejected
        );
        assert_eq!(
            classify_local_error(500, b"failed to parse grammar", true),
            LocalFailure::FormatRejected
        );
    }

    #[tokio::test]
    async fn local_complete_follows_the_images_switch_and_is_free() {
        let request = || LlmRequest {
            user: "look".into(),
            images: vec![vec![1, 2, 3]],
            json_schema: Some(json!({"type": "object"})),
            max_tokens: 50,
            ..Default::default()
        };
        let server = serve(vec![
            Reply::json(200, &ok_body("{\"ok\": true}")),
            Reply::json(200, &ok_body("{\"ok\": true}")),
        ])
        .await;
        let mut ai = local_ai(&server.url);
        let response = complete(&ai, request(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!((response.cost_usd, response.cost_incomplete), (0.0, false));
        ai.local_images = true;
        complete(&ai, request(), &CancellationToken::new())
            .await
            .unwrap();
        let seen = server.seen.lock().unwrap().clone();
        assert!(seen[0].json()["messages"][0]["content"].is_string());
        assert_eq!(
            seen[1].json()["messages"][0]["content"][1]["type"],
            "image_url"
        );
        assert_eq!(seen[1].json()["max_tokens"], 50);
    }

    #[tokio::test]
    async fn local_models_are_listed() {
        let server = serve(vec![Reply::json(
            200,
            &json!({"object": "list", "data": [{"id": "qwen2.5:14b"}, {"id": "llama3.2"}, {"id": "qwen2.5:14b"}, {"id": ""}]}),
        )])
        .await;
        let models = list_local_models(&format!("{}/v1", server.url), Some("k"))
            .await
            .unwrap();
        assert_eq!(models, vec!["qwen2.5:14b", "llama3.2"]);
        let seen = server.seen.lock().unwrap()[0].clone();
        assert!(seen.head.starts_with("GET /v1/models "));
        assert_eq!(seen.header("authorization").as_deref(), Some("Bearer k"));
        assert!(looks_like_ollama("http://localhost:11434/v1"));
        assert!(!looks_like_ollama("http://localhost:1234/v1"));
    }

    #[test]
    fn local_base_urls_are_validated() {
        assert_eq!(
            normalize_local_base_url(" http://localhost:11434/v1/ ").unwrap(),
            "http://localhost:11434/v1"
        );
        for ok in [
            "https://gpu-box.local:8000/v1",
            "http://192.168.1.20:1234/v1",
            "http://[::1]:8080/v1",
            "https://llm.example.com/v1",
        ] {
            assert!(normalize_local_base_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "localhost:11434",
            "ftp://localhost/v1",
            "file:///tmp/x",
            "http://user:pass@localhost:11434/v1",
            "http://localhost:11434/v1?key=1",
            "http://localhost:11434/v1#x",
            "http://local host/v1",
        ] {
            assert!(normalize_local_base_url(bad).is_err(), "{bad}");
        }
        let long = format!("http://localhost/{}", "a".repeat(600));
        assert!(normalize_local_base_url(&long).is_err());
        for local in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:1/v1",
            "http://[::1]:1",
            "http://10.0.0.5/v1",
            "http://studio.local/v1",
            "http://gpu-box:8000/v1",
        ] {
            assert!(base_is_local(local), "{local}");
        }
        assert!(
            !base_is_local("https://api.example.com/v1") && !base_is_local("http://8.8.8.8/v1")
        );
    }

    #[test]
    fn local_json_is_extracted_from_wrapped_replies() {
        assert_eq!(extract_local_json("{\"a\": 1}"), "{\"a\": 1}");
        assert_eq!(
            extract_local_json("<think>{x}</think>{\"a\": 1}"),
            "{\"a\": 1}"
        );
        assert_eq!(extract_local_json("```json\n{\"a\": 1}\n```"), "{\"a\": 1}");
        assert_eq!(
            extract_local_json("Here: {\"a\": {\"b\": 2}} done"),
            "{\"a\": {\"b\": 2}}"
        );
        assert_eq!(extract_local_json("no json"), "no json");
    }
}
