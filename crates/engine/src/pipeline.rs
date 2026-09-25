//! Orchestration: download -> transcribe -> plan -> layout -> render -> result.json.
//! TODO(integration): port ai_clipping_pipeline.py + bridge_runner.py once the modules land.
use crate::error::{EngineError, Result};
use crate::types::{ClipJobRequest, EngineConfig, ProgressFn};
use tokio_util::sync::CancellationToken;

/// Runs one job and returns the JobOutput JSON (src/shared/job-output.ts), also saved as
/// `result.json` in `cfg.output_dir`.
pub async fn run_job(
    _req: ClipJobRequest,
    _cfg: EngineConfig,
    _progress: ProgressFn,
    _cancel: CancellationToken,
) -> Result<serde_json::Value> {
    Err(EngineError::msg("pipeline not ported yet"))
}

/// Fixed, safe UI text (message, hint) for an engine error. Port of bridge_runner.describe_failure.
pub fn describe_failure(_err: &EngineError) -> (String, String) {
    (
        "The clipping pipeline failed.".into(),
        "Check System check and retry. If it persists, report the steps that reproduce it.".into(),
    )
}
