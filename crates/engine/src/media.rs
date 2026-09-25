//! Subprocess helpers shared by every module that runs ffmpeg/ffprobe/yt-dlp or AI CLIs.
use crate::error::{EngineError, Result};
use std::path::Path;
use std::process::{Output, Stdio};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

/// Runs a command to completion, killing it when `cancel` fires. Stdin is null.
pub async fn run(program: &Path, args: &[String], cancel: &CancellationToken) -> Result<Output> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    tokio::select! {
        out = child.wait_with_output() => Ok(out?),
        _ = cancel.cancelled() => Err(EngineError::Cancelled),
    }
}

/// `ffprobe -show_format -show_streams` as JSON.
pub async fn probe(ffprobe: &Path, file: &Path, cancel: &CancellationToken) -> Result<serde_json::Value> {
    let args = ["-v", "error", "-print_format", "json", "-show_format", "-show_streams"]
        .iter()
        .map(|s| s.to_string())
        .chain([file.to_string_lossy().into_owned()])
        .collect::<Vec<_>>();
    let out = run(ffprobe, &args, cancel).await?;
    if !out.status.success() {
        return Err(EngineError::msg("ffprobe failed"));
    }
    serde_json::from_slice(&out.stdout).map_err(|_| EngineError::msg("ffprobe returned invalid json"))
}
