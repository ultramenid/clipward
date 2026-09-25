//! Types shared across engine modules. Mirrors the Python dataclasses in
//! engine/clip_engine (the pre-Rust implementation, kept in git history).
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptWord {
    pub word: String,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TranscriptSegment {
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    pub text: String,
    pub speaker_label: Option<String>,
    pub words: Vec<TranscriptWord>,
    /// Non-speech events inside this segment, e.g. "(laughter)". Planner signal only, never captioned.
    pub audio_events: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Transcript {
    pub segments: Vec<TranscriptSegment>,
    pub full_text: String,
    pub language: Option<String>,
    pub duration_seconds: Option<f64>,
    /// e.g. "whisper.cpp:large-v3-turbo-q5_0"
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClipPlanSegment {
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    pub virality_score: f64,
    pub layout_type: String,
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub emphasis_words: Vec<String>,
    pub render_fallback: Option<String>,
    /// Longform only (source ms).
    pub skip_ranges_ms: Vec<(i64, i64)>,
    pub chapters: Vec<(i64, String)>,
    pub description: Option<String>,
    /// Filled in after rendering.
    pub output_chapters: Vec<(i64, String)>,
    pub subtitle_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlanningCosts {
    pub provider: String,
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub estimated_cost_usd: f64,
    pub attempts: u32,
    pub cost_incomplete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipPlan {
    pub clips: Vec<ClipPlanSegment>,
    pub costs: PlanningCosts,
    /// "transcript" | "visual"
    pub planning_source: String,
    /// The planner's content analysis (plan.json "insights").
    #[serde(default)]
    pub insights: Option<String>,
    /// Frames the visual-only planner saw (JobOutput metrics.visual_frame_count).
    #[serde(default)]
    pub visual_frame_count: u32,
}

/// Where AI planning requests go. `provider` is 'openrouter' | 'anthropic' | 'openai' |
/// 'local' (the user's own OpenAI-compatible server) | 'cli:<id>'.
#[derive(Debug, Clone, Default)]
pub struct AiSettings {
    pub provider: String,
    pub model: String,
    pub openrouter_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    /// Absolute path to the CLI binary when provider is `cli:<id>`.
    pub cli_path: Option<PathBuf>,
    /// OpenAI-compatible base URL (e.g. `http://localhost:11434/v1`) when provider
    /// is `local`; None uses [`crate::llm::DEFAULT_LOCAL_BASE_URL`].
    pub base_url: Option<String>,
    /// Whether the local model accepts images (frames for planning and layout vision).
    pub local_images: bool,
    /// Optional bearer key for a local server that requires one.
    pub local_api_key: Option<String>,
}

/// Paths and settings the app shell hands the engine for every job.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    /// yt-dlp binary for link sources; None when not installed.
    pub ytdlp: Option<PathBuf>,
    /// Directory holding fonts/ and models/ (crates/engine/assets in the repo).
    pub assets_dir: PathBuf,
    /// Downloaded ggml model file.
    pub whisper_model_path: PathBuf,
    pub ai: AiSettings,
    pub custom_vocabulary: Vec<String>,
    /// Scratch space for this job; removed by the caller afterwards.
    pub work_dir: PathBuf,
    /// Run folder where clips, transcript.json, plan.json and result.json land.
    pub output_dir: PathBuf,
}

/// One clipping run, as the Create wizard submits it (src/shared/jobs.ts ClipJobRequest).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipJobRequest {
    pub video_url: String,
    #[serde(default)]
    pub requirements: Option<String>,
    pub max_clips: Option<u32>,
    pub auto_clip_count: bool,
    pub duration_ranges: Option<Vec<String>>,
    pub aspect_ratio: String,
    pub layout_style: String,
    pub layout_vision: bool,
    pub pacing: String,
    #[serde(default = "one")]
    pub video_speed: f64,
    pub include_captions: bool,
    pub caption_preset: String,
    pub start_time_seconds: Option<f64>,
    pub end_time_seconds: Option<f64>,
    pub banner_platform: Option<String>,
    pub banner_channel_url: Option<String>,
    /// Manual clip mode: the user's own moments, cut exactly as typed with no AI planner.
    /// None (the default, and every older payload) means the AI picks the moments.
    #[serde(default)]
    pub manual_clips: Option<Vec<ManualClip>>,
}

/// One moment the user typed in manual clip mode (source seconds).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManualClip {
    pub start_seconds: f64,
    pub end_seconds: f64,
    #[serde(default)]
    pub title: Option<String>,
}

fn one() -> f64 {
    1.0
}

/// Stage labels match JobSnapshot.status in src/shared/jobs.ts.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Pending,
    Downloading,
    Transcribing,
    Planning,
    Rendering,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub stage: Stage,
    pub percent: f64,
    pub step: String,
    pub clips_done: u32,
    pub clips_total: u32,
}

/// Sink for progress updates. Engine code calls it; the app shell forwards to the UI.
pub type ProgressFn = std::sync::Arc<dyn Fn(Progress) + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoMetadata {
    pub title: String,
    pub duration_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub has_audio: bool,
}

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub path: PathBuf,
    pub metadata: VideoMetadata,
    /// True when the file is the user's own local file (never delete it).
    pub is_local: bool,
}
