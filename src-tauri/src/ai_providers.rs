//! AI provider readiness: logged-in CLI detection and a one-request provider test
//! (port of src/main/ai-providers.ts).
use crate::settings::AppSettings;
use crate::shared::AI_CLIS;
use crate::tools::find_binary;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Serialize)]
pub struct DetectedCli {
    pub id: String,
    pub label: String,
    /// Absolute path when installed, else null.
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProviderTestResult {
    pub ok: bool,
    pub message: String,
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
    let task = tauri::async_runtime::spawn(async move {
        clip_engine::llm::complete(&ai, request, &cancel).await
    });
    let result = tokio::time::timeout(Duration::from_secs(180), task).await;
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
        _ => ProviderTestResult {
            ok: false,
            message: "The provider check did not finish. Make sure the CLI is logged in, or check the key.".into(),
        },
    }
}
