//! Source video acquisition for the desktop app. Port of
//! `services/video_downloader.py` (+ the relevant bits of `services/media_process.py`),
//! without S3.
//!
//! Sources:
//! - local file (plain path or `file://` URL): used in place, never copied or deleted;
//! - YouTube and completed Twitch VODs: the `yt-dlp` binary;
//! - any other `http(s)` URL: a pinned reqwest client, redirects followed by hand.
//!
//! Every error message carries one of the lowercase marker phrases that
//! `pipeline::describe_failure` maps to fixed UI text ("video download failed",
//! "unsupported twitch source", "not enough disk space to save clips", ...). Messages
//! never include the source URL, a local path, or raw tool output.
//!
//! Residual risk (yt-dlp): the Python engine ran yt-dlp in-process behind a socket guard
//! that rejected every non-public address, including redirects and CDN hops. A child
//! process cannot be guarded that way. Here only YouTube/Twitch hosts reach yt-dlp, the
//! source host is resolved and checked with `net::resolve_public_destination` before
//! launch, proxies are disabled (`--proxy ""` plus a minimal environment without
//! `*_PROXY` variables), config files are ignored and `--exec` is cleared. yt-dlp still
//! does its own DNS lookups and follows the service's redirects and media URLs, so a
//! hostile DNS answer or redirect from those services could reach a private address.
use crate::error::{EngineError, Result};
use crate::media;
use crate::net;
use crate::types::{DownloadResult, EngineConfig, Progress, ProgressFn, Stage, VideoMetadata};
use serde_json::Value;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use url::Url;

/// Largest source accepted (bytes), across all partial and merged files.
pub const MAX_SOURCE_BYTES: u64 = 20 * 1000 * 1000 * 1000;
/// A download (metadata, transfer, merge and probe) must finish within this time.
pub const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(4 * 60 * 60);
/// Stop a download before it leaves the disk this close to full.
pub const MIN_FREE_BYTES: u64 = 1000 * 1000 * 1000;
/// Longest source the engine processes (settings.max_download_duration_seconds).
pub const MAX_DOWNLOAD_DURATION_SECONDS: f64 = 6.0 * 60.0 * 60.0;

const OUTPUT_FILENAME: &str = "source.mp4";
const MAX_REDIRECTS: usize = 5;
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// `yt-dlp -J` prints the whole info dict (formats, captions) on one line.
const MAX_YTDLP_STDOUT: usize = 64 * 1024 * 1024;
const MAX_STDERR_TAIL: usize = 16 * 1024;
const MAX_STDERR_LINE: usize = 64 * 1024;
const FREE_SPACE_CHECK_BYTES: u64 = 64 * 1024 * 1024;
const PROGRESS_MARKER: &str = "clipward-progress ";
const PROGRESS_START: f64 = 5.0;
const PROGRESS_SPAN: f64 = 10.0;

/// YouTube format selection. Filters only exclude AV1, which the bundled FFmpeg cannot
/// decode in software; the sort picks the best remaining stream. H.264 tops out at 1080p
/// on YouTube, so 1440p/2160p arrives as VP9. "res" measures the shorter side, so 2160
/// covers landscape 4K and vertical 2160x3840. SDR is preferred because renders are SDR
/// H.264 without tone mapping.
pub const YOUTUBE_FORMAT_SORT: &str = "hdr:SDR,res:2160,fps";
pub const YOUTUBE_FORMAT_SELECTORS: [&str; 2] = [
    // Best separate video + audio streams (the only way to get >720p).
    "bv*[vcodec!^=av01]+ba/b[vcodec!^=av01]",
    // If VP9/Opus fails to merge or convert, retry with H.264 + AAC (native to mp4, up
    // to 1080p), then any non-AV1 stream that already includes audio.
    "bv*[vcodec^=avc1]+ba[acodec^=mp4a]/b[vcodec!^=av01]",
];
const TWITCH_FORMAT_SELECTORS: [&str; 1] = ["b[vcodec!^=av01]"];
/// Re-checked at download time: a VOD that went live or back to processing stops the
/// run (exit 101) instead of downloading a partial broadcast.
const TWITCH_LIVE_FILTER: &str = "!is_live & live_status!=?is_live & live_status!=?is_upcoming \
     & live_status!=?post_live & live_status!=?processing";

const TWITCH_HOSTS: [&str; 4] = ["twitch.tv", "www.twitch.tv", "m.twitch.tv", "go.twitch.tv"];
const YOUTUBE_HOSTS: [&str; 6] =
    ["youtube.com", "www.youtube.com", "m.youtube.com", "music.youtube.com", "youtu.be", "www.youtu.be"];

const UA_LIST: [&str; 5] = [
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:89.0) Gecko/20100101 Firefox/89.0",
    "Mozilla/5.0 (X11; Linux x86_64; rv:89.0) Gecko/20100101 Firefox/89.0",
];

const DISK_FULL_MARKERS: [&str; 2] = ["no space left on device", "disk quota exceeded"];
const UNAVAILABLE_MARKERS: [&str; 5] =
    ["video unavailable", "private video", "members-only", "has been removed", "not available in your country"];
const NETWORK_MARKERS: [&str; 5] =
    ["timed out", "name resolution", "network is unreachable", "connection", "unable to download"];

// ---------------------------------------------------------------------------
// Errors (marker phrases only)

fn failed(detail: &str) -> EngineError {
    EngineError::msg(format!("video download failed: {detail}"))
}

fn disk_full() -> EngineError {
    EngineError::msg("not enough disk space to save clips")
}

fn too_long() -> EngineError {
    EngineError::msg("video duration exceeds maximum allowed duration")
}

fn deadline_exceeded() -> EngineError {
    failed("deadline exceeded")
}

fn source_too_large() -> EngineError {
    failed("source exceeds the 20 gb limit")
}

fn local_missing() -> EngineError {
    failed("local video not found")
}

fn twitch_unsupported() -> EngineError {
    EngineError::msg("unsupported twitch source")
}

fn twitch_not_completed() -> EngineError {
    EngineError::msg("twitch vod is not completed")
}

fn twitch_duration() -> EngineError {
    EngineError::msg("twitch vod duration is invalid or too long")
}

fn twitch_unavailable() -> EngineError {
    EngineError::msg("twitch vod unavailable")
}

fn io_error(e: std::io::Error) -> EngineError {
    if matches!(e.kind(), ErrorKind::StorageFull | ErrorKind::QuotaExceeded) {
        disk_full()
    } else {
        failed("the video could not be saved")
    }
}

fn net_error(e: EngineError) -> EngineError {
    match e {
        EngineError::Cancelled => EngineError::Cancelled,
        EngineError::Failed(m) if m.contains("timed out") => failed("source hostname lookup timed out"),
        EngineError::Failed(_) => failed("video destination is not public"),
    }
}

fn http_error(e: reqwest::Error) -> EngineError {
    if e.is_timeout() {
        failed("request timed out")
    } else if e.is_connect() {
        failed("connection failed")
    } else {
        failed("transfer interrupted")
    }
}

// ---------------------------------------------------------------------------
// Source detection

#[derive(Debug, Clone, PartialEq)]
enum Source {
    Local(PathBuf),
    YouTube(String),
    /// Canonical `https://www.twitch.tv/videos/<id>` URL.
    Twitch(String),
    Direct(String),
}

/// Host of an http(s)-looking string even when a strict parser rejects it (bad port).
fn lenient_host(raw: &str) -> String {
    let rest = raw.split_once("://").map_or(raw, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        host_port.rsplit_once(':').map_or(host_port, |(host, _)| host)
    };
    host.to_ascii_lowercase()
}

/// Canonical VOD URL for any twitch.tv address; `Ok(None)` when the host is not Twitch.
/// Twitch pages other than a saved video (channels, clips, collections, odd ports,
/// credentials) are rejected rather than downloaded as a direct URL.
pub fn twitch_vod_url(url: &str) -> Result<Option<String>> {
    let host = lenient_host(url);
    let bare = host.trim_end_matches('.');
    if bare != "twitch.tv" && !bare.ends_with(".twitch.tv") {
        return Ok(None);
    }
    let parsed = Url::parse(url).map_err(|_| twitch_unsupported())?;
    let id = parsed
        .path()
        .strip_prefix("/videos/")
        .map(|rest| rest.strip_suffix('/').unwrap_or(rest))
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
    let host_ok = parsed.host_str().is_some_and(|h| TWITCH_HOSTS.contains(&h));
    // `Url` drops default ports, so any explicit port is a non-default one.
    let valid = host_ok
        && matches!(parsed.scheme(), "http" | "https")
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.port().is_none();
    match id {
        Some(id) if valid => Ok(Some(format!("https://www.twitch.tv/videos/{id}"))),
        _ => Err(twitch_unsupported()),
    }
}

fn detect_source(input: &str) -> Result<Source> {
    if input.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("file://")) {
        let url = Url::parse(input).map_err(|_| local_missing())?;
        return url.to_file_path().map(Source::Local).map_err(|_| local_missing());
    }
    let lower = input.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        // The desktop app only sends links or files the user picked.
        return Ok(Source::Local(PathBuf::from(input)));
    }
    if let Some(canonical) = twitch_vod_url(input)? {
        return Ok(Source::Twitch(canonical));
    }
    let youtube = Url::parse(input)
        .ok()
        .and_then(|u| u.host_str().map(|h| YOUTUBE_HOSTS.contains(&h)))
        .unwrap_or(false);
    Ok(if youtube { Source::YouTube(input.to_string()) } else { Source::Direct(input.to_string()) })
}

// ---------------------------------------------------------------------------
// Limits

fn duration_limit(requested: Option<f64>) -> f64 {
    match requested {
        Some(v) if v.is_finite() && v > 0.0 => v.min(MAX_DOWNLOAD_DURATION_SECONDS),
        _ => MAX_DOWNLOAD_DURATION_SECONDS,
    }
}

fn check_duration(duration: f64, limit: f64) -> Result<()> {
    if duration.is_finite() && duration > limit {
        return Err(too_long());
    }
    if !(duration > 0.0 && duration <= limit) {
        return Err(failed("invalid video duration"));
    }
    Ok(())
}

fn check_source_size(bytes: u64) -> Result<()> {
    if bytes > MAX_SOURCE_BYTES {
        return Err(source_too_large());
    }
    Ok(())
}

fn free_space_ok(available: u64, remaining: u64) -> bool {
    available.saturating_sub(remaining) >= MIN_FREE_BYTES
}

/// Fail early, and say why, rather than filling the disk mid-download.
fn check_free_space(dir: &Path, remaining: u64) -> Result<()> {
    match fs4::available_space(dir) {
        Ok(available) if !free_space_ok(available, remaining) => Err(disk_full()),
        _ => Ok(()),
    }
}

/// Python-style `float(value)` that falls back to `default` for missing, non-numeric
/// and non-finite values.
fn finite_number(value: Option<&Value>, default: f64) -> f64 {
    let number = match value {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        Some(Value::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    };
    number.filter(|n| n.is_finite()).unwrap_or(default)
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// Port of `_validate_twitch_info`: only completed VODs with a usable duration.
fn validate_twitch_info(info: &Value, max_duration: f64) -> Result<()> {
    let Some(obj) = info.as_object() else {
        return Err(twitch_unavailable());
    };
    // Archived broadcasts often have was_live=true and is_live=null.
    let live_status = obj.get("live_status").and_then(Value::as_str);
    if truthy(obj.get("is_live"))
        || matches!(live_status, Some("is_live" | "is_upcoming" | "post_live" | "processing"))
    {
        return Err(twitch_not_completed());
    }
    let duration = finite_number(obj.get("duration"), 0.0);
    if !(duration > 0.0 && duration <= max_duration) {
        return Err(twitch_duration());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Files

/// `source.mp4` plus everything yt-dlp may leave next to it: `.part`/`.ytdl` files,
/// per-format streams (`source.f137.mp4`) and merge/convert temporaries.
fn download_paths(output: &Path) -> Vec<PathBuf> {
    let (Some(name), Some(stem)) =
        (output.file_name().and_then(|n| n.to_str()), output.file_stem().and_then(|s| s.to_str()))
    else {
        return Vec::new();
    };
    let dir = match output.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let prefixes = [format!("{name}."), format!("{stem}.f"), format!("{stem}.temp.")];
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file() || t.is_symlink()))
        .filter(|entry| {
            entry.file_name().to_str().is_some_and(|n| n == name || prefixes.iter().any(|p| n.starts_with(p)))
        })
        .map(|entry| entry.path())
        .collect()
}

fn stored_bytes(output: &Path) -> u64 {
    download_paths(output)
        .iter()
        .filter_map(|p| std::fs::symlink_metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

fn remove_partial_files(output: &Path) {
    for path in download_paths(output) {
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!(kind = ?e.kind(), "could not remove a partial download");
        }
    }
}

/// yt-dlp can append its own extension; move the result to `output`.
fn locate_output(output: &Path) -> bool {
    if output.is_file() {
        return true;
    }
    let base = output.as_os_str().to_owned();
    for ext in [".mp4", ".webm", ".mkv"] {
        let mut candidate = base.clone();
        candidate.push(ext);
        let candidate = PathBuf::from(candidate);
        if candidate.is_file() {
            return std::fs::rename(&candidate, output).is_ok();
        }
    }
    false
}

fn file_title(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "video".into())
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn url_title(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .map(|segment| file_title(Path::new(&percent_decode(segment))))
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "video".into())
}

fn report(progress: &ProgressFn, fraction: f64, step: &str) {
    progress(Progress {
        stage: Stage::Downloading,
        percent: PROGRESS_START + PROGRESS_SPAN * fraction.clamp(0.0, 1.0),
        step: step.to_string(),
        clips_done: 0,
        clips_total: 0,
    });
}

// ---------------------------------------------------------------------------
// Metadata

fn parse_fps(rate: Option<&Value>) -> f64 {
    let fps = match rate.and_then(Value::as_str) {
        Some(text) => match text.split_once('/') {
            Some((num, den)) => match (num.trim().parse::<f64>(), den.trim().parse::<f64>()) {
                (Ok(n), Ok(d)) if d != 0.0 => n / d,
                _ => 30.0,
            },
            None => text.trim().parse::<f64>().unwrap_or(30.0),
        },
        None => 30.0,
    };
    if fps.is_finite() && fps > 0.0 {
        fps
    } else {
        30.0
    }
}

fn metadata_from_probe(info: &Value, title: String) -> Result<VideoMetadata> {
    let streams = info.get("streams").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let kind = |s: &&Value| s.get("codec_type").and_then(Value::as_str).map(str::to_owned);
    let video = streams
        .iter()
        .find(|s| kind(s).as_deref() == Some("video"))
        .ok_or_else(|| failed("source has no video stream"))?;
    let has_audio = streams.iter().any(|s| kind(&s).as_deref() == Some("audio"));
    let invalid = || failed("invalid video metadata");
    let duration = finite_number(info.get("format").and_then(|f| f.get("duration")), 0.0);
    let dimension = |key: &str| {
        video
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0)
    };
    let (Some(width), Some(height)) = (dimension("width"), dimension("height")) else {
        return Err(invalid());
    };
    if duration <= 0.0 {
        return Err(invalid());
    }
    media::validate_video_dimensions(width, height).map_err(|_| invalid())?;
    Ok(VideoMetadata {
        title,
        duration_seconds: duration,
        width,
        height,
        fps: parse_fps(video.get("r_frame_rate")),
        has_audio,
    })
}

/// Real dimensions, duration and audio presence of a media file (ffprobe with the
/// protocol/demuxer whitelist, bounded output and a 30 s limit). Title is the file stem.
pub async fn probe_metadata(ffprobe: &Path, path: &Path, cancel: &CancellationToken) -> Result<VideoMetadata> {
    let info = match media::probe_untrusted(ffprobe, path, cancel).await {
        Ok(info) => info,
        Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
        Err(_) => return Err(failed("metadata probe failed")),
    };
    metadata_from_probe(&info, file_title(path))
}

// ---------------------------------------------------------------------------
// Public entry point

/// Fetches `url` (local path, `file://`, YouTube, Twitch VOD, or direct http(s) link)
/// to `cfg.work_dir/source.mp4` and returns its probed metadata. Local files are used
/// in place (`is_local: true`). Partial files are removed on any failure or cancel.
pub async fn download(
    url: &str,
    cfg: &EngineConfig,
    max_duration_seconds: Option<f64>,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> Result<DownloadResult> {
    let limit = duration_limit(max_duration_seconds);
    let source = detect_source(url)?;
    report(progress, 0.0, "Downloading video...");
    match &source {
        Source::Local(path) => {
            let result = use_local_file(path, cfg, cancel).await?;
            check_duration(result.metadata.duration_seconds, limit)?;
            report(progress, 1.0, "Using local video");
            Ok(result)
        }
        Source::YouTube(u) | Source::Twitch(u) => {
            // yt-dlp resolves hosts itself; check the source host before handing it over.
            net::resolve_public_destination(u).await.map_err(net_error)?;
            fetch_remote(&source, cfg, limit, progress, cancel).await
        }
        // Every hop is validated and pinned in `download_direct`.
        Source::Direct(_) => fetch_remote(&source, cfg, limit, progress, cancel).await,
    }
}

async fn use_local_file(path: &Path, cfg: &EngineConfig, cancel: &CancellationToken) -> Result<DownloadResult> {
    let path = std::path::absolute(path).map_err(|_| local_missing())?;
    let meta = tokio::fs::metadata(&path).await.map_err(|_| local_missing())?;
    if !meta.is_file() {
        return Err(local_missing());
    }
    check_source_size(meta.len())?;
    let metadata = probe_metadata(&cfg.ffprobe, &path, cancel).await?;
    tracing::info!(duration = metadata.duration_seconds, "using local video");
    Ok(DownloadResult { path, metadata, is_local: true })
}

async fn fetch_remote(
    source: &Source,
    cfg: &EngineConfig,
    limit: f64,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> Result<DownloadResult> {
    tokio::fs::create_dir_all(&cfg.work_dir).await.map_err(io_error)?;
    let output = cfg.work_dir.join(OUTPUT_FILENAME);
    let result = fetch_remote_inner(source, cfg, &output, limit, progress, cancel).await;
    if result.is_err() {
        remove_partial_files(&output);
    }
    result
}

async fn fetch_remote_inner(
    source: &Source,
    cfg: &EngineConfig,
    output: &Path,
    limit: f64,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> Result<DownloadResult> {
    let deadline = Instant::now() + DOWNLOAD_DEADLINE;
    check_free_space(&cfg.work_dir, 0)?;
    let title = match source {
        Source::YouTube(url) | Source::Twitch(url) => {
            let ytdlp = cfg.ytdlp.as_deref().ok_or_else(|| failed("yt-dlp is not installed"))?;
            let job = YtdlpJob {
                ytdlp,
                ffmpeg: &cfg.ffmpeg,
                work_dir: &cfg.work_dir,
                output,
                url,
                twitch: matches!(source, Source::Twitch(_)),
                max_duration: limit,
                deadline,
                progress,
                cancel,
            };
            download_with_ytdlp(&job).await?
        }
        Source::Direct(url) => download_direct(url, output, &cfg.work_dir, deadline, progress, cancel).await?,
        Source::Local(_) => return Err(failed("unsupported source")),
    };
    let size = std::fs::metadata(output).map_err(|_| failed("output file not found"))?.len();
    check_source_size(size)?;
    tracing::info!(mb = size as f64 / 1024.0 / 1024.0, "video downloaded");
    report(progress, 0.95, "Reading video metadata...");
    let mut metadata = probe_metadata(&cfg.ffprobe, output, cancel).await?;
    if Instant::now() > deadline {
        return Err(deadline_exceeded());
    }
    if metadata.height < 720 {
        tracing::warn!(height = metadata.height, "downloaded video is below 720p");
    }
    metadata.title = title;
    check_duration(metadata.duration_seconds, limit)?;
    report(progress, 1.0, "Video downloaded");
    Ok(DownloadResult { path: output.to_path_buf(), metadata, is_local: false })
}

// ---------------------------------------------------------------------------
// yt-dlp (YouTube, Twitch)

struct YtdlpJob<'a> {
    ytdlp: &'a Path,
    ffmpeg: &'a Path,
    work_dir: &'a Path,
    output: &'a Path,
    url: &'a str,
    twitch: bool,
    max_duration: f64,
    deadline: Instant,
    progress: &'a ProgressFn,
    cancel: &'a CancellationToken,
}

/// Options shared by the metadata and download runs: no config files, no playlists,
/// no proxy, native HLS/DASH downloaders (no external curl/aria2c/ffmpeg fetching),
/// no `--exec` hooks.
fn common_args(twitch: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "--no-config",
        "--no-playlist",
        "--proxy",
        "",
        "--downloader",
        "native",
        "--hls-prefer-native",
        "--socket-timeout",
        "30",
        "--no-exec",
        "--quiet",
        "--no-warnings",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if twitch {
        args.extend(["--use-extractors".into(), "twitch:vod".into()]);
    }
    args
}

fn info_args(url: &str, twitch: bool) -> Vec<String> {
    let mut args = common_args(twitch);
    args.extend(["-J".into(), "--".into(), url.into()]);
    args
}

fn pick_user_agent() -> &'static str {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    UA_LIST[nanos % UA_LIST.len()]
}

/// Absolute form of a configured tool path, so it survives the child's `current_dir`.
/// Bare names ("yt-dlp") stay as-is for PATH lookup.
fn tool_path(path: &Path) -> PathBuf {
    if path.components().count() > 1 {
        std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

fn download_args(
    url: &str,
    twitch: bool,
    selector: &str,
    ffmpeg: &Path,
    output_name: &str,
    max_duration: f64,
) -> Vec<String> {
    let mut args = common_args(twitch);
    args.extend(
        [
            "--format",
            selector,
            "--format-sort",
            YOUTUBE_FORMAT_SORT,
            "--user-agent",
            pick_user_agent(),
            "--add-headers",
            "Accept-Language:en-US,en;q=0.9",
            "--add-headers",
            "Accept:text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            "--merge-output-format",
            "mp4",
            "--recode-video",
            "mp4",
            "--retries",
            "10",
            "--fragment-retries",
            "10",
            "--force-overwrites",
            "--progress",
            "--newline",
            "--progress-template",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    args.push(format!(
        "download:{PROGRESS_MARKER}%(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s"
    ));
    args.extend(["--max-filesize".into(), MAX_SOURCE_BYTES.to_string()]);
    args.extend(["--ffmpeg-location".into(), tool_path(ffmpeg).to_string_lossy().into_owned()]);
    // Relative to the child's working directory (the job's work_dir), so no path
    // characters are interpreted as output-template syntax.
    args.extend(["--output".into(), output_name.replace('%', "%%")]);
    if twitch {
        args.extend([
            "--abort-on-unavailable-fragments".into(),
            "--break-match-filters".into(),
            TWITCH_LIVE_FILTER.into(),
            "--match-filters".into(),
            format!("duration>0 & duration<={}", max_duration.floor() as u64),
        ]);
    }
    args.extend(["--".into(), url.into()]);
    args
}

#[derive(Debug, Clone, PartialEq)]
enum YtdlpFailure {
    DiskFull,
    Blocked,
    Format,
    Network(String),
    Other(Option<String>),
}

fn http_status(text: &str) -> Option<String> {
    let rest = &text[text.find("http error ")? + "http error ".len()..];
    let code: String = rest.chars().take(3).collect();
    (code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit())).then_some(code)
}

/// Sorts yt-dlp's stderr into the Python retry categories. Only fixed marker phrases
/// (and an HTTP status code) survive; URLs and tokens in the text are dropped.
fn classify_ytdlp_failure(stderr: &str) -> YtdlpFailure {
    let text = stderr.to_lowercase();
    if DISK_FULL_MARKERS.iter().any(|m| text.contains(m)) {
        return YtdlpFailure::DiskFull;
    }
    if text.contains("sign in to confirm") || text.contains("not a bot") {
        return YtdlpFailure::Blocked;
    }
    if text.contains("requested format") || text.contains("no video formats") {
        return YtdlpFailure::Format;
    }
    if let Some(code) = http_status(&text) {
        return YtdlpFailure::Network(format!("http error {code}"));
    }
    if let Some(m) = NETWORK_MARKERS.iter().find(|m| text.contains(*m)) {
        return YtdlpFailure::Network(m.to_string());
    }
    YtdlpFailure::Other(UNAVAILABLE_MARKERS.iter().find(|m| text.contains(*m)).map(|m| m.to_string()))
}

fn ytdlp_error(failure: &YtdlpFailure, twitch: bool) -> EngineError {
    match failure {
        YtdlpFailure::DiskFull => disk_full(),
        _ if twitch => twitch_unavailable(),
        YtdlpFailure::Blocked => failed("youtube is blocking this request"),
        YtdlpFailure::Format => failed("no compatible video format"),
        YtdlpFailure::Network(marker) | YtdlpFailure::Other(Some(marker)) => failed(marker),
        YtdlpFailure::Other(None) => EngineError::msg("video download failed"),
    }
}

async fn fetch_info(job: &YtdlpJob<'_>) -> Result<Value> {
    let run = run_ytdlp(job.ytdlp, &info_args(job.url, job.twitch), job.work_dir, job.deadline, job.cancel, None)
        .await?;
    if !run.status.success() {
        return Err(ytdlp_error(&classify_ytdlp_failure(&run.stderr_tail), job.twitch));
    }
    let info: Value = match serde_json::from_slice(&run.stdout) {
        Ok(v @ Value::Object(_)) => v,
        _ if job.twitch => return Err(twitch_unavailable()),
        _ => return Err(failed("video info was invalid")),
    };
    if job.twitch {
        validate_twitch_info(&info, MAX_DOWNLOAD_DURATION_SECONDS)?;
    } else if info.get("_type").and_then(Value::as_str) == Some("playlist") {
        return Err(failed("playlists are not supported"));
    }
    Ok(info)
}

/// Metadata first (duration and Twitch status), then the format fallback loop.
/// Returns the source title.
async fn download_with_ytdlp(job: &YtdlpJob<'_>) -> Result<String> {
    let info = fetch_info(job).await?;
    let duration = finite_number(info.get("duration"), 0.0);
    if duration > job.max_duration {
        return Err(too_long());
    }
    let title = info
        .get("title")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("Unknown")
        .to_string();

    let selectors: &[&str] = if job.twitch { &TWITCH_FORMAT_SELECTORS } else { &YOUTUBE_FORMAT_SELECTORS };
    let output_name = job.output.file_name().and_then(|n| n.to_str()).unwrap_or(OUTPUT_FILENAME);
    let mut last_failure = None;
    let mut succeeded = false;
    for (idx, selector) in selectors.iter().enumerate() {
        if Instant::now() > job.deadline {
            return Err(deadline_exceeded());
        }
        tracing::info!(attempt = idx + 1, of = selectors.len(), "yt-dlp format attempt");
        remove_partial_files(job.output);
        let args = download_args(job.url, job.twitch, selector, job.ffmpeg, output_name, job.max_duration);
        let mut monitor = DownloadMonitor::new(job.work_dir, job.output, job.progress);
        // Size, disk, deadline and cancel guards abort immediately (no retry).
        let run = run_ytdlp(job.ytdlp, &args, job.work_dir, job.deadline, job.cancel, Some(&mut monitor)).await?;
        if run.status.success() {
            succeeded = true;
            break;
        }
        if job.twitch && run.status.code() == Some(101) {
            return Err(twitch_not_completed());
        }
        let failure = classify_ytdlp_failure(&run.stderr_tail);
        match failure {
            // A full disk, bot check or network failure affects every format selector.
            YtdlpFailure::DiskFull | YtdlpFailure::Blocked | YtdlpFailure::Network(_) => {
                return Err(ytdlp_error(&failure, job.twitch));
            }
            // Missing format, merge or conversion failure: try the next selector.
            YtdlpFailure::Format | YtdlpFailure::Other(_) => {
                tracing::warn!(attempt = idx + 1, "yt-dlp format attempt failed");
                last_failure = Some(failure);
            }
        }
    }
    if !succeeded {
        let failure = last_failure.unwrap_or(YtdlpFailure::Other(None));
        return Err(ytdlp_error(&failure, job.twitch));
    }
    if !locate_output(job.output) {
        // Exit 0 without a file: a match filter (Twitch duration) or --max-filesize skipped it.
        return Err(if job.twitch { twitch_duration() } else { failed("output file not found") });
    }
    Ok(title)
}

/// Live checks while yt-dlp runs: reported and stored bytes, free space, progress.
struct DownloadMonitor<'a> {
    work_dir: &'a Path,
    output: &'a Path,
    progress: &'a ProgressFn,
    downloaded: u64,
    total: u64,
    last_percent: Option<u64>,
}

impl<'a> DownloadMonitor<'a> {
    fn new(work_dir: &'a Path, output: &'a Path, progress: &'a ProgressFn) -> Self {
        Self { work_dir, output, progress, downloaded: 0, total: 0, last_percent: None }
    }

    /// `clipward-progress <downloaded> <total> <estimate>`; unknown fields print "NA".
    fn on_progress_line(&mut self, fields: &str) -> Result<()> {
        let mut parts = fields.split_whitespace().map(|p| {
            p.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v as u64)
        });
        let downloaded = parts.next().flatten().unwrap_or(0);
        let total = parts.next().flatten();
        let estimate = parts.next().flatten();
        let total = total.or(estimate).unwrap_or(0);
        check_source_size(downloaded)?;
        check_source_size(total)?;
        self.downloaded = downloaded;
        self.total = total;
        if total > 0 {
            let percent = (downloaded.min(total) * 100) / total;
            if self.last_percent != Some(percent) {
                self.last_percent = Some(percent);
                report(self.progress, percent as f64 / 100.0 * 0.9, "Downloading video...");
            }
        }
        Ok(())
    }

    /// yt-dlp fetches video and audio separately, so each stream can be under the
    /// limit while their combined files exceed it.
    fn poll(&mut self) -> Result<()> {
        check_source_size(stored_bytes(self.output))?;
        check_free_space(self.work_dir, self.total.saturating_sub(self.downloaded))
    }
}

struct YtdlpRun {
    status: ExitStatus,
    stdout: Vec<u8>,
    /// Last few KB of non-progress stderr, for classification only (never shown).
    stderr_tail: String,
}

#[derive(Default)]
struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &b in chunk {
            if b == b'\n' || b == b'\r' {
                if !self.pending.is_empty() {
                    lines.push(String::from_utf8_lossy(&self.pending).into_owned());
                    self.pending.clear();
                }
            } else {
                self.pending.push(b);
                if self.pending.len() >= MAX_STDERR_LINE {
                    lines.push(String::from_utf8_lossy(&self.pending).into_owned());
                    self.pending.clear();
                }
            }
        }
        lines
    }

    fn finish(&mut self) -> Option<String> {
        (!self.pending.is_empty()).then(|| String::from_utf8_lossy(&std::mem::take(&mut self.pending)).into_owned())
    }
}

fn handle_stderr_line(line: &str, monitor: &mut Option<&mut DownloadMonitor<'_>>, tail: &mut String) -> Result<()> {
    if let Some(fields) = line.strip_prefix(PROGRESS_MARKER) {
        if let Some(m) = monitor.as_deref_mut() {
            m.on_progress_line(fields)?;
        }
        return Ok(());
    }
    tail.push_str(line);
    tail.push('\n');
    if tail.len() > MAX_STDERR_TAIL {
        let mut cut = tail.len() - MAX_STDERR_TAIL;
        while !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail.drain(..cut);
    }
    Ok(())
}

/// Kills yt-dlp and, on Unix, the FFmpeg children it started (its own process group).
async fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id().and_then(|pid| libc::pid_t::try_from(pid).ok()) {
        // SAFETY: plain kill(2) on the process group the child leads (process_group(0)
        // at spawn). The child is not reaped yet, so the id cannot have been reused.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
}

enum RunEnd {
    Status(std::io::Result<ExitStatus>),
    Deadline,
    Cancelled,
}

/// Runs yt-dlp with a minimal environment in `cwd`, streaming stderr through the
/// monitor. Deadline, cancellation and monitor errors kill the process tree.
async fn run_ytdlp(
    program: &Path,
    args: &[String],
    cwd: &Path,
    deadline: Instant,
    cancel: &CancellationToken,
    mut monitor: Option<&mut DownloadMonitor<'_>>,
) -> Result<YtdlpRun> {
    // Minimal environment (no provider keys, no *_PROXY), null stdin, kill-on-drop.
    let mut cmd = media::media_command(&tool_path(program));
    cmd.args(args).current_dir(cwd).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            failed("yt-dlp is not installed")
        } else {
            failed("yt-dlp could not be started")
        }
    })?;
    let (Some(mut out), Some(mut err)) = (child.stdout.take(), child.stderr.take()) else {
        kill_tree(&mut child).await;
        return Err(failed("yt-dlp could not be started"));
    };

    let mut stdout = Vec::new();
    let mut tail = String::new();
    let mut splitter = LineSplitter::default();
    let mut out_buf = vec![0u8; 16 * 1024];
    let mut err_buf = vec![0u8; 16 * 1024];
    let (mut out_open, mut err_open) = (true, true);
    let mut ticker = tokio::time::interval(POLL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let streamed: Result<()> = loop {
        if !out_open && !err_open {
            break Ok(());
        }
        tokio::select! {
            r = out.read(&mut out_buf), if out_open => match r {
                Ok(0) | Err(_) => out_open = false,
                Ok(n) => {
                    if stdout.len() + n > MAX_YTDLP_STDOUT {
                        break Err(failed("video info exceeded the size limit"));
                    }
                    stdout.extend_from_slice(&out_buf[..n]);
                }
            },
            r = err.read(&mut err_buf), if err_open => {
                let lines = match r {
                    Ok(0) | Err(_) => {
                        err_open = false;
                        splitter.finish().into_iter().collect()
                    }
                    Ok(n) => splitter.push(&err_buf[..n]),
                };
                if let Err(e) = lines.iter().try_for_each(|l| handle_stderr_line(l, &mut monitor, &mut tail)) {
                    break Err(e);
                }
            },
            _ = ticker.tick() => {
                if let Some(m) = monitor.as_deref_mut() {
                    if let Err(e) = m.poll() {
                        break Err(e);
                    }
                }
            },
            _ = tokio::time::sleep_until(deadline) => break Err(deadline_exceeded()),
            _ = cancel.cancelled() => break Err(EngineError::Cancelled),
        }
    };
    if let Err(e) = streamed {
        kill_tree(&mut child).await;
        return Err(e);
    }
    let end = tokio::select! {
        s = child.wait() => RunEnd::Status(s),
        _ = tokio::time::sleep_until(deadline) => RunEnd::Deadline,
        _ = cancel.cancelled() => RunEnd::Cancelled,
    };
    let status = match end {
        RunEnd::Status(Ok(status)) => status,
        RunEnd::Status(Err(_)) => return Err(failed("yt-dlp could not be started")),
        RunEnd::Deadline => {
            kill_tree(&mut child).await;
            return Err(deadline_exceeded());
        }
        RunEnd::Cancelled => {
            kill_tree(&mut child).await;
            return Err(EngineError::Cancelled);
        }
    };
    if let Some(m) = monitor {
        // Catch a merge that grew past the limit between polls.
        m.poll()?;
    }
    Ok(YtdlpRun { status, stdout, stderr_tail: tail })
}

// ---------------------------------------------------------------------------
// Direct http(s) URLs

/// Streams a direct link to `output`. No proxy, no automatic redirects: each hop is
/// resolved, checked public and pinned to the checked address. Returns a title taken
/// from the final URL's file name.
async fn download_direct(
    url: &str,
    output: &Path,
    work_dir: &Path,
    deadline: Instant,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> Result<String> {
    let mut current = url.to_string();
    let mut redirects = 0;
    loop {
        let dest = net::resolve_public_destination(&current).await.map_err(net_error)?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&dest.host, dest.addr)
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(300))
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|_| failed("http client could not be created"))?;
        let response = tokio::select! {
            r = client.get(dest.url.clone()).send() => r.map_err(http_error)?,
            _ = tokio::time::sleep_until(deadline) => return Err(deadline_exceeded()),
            _ = cancel.cancelled() => return Err(EngineError::Cancelled),
        };
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .filter(|v| !v.is_empty());
            let next = match location {
                Some(loc) if redirects < MAX_REDIRECTS => dest.url.join(loc).ok(),
                _ => None,
            };
            current = next.ok_or_else(|| failed("redirect could not be followed"))?.to_string();
            redirects += 1;
            continue;
        }
        if !response.status().is_success() {
            return Err(failed(&format!("http error {status}")));
        }
        stream_to_file(response, output, work_dir, deadline, progress, cancel).await?;
        return Ok(url_title(&dest.url));
    }
}

async fn stream_to_file(
    mut response: reqwest::Response,
    output: &Path,
    work_dir: &Path,
    deadline: Instant,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> Result<()> {
    let total = response.content_length();
    if let Some(total) = total {
        check_source_size(total)?;
    }
    check_free_space(work_dir, total.unwrap_or(0))?;
    let mut file = tokio::fs::File::create(output).await.map_err(io_error)?;
    let mut downloaded: u64 = 0;
    let mut since_space_check: u64 = 0;
    let mut last_percent = None;
    loop {
        let chunk = tokio::select! {
            c = response.chunk() => c.map_err(http_error)?,
            _ = tokio::time::sleep_until(deadline) => return Err(deadline_exceeded()),
            _ = cancel.cancelled() => return Err(EngineError::Cancelled),
        };
        let Some(chunk) = chunk else { break };
        downloaded += chunk.len() as u64;
        check_source_size(downloaded)?;
        file.write_all(&chunk).await.map_err(io_error)?;
        since_space_check += chunk.len() as u64;
        if since_space_check >= FREE_SPACE_CHECK_BYTES {
            since_space_check = 0;
            check_free_space(work_dir, total.unwrap_or(0).saturating_sub(downloaded))?;
        }
        if let Some(total) = total.filter(|t| *t > 0) {
            let percent = downloaded.min(total) * 100 / total;
            if last_percent != Some(percent) {
                last_percent = Some(percent);
                report(progress, percent as f64 / 100.0 * 0.9, "Downloading video...");
            }
        }
    }
    file.flush().await.map_err(io_error)?;
    Ok(())
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    const URL: &str = "https://www.twitch.tv/videos/12345";

    fn message(e: EngineError) -> String {
        e.to_string()
    }

    // --- Twitch URL validation (test_twitch_downloader.py) ---

    #[test]
    fn download_twitch_canonical_vod_hosts() {
        for host in TWITCH_HOSTS {
            let url = format!("https://{host}/videos/12345/?t=30s&tracking=secret#x");
            assert_eq!(detect_source(&url).unwrap(), Source::Twitch(URL.into()), "{host}");
            assert_eq!(twitch_vod_url(&url).unwrap().as_deref(), Some(URL));
        }
    }

    #[test]
    fn download_other_twitch_pages_are_unsupported() {
        for url in [
            "https://twitch.tv/channel",
            "https://twitch.tv/collections/abc",
            "https://clips.twitch.tv/Clip",
            "https://player.twitch.tv/?video=12345",
            "https://twitch.tv/videos/nope",
            "https://www.twitch.tv./videos/12345",
            "https://user:pass@twitch.tv/videos/12345",
            "https://twitch.tv:8443/videos/12345",
            "https://twitch.tv:bad/videos/12345",
            "http://twitch.tv:443/videos/12345",
        ] {
            let err = detect_source(url).unwrap_err();
            assert_eq!(message(err), "unsupported twitch source", "{url}");
        }
    }

    #[test]
    fn download_exact_twitch_host_matching() {
        for url in ["https://twitch.tv.evil.test/videos/12345", "https://eviltwitch.tv/videos/12345", "https://example.com/video.mp4"] {
            assert_eq!(detect_source(url).unwrap(), Source::Direct(url.into()), "{url}");
        }
    }

    #[test]
    fn download_default_ports_match_browser_normalization() {
        assert_eq!(twitch_vod_url("https://twitch.tv:443/videos/12345").unwrap().as_deref(), Some(URL));
        assert_eq!(twitch_vod_url("http://twitch.tv:80/videos/12345").unwrap().as_deref(), Some(URL));
    }

    #[test]
    fn download_detects_youtube_local_and_file_urls() {
        for url in ["https://www.youtube.com/watch?v=abc", "https://youtu.be/abc", "https://music.youtube.com/watch?v=x"] {
            assert_eq!(detect_source(url).unwrap(), Source::YouTube(url.into()));
        }
        assert_eq!(detect_source("https://notyoutube.com/v").unwrap(), Source::Direct("https://notyoutube.com/v".into()));
        assert_eq!(detect_source("/videos/My Clip.mp4").unwrap(), Source::Local("/videos/My Clip.mp4".into()));
        #[cfg(unix)]
        assert_eq!(detect_source("file:///videos/My%20Clip.mp4").unwrap(), Source::Local("/videos/My Clip.mp4".into()));
    }

    // --- Twitch metadata validation ---

    #[test]
    fn download_twitch_metadata_rejection() {
        let not_completed = [
            json!({"is_live": true, "duration": 10}),
            json!({"live_status": "is_upcoming", "duration": 10}),
            json!({"live_status": "post_live", "duration": 10}),
            json!({"live_status": "processing", "duration": 10}),
        ];
        for info in not_completed {
            assert_eq!(message(validate_twitch_info(&info, 21600.0).unwrap_err()), "twitch vod is not completed");
        }
        for duration in [Value::Null, json!(0), json!(-1), json!("nan"), json!("inf"), json!(21601)] {
            let info = json!({ "duration": duration });
            assert_eq!(
                message(validate_twitch_info(&info, 21600.0).unwrap_err()),
                "twitch vod duration is invalid or too long"
            );
        }
        assert!(validate_twitch_info(&json!({"duration": 60, "was_live": true, "is_live": null}), 21600.0).is_ok());
    }

    #[test]
    fn download_missing_twitch_metadata_is_unavailable() {
        assert_eq!(message(validate_twitch_info(&Value::Null, 21600.0).unwrap_err()), "twitch vod unavailable");
    }

    // --- Limits ---

    #[test]
    fn download_duration_limits() {
        assert_eq!(duration_limit(None), MAX_DOWNLOAD_DURATION_SECONDS);
        assert_eq!(duration_limit(Some(0.0)), MAX_DOWNLOAD_DURATION_SECONDS);
        assert_eq!(duration_limit(Some(f64::NAN)), MAX_DOWNLOAD_DURATION_SECONDS);
        assert_eq!(duration_limit(Some(99999.0)), MAX_DOWNLOAD_DURATION_SECONDS);
        assert_eq!(duration_limit(Some(60.0)), 60.0);
        assert!(check_duration(60.0, 60.0).is_ok());
        assert!(message(check_duration(61.0, 60.0).unwrap_err()).contains("exceeds maximum allowed duration"));
        for bad in [0.0, -1.0, f64::NAN] {
            assert!(message(check_duration(bad, 60.0).unwrap_err()).starts_with("video download failed"));
        }
    }

    #[test]
    fn download_stops_before_filling_the_disk() {
        let gb = 1000 * 1000 * 1000;
        assert!(free_space_ok(3 * gb, gb));
        assert!(!free_space_ok(3 * gb, 2 * gb + gb / 2));
        assert!(!free_space_ok(MIN_FREE_BYTES - 1, 0));
        assert!(check_source_size(MAX_SOURCE_BYTES).is_ok());
        assert!(message(check_source_size(MAX_SOURCE_BYTES + 1).unwrap_err()).contains("20 gb"));
    }

    // --- Probe metadata (test_media_process_security.py) ---

    #[test]
    fn download_extreme_dimensions_rejected() {
        for (w, h) in [(0, 100), (100, 0), (1, 16000), (20000, 1), (16384, 16384)] {
            assert!(media::validate_video_dimensions(w, h).is_err(), "{w}x{h}");
        }
        for (w, h) in [(3840, 2160), (2160, 3840), (7680, 4320), (128, 4096)] {
            assert!(media::validate_video_dimensions(w, h).is_ok(), "{w}x{h}");
        }
    }

    #[test]
    fn download_metadata_rejects_extreme_geometry() {
        let info = json!({"streams": [{"codec_type": "video", "width": 1, "height": 16000}], "format": {"duration": "10"}});
        assert_eq!(message(metadata_from_probe(&info, "t".into()).unwrap_err()), "video download failed: invalid video metadata");
        let audio_only = json!({"streams": [{"codec_type": "audio"}], "format": {"duration": "10"}});
        assert!(message(metadata_from_probe(&audio_only, "t".into()).unwrap_err()).contains("no video stream"));
        let no_duration = json!({"streams": [{"codec_type": "video", "width": 640, "height": 360}], "format": {}});
        assert!(metadata_from_probe(&no_duration, "t".into()).is_err());
    }

    #[test]
    fn download_metadata_from_probe() {
        let info = json!({
            "streams": [
                {"codec_type": "audio"},
                {"codec_type": "video", "width": 1920, "height": 1080, "r_frame_rate": "30000/1001"}
            ],
            "format": {"duration": "59.5", "filename": "/secret/path.mp4"}
        });
        let meta = metadata_from_probe(&info, "Clip".into()).unwrap();
        assert_eq!((meta.width, meta.height, meta.has_audio, meta.title.as_str()), (1920, 1080, true, "Clip"));
        assert!((meta.fps - 29.97).abs() < 0.01);
        assert_eq!(meta.duration_seconds, 59.5);
        assert_eq!(parse_fps(Some(&json!("0/0"))), 30.0);
        assert_eq!(parse_fps(Some(&json!("25"))), 25.0);
        assert_eq!(parse_fps(None), 30.0);
    }

    // --- yt-dlp options (test_local_downloader_security.py) ---

    #[test]
    fn download_youtube_prefers_highest_quality_without_av1() {
        assert_eq!(YOUTUBE_FORMAT_SORT, "hdr:SDR,res:2160,fps");
        for selector in YOUTUBE_FORMAT_SELECTORS.iter().chain(TWITCH_FORMAT_SELECTORS.iter()) {
            assert!(!selector.contains("height"));
            for alternative in selector.split('/') {
                for part in alternative.split('+') {
                    assert!(
                        part.contains("vcodec!^=av01") || part.contains("vcodec^=avc1") || part == "ba" || part.starts_with("ba["),
                        "{part}"
                    );
                }
            }
        }
    }

    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    #[test]
    fn download_ytdlp_args_disable_proxy_config_and_native_handlers() {
        let ffmpeg = Path::new("/opt/ff/ffmpeg");
        for twitch in [false, true] {
            let info = info_args(URL, twitch);
            let dl = download_args(URL, twitch, "b[vcodec!^=av01]", ffmpeg, "source.mp4", 21600.0);
            for args in [&info, &dl] {
                assert!(has_pair(args, "--proxy", ""));
                assert!(has_pair(args, "--downloader", "native"));
                for flag in ["--no-config", "--no-playlist", "--hls-prefer-native", "--no-exec"] {
                    assert!(args.iter().any(|a| a == flag), "{flag}");
                }
                assert_eq!(args[args.len() - 2..], ["--".to_string(), URL.to_string()]);
                assert_eq!(has_pair(args, "--use-extractors", "twitch:vod"), twitch);
                assert!(!args.iter().any(|a| a == "--download-sections" || a == "--no-check-certificates"));
            }
            assert!(info.iter().any(|a| a == "-J"));
            assert!(has_pair(&dl, "--merge-output-format", "mp4"));
            assert!(has_pair(&dl, "--ffmpeg-location", "/opt/ff/ffmpeg"));
            assert!(has_pair(&dl, "--max-filesize", "20000000000"));
            assert!(has_pair(&dl, "--format-sort", YOUTUBE_FORMAT_SORT));
            assert!(has_pair(&dl, "--retries", "10"));
            assert!(has_pair(&dl, "--output", "source.mp4"));
            assert_eq!(dl.iter().any(|a| a == "--abort-on-unavailable-fragments"), twitch);
            assert_eq!(has_pair(&dl, "--break-match-filters", TWITCH_LIVE_FILTER), twitch);
            assert_eq!(has_pair(&dl, "--match-filters", "duration>0 & duration<=21600"), twitch);
        }
        let escaped = download_args(URL, false, "b", Path::new("ffmpeg"), "a%(id)s.mp4", 60.0);
        assert!(has_pair(&escaped, "--output", "a%%(id)s.mp4"));
    }

    #[test]
    fn download_ytdlp_failures_are_sanitized() {
        let stderr = "ERROR: unable to download video data: HTTP Error 403: Forbidden https://secret.invalid/token";
        let failure = classify_ytdlp_failure(stderr);
        assert_eq!(failure, YtdlpFailure::Network("http error 403".into()));
        assert_eq!(message(ytdlp_error(&failure, false)), "video download failed: http error 403");
        assert_eq!(message(ytdlp_error(&failure, true)), "twitch vod unavailable");
        let disk = classify_ytdlp_failure("ERROR: [Errno 28] No space left on device: '/tmp/x'");
        assert_eq!(message(ytdlp_error(&disk, true)), "not enough disk space to save clips");
        let bot = classify_ytdlp_failure("ERROR: Sign in to confirm you're not a bot");
        assert!(message(ytdlp_error(&bot, false)).contains("blocking this request"));
        assert_eq!(classify_ytdlp_failure("ERROR: Requested format is not available"), YtdlpFailure::Format);
        assert_eq!(
            classify_ytdlp_failure("ERROR: [youtube] abc: Private video. Sign in if you've been granted access"),
            YtdlpFailure::Other(Some("private video".into()))
        );
        assert_eq!(message(ytdlp_error(&classify_ytdlp_failure("boom /home/me"), false)), "video download failed");
    }


    #[test]
    fn download_line_splitter_and_progress() {
        let mut splitter = LineSplitter::default();
        assert_eq!(splitter.push(b"a\nclipward-pro"), ["a"]);
        assert_eq!(splitter.push(b"gress 5 10 NA\r\n"), ["clipward-progress 5 10 NA"]);
        assert_eq!(splitter.finish(), None);

        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join(OUTPUT_FILENAME);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let progress: ProgressFn = Arc::new(move |p: Progress| sink.lock().unwrap().push(p.percent));
        let mut monitor = DownloadMonitor::new(dir.path(), &output, &progress);
        monitor.on_progress_line("50 100 NA").unwrap();
        monitor.on_progress_line("50 NA 100.5").unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert!(monitor.on_progress_line(&format!("{} NA NA", MAX_SOURCE_BYTES + 1)).is_err());
        assert!(monitor.on_progress_line(&format!("1 NA {}", MAX_SOURCE_BYTES + 1)).is_err());
    }

    #[test]
    fn download_partial_files_are_found_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join(OUTPUT_FILENAME);
        let partials = ["source.mp4", "source.mp4.part", "source.mp4.ytdl", "source.f137.mp4.part", "source.temp.mp4"];
        for name in partials.iter().chain(["keep.mp4", "sources.txt"].iter()) {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        assert_eq!(download_paths(&output).len(), partials.len());
        assert_eq!(stored_bytes(&output), partials.len() as u64);
        remove_partial_files(&output);
        let mut left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name()).collect();
        left.sort();
        assert_eq!(left, ["keep.mp4", "sources.txt"]);
    }

    #[test]
    fn download_url_titles() {
        assert_eq!(url_title(&Url::parse("https://cdn.example/v/My%20Talk.mp4?sig=1").unwrap()), "My Talk");
        assert_eq!(url_title(&Url::parse("https://cdn.example/").unwrap()), "video");
    }

    #[tokio::test]
    async fn download_rejects_private_direct_urls_without_leaking_them() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path(), None);
        let progress: ProgressFn = Arc::new(|_| {});
        for url in ["http://127.0.0.1/secret-token.mp4", "http://localhost/secret-token.mp4", "http://10.0.0.1:8080/secret-token.mp4"] {
            let err = message(download(url, &cfg, None, &progress, &CancellationToken::new()).await.unwrap_err());
            assert!(err.starts_with("video download failed"), "{err}");
            assert!(!err.contains("secret") && !err.contains("127.0.0.1"), "{err}");
        }
    }

    #[tokio::test]
    async fn download_missing_local_file_does_not_leak_path() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path(), None);
        let progress: ProgressFn = Arc::new(|_| {});
        let err = message(download("/no/such/secret.mp4", &cfg, None, &progress, &CancellationToken::new()).await.unwrap_err());
        assert_eq!(err, "video download failed: local video not found");
    }

    fn test_config(root: &Path, ytdlp: Option<PathBuf>) -> EngineConfig {
        EngineConfig {
            ffmpeg: root.join("bin/ffmpeg"),
            ffprobe: root.join("bin/ffprobe"),
            ytdlp,
            assets_dir: root.join("assets"),
            whisper_model_path: root.join("model.bin"),
            ai: Default::default(),
            custom_vocabulary: Vec::new(),
            work_dir: root.join("work"),
            output_dir: root.join("out"),
        }
    }

    // --- Fake yt-dlp / ffprobe binaries (Unix shell scripts) ---

    #[cfg(unix)]
    mod fake_tools {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        pub struct Fixture {
            pub _dir: tempfile::TempDir,
            pub cfg: EngineConfig,
            pub bin: PathBuf,
        }

        fn script(path: &Path, body: &str) {
            std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        /// `download_body` runs in the work dir for non `-J` calls.
        pub fn fixture(info: &Value, download_body: &str) -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let bin = dir.path().join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("info.json"), info.to_string()).unwrap();
            std::fs::write(
                bin.join("probe.json"),
                json!({"streams": [{"codec_type": "video", "width": 1280, "height": 720, "r_frame_rate": "30/1"},
                                   {"codec_type": "audio"}],
                       "format": {"duration": "59"}})
                .to_string(),
            )
            .unwrap();
            std::fs::write(bin.join("download.sh"), download_body).unwrap();
            script(
                &bin.join("yt-dlp"),
                "d=$(dirname \"$0\")\nprintf '%s\\n' \"$*\" >> \"$d/args.log\"\nenv >> \"$d/env.log\"\n\
                 for a in \"$@\"; do if [ \"$a\" = \"-J\" ]; then cat \"$d/info.json\"; exit 0; fi; done\n\
                 . \"$d/download.sh\"\n",
            );
            script(&bin.join("ffprobe"), "cat \"$(dirname \"$0\")/probe.json\"\n");
            let cfg = test_config(dir.path(), Some(bin.join("yt-dlp")));
            Fixture { _dir: dir, cfg, bin }
        }

        pub fn saved_info() -> Value {
            json!({"title": "Saved stream", "duration": 60, "was_live": true, "is_live": null, "uploader": "Streamer"})
        }

        pub async fn run(fx: &Fixture, source: Source) -> Result<DownloadResult> {
            let progress: ProgressFn = Arc::new(|_| {});
            fetch_remote(&source, &fx.cfg, 21600.0, &progress, &CancellationToken::new()).await
        }

        pub fn work_files(fx: &Fixture) -> Vec<String> {
            std::fs::read_dir(&fx.cfg.work_dir)
                .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
                .unwrap_or_default()
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_twitch_uses_native_vod_and_actual_probe() {
        use fake_tools::*;
        let fx = fixture(
            &saved_info(),
            "printf partial > source.mp4.part\necho 'clipward-progress 7 NA NA' >&2\nprintf media > source.mp4\nrm -f source.mp4.part\n",
        );
        std::env::set_var("OPENROUTER_API_KEY", "dummy-test-secret");
        std::env::set_var("HTTPS_PROXY", "http://proxy.invalid");
        let result = run(&fx, Source::Twitch(URL.into())).await.unwrap();
        assert!(!result.is_local);
        assert_eq!(result.path, fx.cfg.work_dir.join("source.mp4"));
        let m = &result.metadata;
        assert_eq!((m.duration_seconds, m.width, m.height, m.title.as_str()), (59.0, 1280, 720, "Saved stream"));
        let log = std::fs::read_to_string(fx.bin.join("args.log")).unwrap();
        let calls: Vec<&str> = log.lines().collect();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].contains("-J"));
        for call in &calls {
            assert!(call.contains("--use-extractors twitch:vod") && call.contains("--downloader native"));
            assert!(call.ends_with(&format!("-- {URL}")));
        }
        assert!(calls[1].contains("--format b[vcodec!^=av01]") && calls[1].contains("--abort-on-unavailable-fragments"));
        let env = std::fs::read_to_string(fx.bin.join("env.log")).unwrap();
        assert!(!env.contains("OPENROUTER_API_KEY") && !env.contains("proxy.invalid"));
        assert_eq!(work_files(&fx), ["source.mp4"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_live_status_is_rechecked_at_download() {
        use fake_tools::*;
        let fx = fixture(&saved_info(), "printf partial > source.mp4.part\nexit 101\n");
        let err = run(&fx, Source::Twitch(URL.into())).await.unwrap_err();
        assert_eq!(message(err), "twitch vod is not completed");
        assert!(work_files(&fx).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_live_twitch_metadata_is_rejected_before_download() {
        use fake_tools::*;
        let fx = fixture(&json!({"is_live": true, "duration": 60}), "exit 1\n");
        let err = run(&fx, Source::Twitch(URL.into())).await.unwrap_err();
        assert_eq!(message(err), "twitch vod is not completed");
        assert_eq!(std::fs::read_to_string(fx.bin.join("args.log")).unwrap().lines().count(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_oversize_downloads_remove_partials() {
        use fake_tools::*;
        for line in [format!("{} NA NA", MAX_SOURCE_BYTES + 1), format!("1 NA {}", MAX_SOURCE_BYTES + 1)] {
            let fx = fixture(
                &saved_info(),
                &format!("printf partial > source.mp4.part\necho 'clipward-progress {line}' >&2\nsleep 5\nprintf media > source.mp4\n"),
            );
            let started = std::time::Instant::now();
            let err = run(&fx, Source::Twitch(URL.into())).await.unwrap_err();
            assert!(message(err).contains("20 gb"));
            assert!(started.elapsed() < Duration::from_secs(4), "child was not killed");
            assert!(work_files(&fx).is_empty());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_provider_details_are_sanitized_and_partial_removed() {
        use fake_tools::*;
        let body = "printf partial > source.mp4.part\necho 'ERROR: HTTP Error 403 https://secret.invalid/token' >&2\nexit 1\n";
        let fx = fixture(&saved_info(), body);
        assert_eq!(message(run(&fx, Source::Twitch(URL.into())).await.unwrap_err()), "twitch vod unavailable");
        assert!(work_files(&fx).is_empty());

        let fx = fixture(&json!({"title": "t", "duration": 60}), body);
        let yt = "https://www.youtube.com/watch?v=abc";
        let err = message(run(&fx, Source::YouTube(yt.into())).await.unwrap_err());
        assert_eq!(err, "video download failed: http error 403");
        // Network failures are not retried with the next format selector.
        assert_eq!(std::fs::read_to_string(fx.bin.join("args.log")).unwrap().lines().count(), 2);
        assert!(work_files(&fx).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_format_failures_fall_back_to_next_selector() {
        use fake_tools::*;
        let body = "case \"$*\" in *avc1*) printf media > source.mp4; exit 0;; esac\n\
                    echo 'ERROR: Requested format is not available' >&2\nexit 1\n";
        let fx = fixture(&json!({"title": "t", "duration": 60}), body);
        let result = run(&fx, Source::YouTube("https://youtu.be/abc".into())).await.unwrap();
        assert_eq!(result.metadata.title, "t");
        assert_eq!(std::fs::read_to_string(fx.bin.join("args.log")).unwrap().lines().count(), 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_too_long_video_is_rejected_before_download() {
        use fake_tools::*;
        let fx = fixture(&json!({"title": "t", "duration": 99999}), "exit 1\n");
        let err = message(run(&fx, Source::YouTube("https://youtu.be/abc".into())).await.unwrap_err());
        assert!(err.contains("exceeds maximum allowed duration"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_cancellation_kills_child_and_cleans_up() {
        use fake_tools::*;
        let fx = fixture(&saved_info(), "printf partial > source.mp4.part\nsleep 30\n");
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            trigger.cancel();
        });
        let progress: ProgressFn = Arc::new(|_| {});
        let started = std::time::Instant::now();
        let err = fetch_remote(&Source::Twitch(URL.into()), &fx.cfg, 21600.0, &progress, &cancel).await.unwrap_err();
        assert!(matches!(err, EngineError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(work_files(&fx).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_missing_ytdlp_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let progress: ProgressFn = Arc::new(|_| {});
        let cfg = test_config(dir.path(), None);
        let err = fetch_remote(&Source::Twitch(URL.into()), &cfg, 60.0, &progress, &CancellationToken::new()).await;
        assert_eq!(message(err.unwrap_err()), "video download failed: yt-dlp is not installed");
        let cfg = test_config(dir.path(), Some(dir.path().join("missing/yt-dlp")));
        let err = fetch_remote(&Source::Twitch(URL.into()), &cfg, 60.0, &progress, &CancellationToken::new()).await;
        assert_eq!(message(err.unwrap_err()), "video download failed: yt-dlp is not installed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn download_local_file_is_used_in_place() {
        use fake_tools::*;
        let fx = fixture(&saved_info(), "");
        let video = fx.bin.join("My Talk.mp4");
        std::fs::write(&video, b"media").unwrap();
        let progress: ProgressFn = Arc::new(|_| {});
        let url = Url::from_file_path(&video).unwrap().to_string();
        for input in [video.to_string_lossy().into_owned(), url] {
            let result = download(&input, &fx.cfg, None, &progress, &CancellationToken::new()).await.unwrap();
            assert!(result.is_local);
            assert_eq!(result.path, video);
            assert_eq!(result.metadata.title, "My Talk");
        }
        let err = download(&video.to_string_lossy(), &fx.cfg, Some(10.0), &progress, &CancellationToken::new()).await;
        assert!(message(err.unwrap_err()).contains("exceeds maximum allowed duration"));
        assert!(video.exists());
    }
}
