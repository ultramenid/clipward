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

// ------------------------------------------------------------------
// Untrusted-input media tools (port of services/media_process.py)
// ------------------------------------------------------------------

/// Input options for FFmpeg reading untrusted media: local files/pipes only,
/// and only container demuxers the app actually produces or accepts.
pub const MEDIA_INPUT_OPTIONS: [&str; 4] =
    ["-protocol_whitelist", "file,pipe,fd", "-format_whitelist", "mov,matroska,webm,avi,flv,mpegts"];
/// Cap on diagnostics (stderr) or small stdout captured from a media tool.
pub const MAX_TOOL_OUTPUT_BYTES: usize = 1024 * 1024;
pub const MAX_VIDEO_EDGE: u64 = 16384;
pub const MAX_VIDEO_PIXELS: u64 = 64 * 1024 * 1024;
pub const MAX_VIDEO_ASPECT: u64 = 32;

/// Rejects dimensions that would blow up decode buffers or filter graphs.
pub fn validate_video_dimensions(width: u32, height: u32) -> Result<()> {
    let (w, h) = (width as u64, height as u64);
    if w == 0
        || h == 0
        || w.max(h) > MAX_VIDEO_EDGE
        || w * h > MAX_VIDEO_PIXELS
        || w.max(h) > w.min(h) * MAX_VIDEO_ASPECT
    {
        return Err(EngineError::msg("video dimensions exceed supported limits"));
    }
    Ok(())
}

const MEDIA_ALLOWED_ENV: [&str; 16] = [
    "PATH", "HOME", "TMPDIR", "TEMP", "TMP", "SYSTEMROOT", "WINDIR", "USERPROFILE", "LD_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH", "FONTCONFIG_PATH", "FONTCONFIG_FILE", "LANG", "LC_ALL", "LC_CTYPE", "TZ",
];

/// A command for a media tool with a minimal environment (no provider keys),
/// null stdin and kill-on-drop. Callers set stdout/stderr.
pub fn media_command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear();
    for (key, value) in std::env::vars_os() {
        let upper = key.to_string_lossy().to_uppercase();
        if MEDIA_ALLOWED_ENV.contains(&upper.as_str()) {
            cmd.env(key, value);
        }
    }
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    cmd
}
