//! Subprocess helpers shared by every module that runs ffmpeg/ffprobe/yt-dlp or AI CLIs.
use crate::error::{EngineError, Result};
use std::ffi::OsString;
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::io::AsyncReadExt;
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

// ---- Untrusted media (port of services/media_process.py) ----

/// Input options for every ffmpeg/ffprobe run on caller-supplied media: only local
/// file protocols and container demuxers, so a disguised playlist (ffconcat, HLS)
/// cannot make the tool read other files or open network connections.
pub const MEDIA_INPUT_OPTIONS: [&str; 4] = [
    "-protocol_whitelist",
    "file,pipe,fd",
    "-format_whitelist",
    "mov,matroska,webm,avi,flv,mpegts",
];
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_TOOL_OUTPUT_BYTES: usize = 1024 * 1024;
pub const MAX_VIDEO_EDGE: u32 = 16384;
pub const MAX_VIDEO_PIXELS: u64 = 64 * 1024 * 1024;
pub const MAX_VIDEO_ASPECT: u64 = 32;

/// Environment variables media tools (and yt-dlp) may inherit. Provider keys never pass.
const ALLOWED_ENV: &[&str] = &[
    "PATH", "HOME", "TMPDIR", "TEMP", "TMP", "SYSTEMROOT", "WINDIR", "USERPROFILE",
    "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "FONTCONFIG_PATH", "FONTCONFIG_FILE",
    "LANG", "LC_ALL", "LC_CTYPE", "TZ",
];

/// Keeps only the allowlisted variables (case-insensitive names, as on Windows).
pub fn filter_child_env<I>(vars: I) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    vars.into_iter()
        .filter(|(key, _)| {
            key.to_str()
                .is_some_and(|k| ALLOWED_ENV.iter().any(|allowed| allowed.eq_ignore_ascii_case(k)))
        })
        .collect()
}

/// Minimal environment for a media child: use with `Command::env_clear().envs(..)`.
pub fn child_env() -> Vec<(OsString, OsString)> {
    filter_child_env(std::env::vars_os())
}

/// Rejects zero, huge, or extremely thin frames before any decoder allocates for them.
pub fn validate_video_dimensions(width: u32, height: u32) -> Result<()> {
    let (w, h) = (u64::from(width), u64::from(height));
    if w == 0
        || h == 0
        || w.max(h) > u64::from(MAX_VIDEO_EDGE)
        || w * h > MAX_VIDEO_PIXELS
        || w.max(h) > w.min(h) * MAX_VIDEO_ASPECT
    {
        return Err(EngineError::msg("video dimensions exceed supported limits"));
    }
    Ok(())
}

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(reader: R, max: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    reader.take(max as u64 + 1).read_to_end(&mut buf).await?;
    if buf.len() > max {
        return Err(std::io::Error::other("media output exceeded the size limit"));
    }
    Ok(buf)
}

/// Like [`run`], but with a minimal environment, a wall-clock limit and a cap on
/// stdout and stderr. The child is killed and reaped when any limit trips.
pub async fn run_bounded(
    program: &Path,
    args: &[String],
    timeout: Duration,
    max_output: usize,
    cancel: &CancellationToken,
) -> Result<Output> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .envs(child_env())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(EngineError::msg("media tool output is unavailable"));
    };
    let outcome = tokio::select! {
        r = async { tokio::try_join!(read_capped(stdout, max_output), read_capped(stderr, max_output)) } => {
            r.map_err(|_| EngineError::msg("media output exceeded the size limit"))
        }
        _ = tokio::time::sleep_until(deadline) => Err(EngineError::msg("media processing exceeded its time limit")),
        _ = cancel.cancelled() => Err(EngineError::Cancelled),
    };
    let (stdout, stderr) = match outcome {
        Ok(streams) => streams,
        Err(e) => {
            let _ = child.kill().await;
            return Err(e);
        }
    };
    let status = tokio::select! {
        s = child.wait() => s?,
        _ = tokio::time::sleep_until(deadline) => {
            let _ = child.kill().await;
            return Err(EngineError::msg("media processing exceeded its time limit"));
        }
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            return Err(EngineError::Cancelled);
        }
    };
    Ok(Output { status, stdout, stderr })
}

/// ffprobe JSON for an untrusted file: whitelisted protocols and demuxers, bounded
/// output and a 30 s limit. Errors carry no path.
pub async fn probe_untrusted(ffprobe: &Path, file: &Path, cancel: &CancellationToken) -> Result<serde_json::Value> {
    let mut args: Vec<String> = [
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_entries",
        "format=duration,filename,format_name:stream=codec_type,width,height,r_frame_rate",
        "-show_format",
        "-show_streams",
    ]
    .iter()
    .chain(MEDIA_INPUT_OPTIONS.iter())
    .map(|s| s.to_string())
    .collect();
    args.push(file.to_string_lossy().into_owned());
    let out = match run_bounded(ffprobe, &args, PROBE_TIMEOUT, MAX_TOOL_OUTPUT_BYTES, cancel).await {
        Ok(out) => out,
        Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
        Err(_) => return Err(EngineError::msg("ffprobe failed")),
    };
    if !out.status.success() {
        return Err(EngineError::msg("ffprobe failed"));
    }
    serde_json::from_slice(&out.stdout).map_err(|_| EngineError::msg("ffprobe returned invalid json"))
}

/// A command for a media tool with a minimal environment (no provider keys),
/// null stdin and kill-on-drop. Callers set stdout/stderr.
pub fn media_command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear().envs(child_env()).stdin(Stdio::null()).kill_on_drop(true);
    cmd
}
