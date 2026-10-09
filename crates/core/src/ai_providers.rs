//! AI provider readiness: logged-in CLI detection, local AI server model listing
//! and a one-request provider test.
use crate::settings::AppSettings;
use crate::shared::AI_CLIS;
use crate::tools::find_binary;
use clip_engine::llm::ModelChoice;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct DetectedCli {
    pub id: String,
    pub label: String,
    /// Absolute path when installed, else null.
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderTestResult {
    pub ok: bool,
    pub message: String,
}

/// Models the local AI server offers (LocalModelsResult in src/shared/ai-providers.ts).
#[derive(Debug, Clone, Serialize)]
pub struct LocalModelsResult {
    pub ok: bool,
    pub models: Vec<String>,
    /// Fixed UI text when the server could not be read.
    pub message: String,
}

/// Probe the saved local AI server for its models, for the Settings picker.
pub async fn detect_local_models(settings: &AppSettings) -> LocalModelsResult {
    let key = Some(settings.local_api_key.as_str()).filter(|k| !k.is_empty());
    let base = settings.local_base_url.clone();
    let key = key.map(str::to_string);
    let task =
        tokio::spawn(
            async move { clip_engine::llm::list_local_models(&base, key.as_deref()).await },
        );
    match task.await {
        Ok(Ok(models)) => LocalModelsResult {
            ok: true,
            message: if models.is_empty() {
                "The server is running but offers no models. Pull or load one first.".into()
            } else {
                format!(
                    "Found {} model{}",
                    models.len(),
                    if models.len() == 1 { "" } else { "s" }
                )
            },
            models,
        },
        Ok(Err(error)) => {
            let (message, _) = clip_engine::pipeline::describe_failure(&error.into());
            LocalModelsResult {
                ok: false,
                models: Vec::new(),
                message,
            }
        }
        Err(_) => LocalModelsResult {
            ok: false,
            models: Vec::new(),
            message: "The local AI server could not be checked.".into(),
        },
    }
}

pub fn detect_clis(home: &Path) -> Vec<DetectedCli> {
    AI_CLIS
        .iter()
        .map(|(id, label, binaries)| DetectedCli {
            id: (*id).into(),
            label: (*label).into(),
            path: find_binary(home, binaries).map(|path| path.to_string_lossy().into_owned()),
        })
        .collect()
}

pub fn resolve_cli_path(home: &Path, provider: &str) -> Option<PathBuf> {
    let id = provider.strip_prefix("cli:")?;
    let (_, _, binaries) = AI_CLIS.iter().find(|(cli, _, _)| *cli == id)?;
    find_binary(home, binaries)
}

/// Why the selected provider can't run a job, or None when it can.
pub fn provider_problem(home: &Path, settings: &AppSettings) -> Option<&'static str> {
    if settings.ai_provider.starts_with("cli:") {
        return resolve_cli_path(home, &settings.ai_provider)
            .is_none()
            .then_some("The selected AI CLI is not installed. Choose another provider in Settings → AI provider.");
    }
    if settings.ai_provider == "local" {
        // The server itself is only checked by a request (Test connection, or the job).
        return settings
            .local_model
            .trim()
            .is_empty()
            .then_some("Choose the local model to use in Settings → AI provider.");
    }
    settings
        .provider_key_missing()
        .then_some("Add an API key for the selected AI provider in Settings → AI provider.")
}

/// One tiny structured-output request through the engine's own gateway, so the test
/// exercises exactly what a clipping job will run.
pub async fn test_provider(home: &Path, settings: &AppSettings) -> ProviderTestResult {
    if let Some(problem) = provider_problem(home, settings) {
        return ProviderTestResult {
            ok: false,
            message: problem.into(),
        };
    }
    let ai = settings.engine_ai(resolve_cli_path(home, &settings.ai_provider));
    let request = clip_engine::llm::LlmRequest {
        system: "You are a connectivity check. Reply with JSON only.".into(),
        user: "Reply with {\"ok\": true}.".into(),
        images: Vec::new(),
        json_schema: Some(serde_json::json!({
            "type": "object",
            "properties": { "ok": { "type": "boolean" } },
            "required": ["ok"],
            "additionalProperties": false
        })),
        max_tokens: 50,
        temperature: None,
    };
    let started = Instant::now();
    let cancel = tokio_util::sync::CancellationToken::new();
    let guard = cancel.clone().drop_guard();
    // A separate task, so a panic inside the engine becomes a failed check.
    let task = tokio::spawn(async move { clip_engine::llm::complete(&ai, request, &cancel).await });
    // A local server may first load the model from disk, which takes a while.
    let limit = if settings.ai_provider == "local" {
        600
    } else {
        180
    };
    let result = tokio::time::timeout(Duration::from_secs(limit), task).await;
    drop(guard);
    match result {
        Ok(Ok(Ok(_))) => ProviderTestResult {
            ok: true,
            message: format!("Connected in {:.1}s", started.elapsed().as_secs_f64()),
        },
        Ok(Ok(Err(error))) => {
            let (message, _) = clip_engine::pipeline::describe_failure(&error);
            ProviderTestResult { ok: false, message: message.chars().take(300).collect() }
        }
        _ if settings.ai_provider == "local" => ProviderTestResult {
            ok: false,
            message: "The local model did not answer in time. Check that the server has it loaded, then retry.".into(),
        },
        _ => ProviderTestResult {
            ok: false,
            message: "The provider check did not finish. Make sure the CLI is logged in, or check the key.".into(),
        },
    }
}

/// Model ids the selected provider offers: an API provider's list for this key, or a
/// CLI's own listing. Empty when there is none (no key, or a CLI without a listing),
/// so the renderer falls back to a free-text field.
pub async fn list_models(home: &Path, settings: &AppSettings) -> Result<Vec<ModelChoice>, String> {
    use clip_engine::llm::{ANTHROPIC_BASE_URL, OPENAI_BASE_URL, OPENROUTER_BASE_URL};
    if let Some(cli) = settings.ai_provider.strip_prefix("cli:") {
        let Some(path) = resolve_cli_path(home, &settings.ai_provider) else {
            return Ok(Vec::new());
        };
        return clip_engine::llm::list_cli_models(cli, &path).await;
    }
    let Some(key) = settings.provider_key().filter(|key| !key.is_empty()) else {
        return Ok(Vec::new());
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "Could not start the model request".to_string())?;
    let request = match settings.ai_provider.as_str() {
        "anthropic" => client
            .get(format!("{ANTHROPIC_BASE_URL}/v1/models?limit=1000"))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        "openai" => client
            .get(format!("{OPENAI_BASE_URL}/models"))
            .bearer_auth(key),
        _ => client
            .get(format!("{OPENROUTER_BASE_URL}/models"))
            .bearer_auth(key),
    };
    let response = request
        .send()
        .await
        .map_err(|_| "Could not reach the provider to list models".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "The provider refused the model list ({})",
            response.status().as_u16()
        ));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "The provider sent an unreadable model list".to_string())?;
    // All three answer { data: [{ id }] }. OpenAI also lists audio, image and
    // embedding models, which can't plan clips.
    const NOT_CHAT: &[&str] = &[
        "embedding",
        "whisper",
        "tts",
        "dall-e",
        "moderation",
        "image",
        "audio",
        "realtime",
        "transcribe",
        "davinci",
        "babbage",
    ];
    // OpenRouter and Anthropic also send a display `name` / `display_name`.
    let mut models: Vec<ModelChoice> = body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model["id"].as_str()?;
            let label = model["display_name"]
                .as_str()
                .or(model["name"].as_str())
                .unwrap_or(id);
            Some(ModelChoice::new(id, label))
        })
        .filter(|model| {
            settings.ai_provider != "openai" || !NOT_CHAT.iter().any(|word| model.id.contains(word))
        })
        .collect();
    models.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(provider: &str) -> AppSettings {
        let store = crate::settings::SettingsStore::new(
            &std::env::temp_dir().join(format!("clipward-ai-{}", uuid::Uuid::new_v4())),
            None,
            &std::env::temp_dir(),
        );
        AppSettings {
            ai_provider: provider.into(),
            ..store.load().unwrap()
        }
    }

    #[test]
    fn local_needs_a_model_but_no_key() {
        let home = std::env::temp_dir();
        let mut local = settings("local");
        assert!(provider_problem(&home, &local).is_some());
        local.local_model = "qwen2.5:14b".into();
        assert!(provider_problem(&home, &local).is_none());
        assert!(provider_problem(&home, &settings("openai")).is_some());
    }

    #[tokio::test]
    async fn unreachable_local_server_reports_fixed_text() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut local = settings("local");
        local.local_base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        drop(listener);
        let result = detect_local_models(&local).await;
        assert!(!result.ok && result.models.is_empty());
        assert_eq!(result.message, "The local AI server is not reachable.");
    }
}
