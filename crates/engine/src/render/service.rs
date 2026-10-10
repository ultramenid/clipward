//! Rendering service: FFmpeg rendering with smart framing, pacing and captions.
//! Port of rendering_service.py.
//!
//! render_clip pipeline:
//!   1. Layout plan (9:16): per-shot framing from the layout analyzer.
//!   2. Pacing: keep intervals from the clip editor (tight pacing cuts dead air/fillers).
//!   3. One filter graph: video and audio edited on the same presentation clock.
//!   4. Captions and title/banner overlays remapped onto the edited timeline.
use super::captions::{self, CaptionRequest, CaptionStyle};
use super::clip_editor::{
    compute_keep_intervals, reaction_intervals, remap_plan, remap_segments, subtract_intervals, window_words, TimeMap,
    TIGHT,
};
use super::fonts::{self, Canvas, TextFont};
use super::layout_renderer::{
    banner_y, build_layout_graph, caption_anchor, even, face_zones, measured_loudness_filter, per_shot_expr, title_y,
    CaptionPlacer, GraphOptions, AUDIO_FORMAT, AUDIO_SYNC,
};
use super::process::{MediaOutput, MediaRunner, SystemRunner, MEDIA_INPUT_OPTIONS, MEDIA_TIMEOUT, PROBE_TIMEOUT};
use super::pyfmt::{self, Rational};
use super::video_speed::{scaled_duration_ms, speed_video_filter, validate_video_speed};
use crate::error::{EngineError, Result};
use crate::layout::types::{ClipLayoutPlan, ShotLayout, SCREEN};
use crate::layout::{LayoutRequest, VisionCache};
use crate::media::MAX_TOOL_OUTPUT_BYTES;
use crate::types::{ClipJobRequest, ClipPlanSegment, EngineConfig, LogoOverlay, TranscriptSegment};
use futures::future::BoxFuture;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Landscape title card: on screen for the opening seconds only.
pub const LANDSCAPE_TITLE_SHOW_S: (f64, f64) = (0.4, 5.5);
pub const LANDSCAPE_TITLE_FADE_S: f64 = 0.4;
/// Logo width limits (fraction of the frame width) and the gap to the frame edge.
pub const MIN_LOGO_SIZE: f64 = 0.05;
pub const MAX_LOGO_SIZE: f64 = 0.4;
pub const LOGO_MARGIN: f64 = 0.04;
/// Output frame rates a landscape render keeps from its source (higher is capped).
pub const MAX_OUTPUT_FPS: i64 = 60;
/// Leave room below Windows' process command-line limit; longer graphs go to a script file.
pub const MAX_INLINE_FILTER_GRAPH_BYTES: usize = 8192;
/// Audio padding before start and after end (settings.audio_padding_ms).
pub const AUDIO_PADDING_MS: i64 = 150;
/// libx264 settings when VideoToolbox isn't used.
pub const FFMPEG_PRESET: &str = "veryfast";
pub const FFMPEG_CRF: i64 = 20;
/// OpenH264 (no libx264 in the LGPL bundles): High profile, and a bitrate it never reaches so
/// frames sit at the QP floor. QP 20 matches libx264 CRF 20 (VMAF, PSNR) at ~2x the size;
/// its default rate control would go ~6x below that.
pub const OPENH264_ARGS: &[&str] = &["-c:v", "libopenh264", "-profile:v", "high", "-b:v", "50M", "-qmin", "20"];

pub const RENDER_FAILED: &str = "video render failed";
pub const DISK_FULL: &str = "not enough disk space to save clips";

pub const ASPECT_VERTICAL: &str = "9:16";
pub const ASPECT_HORIZONTAL: &str = "16:9";
pub const LAYOUT_STYLE_AUTO: &str = "auto";
pub const LAYOUT_STYLE_FILL: &str = "fill";
pub const LAYOUT_STYLE_FIT: &str = "fit";

/// A render failure carrying the marker phrase the UI maps to a fixed message
/// ("video render failed", or "not enough disk space to save clips").
pub fn render_error(detail: impl AsRef<str>) -> EngineError {
    let detail = detail.as_ref();
    let lower = detail.to_lowercase();
    if lower.contains("no space left on device") || lower.contains("disk quota exceeded") || lower.contains(DISK_FULL) {
        return EngineError::msg(DISK_FULL);
    }
    if lower.starts_with(RENDER_FAILED) {
        return EngineError::msg(detail);
    }
    EngineError::msg(format!("{RENDER_FAILED}: {detail}"))
}

/// io::Error -> render error, recognizing a full disk.
pub fn io_render_error(e: std::io::Error) -> EngineError {
    let disk_full = e.kind() == std::io::ErrorKind::StorageFull
        || e.kind() == std::io::ErrorKind::QuotaExceeded
        || matches!(e.raw_os_error(), Some(28) | Some(122) if cfg!(unix))
        || matches!(e.raw_os_error(), Some(39) | Some(112) if cfg!(windows));
    if disk_full {
        return EngineError::msg(DISK_FULL);
    }
    render_error(e.to_string())
}

fn map_render_err(e: EngineError) -> EngineError {
    match e {
        EngineError::Cancelled => EngineError::Cancelled,
        EngineError::Failed(m) => render_error(m),
    }
}

/// Output dimensions for an aspect ratio (config.get_output_dimensions).
pub fn get_output_dimensions(aspect_ratio: &str) -> (i64, i64) {
    if aspect_ratio == ASPECT_HORIZONTAL {
        (1920, 1080)
    } else {
        (1080, 1920)
    }
}

/// 16:9 output size that keeps the source's detail: 4K, 1440p or 1080p.
pub fn get_landscape_dimensions(source_width: i64, source_height: i64) -> (i64, i64) {
    if source_width <= 0 || source_height <= 0 {
        return (1920, 1080);
    }
    let fillable = (source_height as f64).min(source_width as f64 * 9.0 / 16.0);
    for height in [2160i64, 1440, 1080] {
        if fillable >= height as f64 * 0.98 {
            return (height * 16 / 9, height);
        }
    }
    (1920, 1080)
}

/// Parallel renders on the desktop: scale with cores. A render keeps ~3 busy, and its
/// serial filter chain and the next clip's layout analysis overlap the other's encode.
pub fn max_concurrent_renders() -> usize {
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    (cpus / 4).clamp(1, 4)
}

/// Request for rendering one clip (RenderRequest in the Python engine).
#[derive(Debug, Clone)]
pub struct RenderRequest {
    pub video_path: PathBuf,
    pub output_path: PathBuf,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    pub source_width: i64,
    pub source_height: i64,
    /// Word timings drive tight pacing as well as captions, so pass them even when captions are off.
    pub transcript_segments: Vec<TranscriptSegment>,
    pub include_captions: bool,
    pub caption_style: Option<CaptionStyle>,
    pub title_text: Option<String>,
    /// Planner-chosen punch words highlighted in the captions.
    pub emphasis_words: Vec<String>,
    pub banner_platform: Option<String>,
    pub banner_channel_url: Option<String>,
    /// Images stamped in the frame's corners.
    pub logos: Vec<LogoOverlay>,
    pub include_audio: bool,
    pub apply_padding: bool,
    /// "9:16" | "16:9"
    pub aspect_ratio: String,
    /// Framing for 9:16 output: auto | fill | fit.
    pub layout_style: String,
    /// Ask the vision model while planning the layout (the job's layout_vision toggle).
    pub layout_vision: bool,
    /// tight | natural
    pub pacing: String,
    pub video_speed: f64,
    /// Longform episode: gentler pacing, SRT sidecar, planner skips and chapters (source ms).
    pub longform: bool,
    pub skip_ranges_ms: Vec<(i64, i64)>,
    pub chapters: Vec<(i64, String)>,
}

impl Default for RenderRequest {
    fn default() -> Self {
        Self {
            video_path: PathBuf::new(),
            output_path: PathBuf::new(),
            start_time_ms: 0,
            end_time_ms: 0,
            source_width: 0,
            source_height: 0,
            transcript_segments: Vec::new(),
            include_captions: true,
            caption_style: None,
            title_text: None,
            emphasis_words: Vec::new(),
            banner_platform: None,
            banner_channel_url: None,
            logos: Vec::new(),
            include_audio: true,
            apply_padding: true,
            aspect_ratio: ASPECT_VERTICAL.into(),
            layout_style: LAYOUT_STYLE_AUTO.into(),
            layout_vision: true,
            pacing: TIGHT.into(),
            video_speed: 1.0,
            longform: false,
            skip_ranges_ms: Vec::new(),
            chapters: Vec::new(),
        }
    }
}

/// Transcript segments overlapping [start, end) (pipeline._filter_transcript_for_clip).
pub fn filter_transcript_for_clip(segments: &[TranscriptSegment], start_ms: i64, end_ms: i64) -> Vec<TranscriptSegment> {
    segments.iter().filter(|s| s.end_time_ms > start_ms && s.start_time_ms < end_ms).cloned().collect()
}

impl RenderRequest {
    /// The request the pipeline builds for one planned clip.
    #[allow(clippy::too_many_arguments)]
    pub fn for_segment(
        job: &ClipJobRequest,
        segment: &ClipPlanSegment,
        transcript: &[TranscriptSegment],
        captions_available: bool,
        longform: bool,
        video_path: &Path,
        output_path: &Path,
        source_width: u32,
        source_height: u32,
    ) -> Self {
        Self {
            video_path: video_path.to_path_buf(),
            output_path: output_path.to_path_buf(),
            start_time_ms: segment.start_time_ms,
            end_time_ms: segment.end_time_ms,
            source_width: source_width as i64,
            source_height: source_height as i64,
            transcript_segments: filter_transcript_for_clip(transcript, segment.start_time_ms, segment.end_time_ms),
            include_captions: job.include_captions && captions_available,
            caption_style: Some(captions::caption_style_for(&job.caption_preset)),
            title_text: segment.summary.clone(),
            emphasis_words: segment.emphasis_words.clone(),
            banner_platform: job.banner_platform.clone(),
            banner_channel_url: job.banner_channel_url.clone(),
            logos: job.logos.clone(),
            include_audio: true,
            apply_padding: true,
            aspect_ratio: job.aspect_ratio.clone(),
            layout_style: job.layout_style.clone(),
            layout_vision: job.layout_vision,
            pacing: job.pacing.clone(),
            video_speed: job.video_speed,
            longform,
            skip_ranges_ms: segment.skip_ranges_ms.clone(),
            chapters: segment.chapters.clone(),
        }
    }
}

/// Result of rendering one clip.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RenderResult {
    pub output_path: PathBuf,
    pub file_size_bytes: u64,
    /// Of the rendered file, after pacing cuts and speed.
    pub duration_ms: i64,
    /// Dead air / fillers cut by tight pacing (and planner skips).
    pub removed_ms: i64,
    pub cut_count: usize,
    /// Dominant smart layout, or "fit".
    pub layout_type: String,
    /// ShotLayout::summary() of each shot when smart framing rendered.
    pub layout_shots: Vec<serde_json::Value>,
    pub layout_cost_usd: f64,
    /// None (as planned), "letterbox" (framing dropped) or "letterbox_natural" (framing and pacing dropped).
    pub render_fallback: Option<String>,
    /// (output ms, title) on the edited timeline.
    pub chapters: Vec<(i64, String)>,
    /// SRT sidecar for longform clips, next to the clip.
    pub subtitle_path: Option<PathBuf>,
    pub output_width: i64,
    pub output_height: i64,
    /// Output frame rate as an FFmpeg rational ("30", or the source's for 16:9).
    pub fps: String,
    /// Wall-clock seconds in layout analysis, loudness measurement and the FFmpeg
    /// render (failed rungs included), for the metrics.
    pub analysis_seconds: f64,
    pub loudness_seconds: f64,
    pub encode_seconds: f64,
    /// What the successful rung rendered, for the clip's editable project.
    #[serde(skip)]
    pub edit: Option<RenderedEdit>,
}

/// The framing and cuts one clip was rendered with (window time).
#[derive(Debug, Clone, Default)]
pub struct RenderedEdit {
    pub window_start_ms: i64,
    pub plan: ClipLayoutPlan,
    pub keeps: Vec<(i64, i64)>,
}

impl RenderResult {
    /// Fill the plan segment's render outputs (pipeline behavior).
    pub fn apply_to(&self, segment: &mut ClipPlanSegment) {
        segment.layout_type = self.layout_type.clone();
        segment.render_fallback = self.render_fallback.clone();
        segment.output_chapters = self.chapters.clone();
        segment.subtitle_path = self.subtitle_path.as_ref().map(|p| p.to_string_lossy().into_owned());
    }

    /// The `clip_layouts[]` metrics entry saved in result.json.
    pub fn clip_layout_entry(&self, clip_index: usize, request: &RenderRequest) -> serde_json::Value {
        let framing_status = if self.render_fallback.is_some() {
            "fallback"
        } else if request.aspect_ratio == ASPECT_HORIZONTAL || request.layout_style == LAYOUT_STYLE_FIT {
            "classic"
        } else if self.layout_type == "fit" {
            "whole_frame_auto"
        } else {
            "smart"
        };
        serde_json::json!({
            "clip_index": clip_index,
            "layout_type": self.layout_type,
            "framing_status": framing_status,
            "shots": self.layout_shots,
            "pacing_removed_ms": self.removed_ms,
            "render_fallback": self.render_fallback,
            "analysis_seconds": round_ms(self.analysis_seconds),
            "loudness_seconds": round_ms(self.loudness_seconds),
            "encode_seconds": round_ms(self.encode_seconds),
        })
    }
}

fn round_ms(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

/// Plans a render window's layout. `Ok(None)` or an error means "use the letterbox".
pub type AnalyzeFn =
    Arc<dyn Fn(LayoutRequest, CancellationToken) -> BoxFuture<'static, Result<Option<ClipLayoutPlan>>> + Send + Sync>;

/// The layout analyzer (`layout::analyze_layout`) with one vision cache per job.
pub fn default_analyzer(cfg: &EngineConfig) -> AnalyzeFn {
    let cfg = Arc::new(cfg.clone());
    let cache = Arc::new(VisionCache::new());
    Arc::new(move |req: LayoutRequest, cancel: CancellationToken| {
        let (cfg, cache) = (cfg.clone(), cache.clone());
        Box::pin(async move { crate::layout::analyze_layout(&cfg, &req, Some(&cache), &cancel).await })
    })
}

/// An image composited over the video at (x_expr, y_expr): `filter` (a scale, alpha
/// fades) runs on the image first, and `enable` limits when it shows.
/// The file is deleted after the render, so it must be a per-clip copy.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Overlay {
    pub path: PathBuf,
    pub x: String,
    pub y: String,
    pub filter: Option<String>,
    pub enable: Option<String>,
}

/// One rung of the fallback ladder, handed to the edit renderer.
pub struct EditJob<'a> {
    pub request: &'a RenderRequest,
    pub plan: ClipLayoutPlan,
    pub time_map: TimeMap,
    pub window_start_ms: i64,
    pub window_ms: i64,
    pub width: i64,
    pub height: i64,
    pub landscape: bool,
    pub fps: String,
    pub loudness_filter: Option<String>,
}

/// Renders clips with FFmpeg. Cheap to share across concurrent renders.
pub struct Renderer {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    /// Bundled caption fonts (libass only sees fonts it is pointed at).
    pub fonts_dir: Option<PathBuf>,
    /// Title card / banner face.
    pub font_path: Option<PathBuf>,
    pub runner: Arc<dyn MediaRunner>,
    /// None: no smart framing (letterbox), and pacing treats shot content as unknown.
    pub analyzer: Option<AnalyzeFn>,
    /// Replaces the encoder arguments (tests; the app's lighter preview chunks).
    pub video_codec_override: Option<Vec<String>>,
    /// Encode with VideoToolbox (the desktop macOS build) instead of probing GPU encoders.
    pub videotoolbox: bool,
    /// Hardware encoder found by probing (without VideoToolbox); pre-fill with `Some(None)` to skip the probe.
    pub hw_encoder: tokio::sync::OnceCell<Option<HwEncoder>>,
    /// Whether FFmpeg has libx264 (GPL builds; the bundles are LGPL), probed once; pre-fill to skip the probe.
    pub x264: tokio::sync::OnceCell<bool>,
    pub audio_padding_ms: i64,
    /// Script-file directory for long filter graphs (system temp when None).
    pub temp_dir: Option<PathBuf>,
}

impl Renderer {
    /// Renderer for a job: the configured ffmpeg/ffprobe, bundled fonts and the layout analyzer.
    pub fn new(cfg: &EngineConfig) -> Self {
        let fonts_dir = Some(cfg.assets_dir.join("fonts")).filter(|p| p.is_dir());
        if fonts_dir.is_none() {
            tracing::warn!("Caption fonts directory not found; captions will use system fonts");
        }
        let font_path = fonts::overlay_font_path(fonts_dir.as_deref());
        Self {
            ffmpeg: cfg.ffmpeg.clone(),
            ffprobe: cfg.ffprobe.clone(),
            fonts_dir,
            font_path,
            runner: Arc::new(SystemRunner),
            analyzer: Some(default_analyzer(cfg)),
            video_codec_override: None,
            videotoolbox: cfg!(target_os = "macos"),
            hw_encoder: tokio::sync::OnceCell::new(),
            x264: tokio::sync::OnceCell::new(),
            audio_padding_ms: AUDIO_PADDING_MS,
            temp_dir: None,
        }
    }

    // ------------------------------------------------------------------
    // Entry point
    // ------------------------------------------------------------------

    /// Render a clip in the requested aspect ratio.
    ///
    /// 9:16: smart framing per shot, or the blurred-background letterbox for the
    /// "fit" style / as a fallback. 16:9: whole frame at the source's resolution
    /// (1080p-4K) and frame rate, over a blurred fill when the source isn't 16:9.
    pub async fn render_clip(&self, request: &RenderRequest, cancel: &CancellationToken) -> Result<RenderResult> {
        self.render_clip_with(request, cancel, |job| Box::pin(self.render_edit(job, cancel))).await
    }

    /// `render_clip` with the per-rung edit renderer injected (tests stub FFmpeg here).
    pub async fn render_clip_with<'r, F>(
        &self,
        request: &'r RenderRequest,
        cancel: &CancellationToken,
        mut render_edit: F,
    ) -> Result<RenderResult>
    where
        F: FnMut(EditJob<'r>) -> BoxFuture<'r, Result<()>>,
    {
        validate_video_speed(request.video_speed)?;
        if let Some(parent) = request.output_path.parent() {
            std::fs::create_dir_all(parent).map_err(io_render_error)?;
        }
        let duration_ms = request.end_time_ms - request.start_time_ms;
        if duration_ms <= 0 {
            return Err(render_error("Invalid clip duration"));
        }
        let is_landscape = request.aspect_ratio == ASPECT_HORIZONTAL;

        let (mut source_w, mut source_h) = (request.source_width, request.source_height);
        let (actual_w, actual_h) = self.get_video_dimensions(&request.video_path, cancel).await?;
        if actual_w > 0 && actual_h > 0 {
            (source_w, source_h) = (actual_w, actual_h);
        }
        let ((target_width, target_height), fps) = if is_landscape {
            (get_landscape_dimensions(source_w, source_h), self.probe_fps(&request.video_path, cancel).await?)
        } else {
            (get_output_dimensions(&request.aspect_ratio), "30".to_string())
        };
        tracing::info!(
            "Rendering clip: {}ms-{}ms, output={}x{}@{} ({}{}), include_audio={}",
            request.start_time_ms,
            request.end_time_ms,
            target_width,
            target_height,
            fps,
            request.aspect_ratio,
            if request.longform { ", longform" } else { "" },
            request.include_audio
        );

        let (window_start_ms, window_ms) = if request.apply_padding {
            self.compute_padded_range(request.start_time_ms, duration_ms)
        } else {
            (request.start_time_ms, duration_ms)
        };

        let timer = std::time::Instant::now();
        let mut plan: Option<ClipLayoutPlan> = None;
        if !is_landscape {
            plan = self.plan_layout(request, source_w, source_h, window_start_ms, window_ms, cancel).await?;
        }
        let analyzed = plan.is_some();
        // Pacing needs to know what's on screen even when the framing doesn't use it.
        let mut pacing_plan = plan.clone();
        if pacing_plan.is_none() && (is_landscape || request.layout_style == LAYOUT_STYLE_FIT) && paces(request) {
            pacing_plan = self.content_plan(request, source_w, source_h, window_start_ms, window_ms, cancel).await?;
        }
        let analysis_seconds = timer.elapsed().as_secs_f64();
        let face_samples = pacing_plan.as_ref().map(|p| p.face_samples.clone()).unwrap_or_default();
        let style_plan = |source: &str| ClipLayoutPlan {
            shots: vec![ShotLayout {
                start_ms: 0,
                end_ms: window_ms,
                layout: SCREEN.into(),
                source: source.into(),
                ..Default::default()
            }],
            source_width: source_w as u32,
            source_height: source_h as u32,
            vision_cost_usd: 0.0,
            face_samples: face_samples.clone(),
        };
        let plan = plan.unwrap_or_else(|| style_plan("style"));

        let skips = window_skips(request, window_start_ms, window_ms);
        let keeps = keep_intervals(request, pacing_plan.as_ref(), window_start_ms, window_ms);
        let time_map = TimeMap::new(subtract_intervals(&keeps, &skips), Some(window_ms));
        // Natural timing still honours the planner's skips: they're edit decisions, not pacing.
        let natural_map = TimeMap::new(subtract_intervals(&[(0, window_ms)], &skips), Some(window_ms));
        let smart = analyzed && !plan.is_letterbox_only();
        let vision_cost = if analyzed { plan.vision_cost_usd } else { 0.0 };

        // Longform audio gets two-pass (linear) loudness normalization.
        let timer = std::time::Instant::now();
        let mut loudness_filter = None;
        if request.longform && request.include_audio && self.has_audio(&request.video_path, cancel).await? {
            loudness_filter = self.measure_loudness(&request.video_path, window_start_ms, window_ms, cancel).await?;
        }
        let loudness_seconds = timer.elapsed().as_secs_f64();

        // Fallback ladder: as planned -> letterbox with the same cuts -> letterbox
        // at natural timing (a pacing edge case can be what failed, so cuts go last).
        let letterbox = style_plan("fallback");
        let mut ladder: Vec<(&ClipLayoutPlan, &TimeMap, Option<&str>)> = vec![(&plan, &time_map, None)];
        if smart {
            ladder.push((&letterbox, &time_map, Some("letterbox")));
        }
        if time_map.keeps != natural_map.keeps {
            ladder.push((&letterbox, &natural_map, Some("letterbox_natural")));
        }

        let timer = std::time::Instant::now();
        let mut rendered: Option<(&ClipLayoutPlan, &TimeMap, Option<&str>)> = None;
        for (step,&(step_plan, step_map, fallback)) in ladder.iter().enumerate() {
            let job = EditJob {
                request,
                plan: step_plan.clone(),
                time_map: step_map.clone(),
                window_start_ms,
                window_ms,
                width: target_width,
                height: target_height,
                landscape: is_landscape,
                fps: fps.clone(),
                loudness_filter: loudness_filter.clone(),
            };
            match render_edit(job).await {
                Ok(()) => {
                    rendered = Some((step_plan, step_map, fallback));
                    break;
                }
                // Cancellation is not a render failure: stop at once.
                Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
                Err(e) if step == ladder.len() - 1 => return Err(map_render_err(e)),
                Err(e) => {
                    tracing::warn!("Render failed, retrying as {}: {e}", ladder[step + 1].2.unwrap_or("letterbox"));
                }
            }
        }
        let encode_seconds = timer.elapsed().as_secs_f64();
        let (plan, time_map, render_fallback) = rendered.ok_or_else(|| render_error("no render attempted"))?;
        let (smart, analyzed) = if render_fallback.is_some() { (false, false) } else { (smart, analyzed) };

        let file_size = std::fs::metadata(&request.output_path).map_err(io_render_error)?.len();
        let removed_ms = time_map.removed_ms();
        let chapters = output_chapters(request, window_start_ms, time_map)?;
        let subtitle_path = self.write_subtitles(request, window_start_ms, time_map);
        let out_duration = scaled_duration_ms(time_map.output_ms, request.video_speed)?;
        tracing::info!(
            "Clip rendered: {} ({:.1} MB, {:.1}s at {}x{}{})",
            request.output_path.display(),
            file_size as f64 / 1024.0 / 1024.0,
            out_duration as f64 / 1000.0,
            pyfmt::fmt_g(request.video_speed, 6),
            if removed_ms > 0 {
                format!(", pacing removed {:.1}s in {} cuts", removed_ms as f64 / 1000.0, time_map.cut_count())
            } else {
                String::new()
            },
            render_fallback.map(|f| format!(", fallback={f}")).unwrap_or_default(),
        );
        Ok(RenderResult {
            output_path: request.output_path.clone(),
            file_size_bytes: file_size,
            duration_ms: out_duration,
            removed_ms,
            cut_count: time_map.cut_count(),
            layout_type: if smart { plan.dominant_layout().to_string() } else { "fit".into() },
            layout_shots: if analyzed { plan.shots.iter().map(|s| s.summary()).collect() } else { Vec::new() },
            // Vision calls made for the plan are paid for even if a fallback rendered.
            layout_cost_usd: vision_cost,
            render_fallback: render_fallback.map(str::to_string),
            chapters,
            subtitle_path,
            output_width: target_width,
            output_height: target_height,
            fps: fps.clone(),
            analysis_seconds,
            loudness_seconds,
            encode_seconds,
            edit: Some(RenderedEdit { window_start_ms, plan: plan.clone(), keeps: time_map.keeps.clone() }),
        })
    }

    fn compute_padded_range(&self, start_time_ms: i64, duration_ms: i64) -> (i64, i64) {
        compute_padded_range(self.audio_padding_ms, start_time_ms, duration_ms)
    }

    // ------------------------------------------------------------------
    // One edit
    // ------------------------------------------------------------------

    /// Build the graph, captions and overlays for one edit and run FFmpeg.
    pub async fn render_edit(&self, job: EditJob<'_>, cancel: &CancellationToken) -> Result<()> {
        let request = job.request;
        let (plan, time_map, fps) = (&job.plan, &job.time_map, job.fps.as_str());
        let has_audio = request.include_audio && self.has_audio(&request.video_path, cancel).await?;
        let mut graph = build_layout_graph(
            plan,
            job.width,
            job.height,
            &GraphOptions {
                keeps: Some(&time_map.keeps),
                with_audio: has_audio,
                landscape: job.landscape,
                fps,
                loudness_filter: job.loudness_filter.as_deref(),
                video_speed: request.video_speed,
                backdrop: None,
            },
        )?;
        let out_plan = remap_plan(plan, time_map);
        let caption_path = self.generate_captions(
            request,
            job.width,
            job.height,
            job.window_start_ms,
            time_map,
            &out_plan,
            job.landscape,
            Some(plan),
        )?;
        graph += &format!(";[base]{}[captioned]", self.caption_filter(caption_path.as_deref()));
        let overlays = self.overlays(request, &out_plan, job.width, job.height, job.landscape)?;
        // Burn captions and animate framing/overlays on the edited source clock,
        // then speed up the entire composited picture to match the tempo audio.
        let (filter_complex, extra_inputs) =
            compose_overlays(&graph, &overlays, &speed_video_filter(request.video_speed, fps)?);

        let result = self
            .run_ffmpeg_complex(
                &FfmpegJob {
                    input_path: &request.video_path,
                    more_sources: &[],
                    audio_inputs: &[],
                    output_path: &request.output_path,
                    start_time_ms: job.window_start_ms,
                    duration_ms: job.window_ms,
                    filter_complex: &filter_complex,
                    audio_label: has_audio.then_some("[aout]"),
                    extra_inputs: &extra_inputs,
                    fps,
                    output_size: (job.width, job.height),
                    output_duration_ms: Some(scaled_duration_ms(time_map.output_ms, request.video_speed)?),
                    progress_path: None,
                },
                cancel,
            )
            .await;
        for path in &extra_inputs {
            let _ = std::fs::remove_file(path);
        }
        result?;
        if !request.output_path.is_file() {
            return Err(render_error("Render failed: output file not created"));
        }
        Ok(())
    }

    async fn plan_layout(
        &self,
        request: &RenderRequest,
        source_w: i64,
        source_h: i64,
        window_start_ms: i64,
        window_ms: i64,
        cancel: &CancellationToken,
    ) -> Result<Option<ClipLayoutPlan>> {
        if request.layout_style == LAYOUT_STYLE_FIT {
            return Ok(None);
        }
        let style = request.layout_style.clone();
        self.analyze(request, source_w, source_h, window_start_ms, window_ms, style, request.layout_vision, cancel)
            .await
            .map(|r| {
                r.unwrap_or_else(|e| {
                    tracing::warn!("Layout analysis failed, falling back to letterbox: {e}");
                    None
                })
            })
    }

    /// Heuristic shot analysis for pacing only (no paid vision call). None if unavailable.
    async fn content_plan(
        &self,
        request: &RenderRequest,
        source_w: i64,
        source_h: i64,
        window_start_ms: i64,
        window_ms: i64,
        cancel: &CancellationToken,
    ) -> Result<Option<ClipLayoutPlan>> {
        let style = LAYOUT_STYLE_AUTO.to_string();
        self.analyze(request, source_w, source_h, window_start_ms, window_ms, style, false, cancel).await.map(|r| {
            r.unwrap_or_else(|e| {
                tracing::warn!("Content analysis for pacing failed; using the default pause limit: {e}");
                None
            })
        })
    }

    /// Outer error: cancellation only. Inner: the analyzer's own outcome.
    #[allow(clippy::too_many_arguments)]
    async fn analyze(
        &self,
        request: &RenderRequest,
        source_w: i64,
        source_h: i64,
        window_start_ms: i64,
        window_ms: i64,
        style: String,
        vision: bool,
        cancel: &CancellationToken,
    ) -> Result<Result<Option<ClipLayoutPlan>>> {
        let Some(analyzer) = &self.analyzer else { return Ok(Ok(None)) };
        let req = LayoutRequest {
            video_path: request.video_path.clone(),
            start_ms: window_start_ms,
            duration_ms: window_ms,
            src_w: source_w.max(0) as u32,
            src_h: source_h.max(0) as u32,
            style,
            vision,
            vision_model: None,
        };
        match analyzer(req, cancel.clone()).await {
            Err(EngineError::Cancelled) => Err(EngineError::Cancelled),
            other => Ok(other),
        }
    }

    /// Title card and channel banner, positioned per shot on the output timeline.
    pub fn overlays(
        &self,
        request: &RenderRequest,
        out_plan: &ClipLayoutPlan,
        target_width: i64,
        target_height: i64,
        is_landscape: bool,
    ) -> Result<Vec<Overlay>> {
        let (src_w, src_h) = (out_plan.source_width as i64, out_plan.source_height as i64);
        let mut overlays = Vec::new();
        // Landscape overlays were sized for 1080p; scale them with the output.
        let scale = if is_landscape { target_height as f64 / 1080.0 } else { 1.0 };
        if let Some((path, _, height)) = self.title_overlay_image(request, target_width, scale)? {
            if is_landscape {
                let (show_from, show_to) = LANDSCAPE_TITLE_SHOW_S;
                let fade = LANDSCAPE_TITLE_FADE_S;
                overlays.push(Overlay {
                    path,
                    x: "(W-w)/2".into(),
                    y: format!("{}", pyfmt::round(40.0 * scale)),
                    enable: Some(format!("between(t,{},{})", pyfmt::repr(show_from), pyfmt::repr(show_to))),
                    filter: Some(format!(
                        "format=rgba,fade=t=in:st={}:d={}:alpha=1,fade=t=out:st={}:d={}:alpha=1",
                        pyfmt::repr(show_from),
                        pyfmt::repr(fade),
                        pyfmt::repr(show_to - fade),
                        pyfmt::repr(fade),
                    )),
                });
            } else {
                let values: Vec<i64> = out_plan
                    .shots
                    .iter()
                    .map(|s| title_y(s, src_w, src_h, target_width, target_height, height))
                    .collect();
                overlays.push(Overlay { path, x: "(W-w)/2".into(), y: per_shot_expr(out_plan, &values), ..Default::default() });
            }
        }
        let banner_size = if is_landscape { pyfmt::round(28.0 * scale) } else { 34 };
        if let Some((path, _, _)) = self.banner_overlay_image(request, banner_size)? {
            if is_landscape {
                overlays.push(Overlay {
                    path,
                    x: format!("W-w-{}", pyfmt::round(20.0 * scale)),
                    y: format!("H-h-{}", pyfmt::round(16.0 * scale)),
                    ..Default::default()
                });
            } else {
                let values: Vec<i64> =
                    out_plan.shots.iter().map(|s| banner_y(s, src_w, src_h, target_width, target_height)).collect();
                overlays.push(Overlay { path, x: "(W-w)/2".into(), y: per_shot_expr(out_plan, &values), ..Default::default() });
            }
        }
        overlays.extend(self.logo_overlays(request, target_width)?);
        Ok(overlays)
    }

    /// The request's logos in their corners, scaled to `size` of the frame width. Each
    /// is copied next to the clip first: overlay inputs are deleted after the render.
    fn logo_overlays(&self, request: &RenderRequest, target_width: i64) -> Result<Vec<Overlay>> {
        let margin = pyfmt::round(target_width as f64 * LOGO_MARGIN);
        let mut overlays = Vec::new();
        for (i, logo) in request.logos.iter().enumerate() {
            let source = Path::new(&logo.path);
            let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
            let path = sibling(
                &request.output_path,
                &format!("logo-{}-{}-{i}.{ext}", request.start_time_ms, request.end_time_ms),
            );
            if let Err(e) = std::fs::copy(source, &path) {
                tracing::warn!("Logo {} unavailable; rendering without it: {e}", i + 1);
                continue;
            }
            let width = even(target_width as f64 * logo.size.clamp(MIN_LOGO_SIZE, MAX_LOGO_SIZE));
            let (right, bottom) = (logo.position.ends_with("right"), logo.position.starts_with("bottom"));
            overlays.push(Overlay {
                path,
                x: if right { format!("W-w-{margin}") } else { margin.to_string() },
                y: if bottom { format!("H-h-{margin}") } else { margin.to_string() },
                filter: Some(format!("scale={width}:-1:flags=lanczos,format=rgba")),
                enable: None,
            });
        }
        Ok(overlays)
    }

    /// `ass=` filter for the caption file, or a no-op.
    pub fn caption_filter(&self, caption_path: Option<&Path>) -> String {
        match caption_path {
            Some(path) if path.is_file() => {
                let mut ass = format!("ass={}", escape_filter_path(&path.to_string_lossy()));
                if let Some(dir) = &self.fonts_dir {
                    ass += &format!(":fontsdir={}", escape_filter_path(&dir.to_string_lossy()));
                }
                ass
            }
            _ => "null".into(),
        }
    }

    pub(crate) fn title_overlay_image(
        &self,
        request: &RenderRequest,
        target_width: i64,
        scale: f64,
    ) -> Result<Option<(PathBuf, i64, i64)>> {
        let Some(title) = request.title_text.as_deref() else { return Ok(None) };
        if title.is_empty() {
            return Ok(None);
        }
        let path = sibling(&request.output_path, &format!("title-{}-{}.png", request.start_time_ms, request.end_time_ms));
        Ok(self.build_title_card(title, target_width, &path, scale)?.map(|(w, h)| (path, w, h)))
    }

    /// Rounded white card with the title in 1-2 lines. Returns (width, height).
    fn build_title_card(&self, title_text: &str, _target_width: i64, output_path: &Path, scale: f64) -> Result<Option<(i64, i64)>> {
        let clean = title_text.replace('\n', " ");
        let clean = clean.trim();
        if clean.is_empty() {
            return Ok(None);
        }
        let mut words: Vec<&str> = clean.split_whitespace().collect();
        words.truncate(7);
        let lines: Vec<String> = if words.len() <= 3 {
            vec![words.join(" ")]
        } else {
            let mid = words.len().div_ceil(2);
            vec![words[..mid].join(" "), words[mid..].join(" ")]
        };

        let font_size = pyfmt::round(42.0 * scale);
        let pad_x = pyfmt::round(28.0 * scale);
        let pad_y = pyfmt::round(20.0 * scale);
        let line_spacing = pyfmt::int(font_size as f64 * 1.35);
        let corner_radius = pyfmt::round(16.0 * scale);

        let Some(font) = self.font_path.as_deref().and_then(|p| TextFont::load(p, font_size as f64)) else {
            tracing::warn!("No title font available; skipping the title card");
            return Ok(None);
        };
        let widths: Vec<i64> = lines.iter().map(|l| font.bbox(l)).map(|b| b.2 - b.0).collect();
        let max_text_w = widths.iter().copied().max().unwrap_or(0);
        let text_block_h = font_size + (lines.len() as i64 - 1).max(0) * line_spacing;
        let (img_w, img_h) = ((max_text_w + pad_x * 2).max(1), (text_block_h + pad_y * 2).max(1));

        let mut canvas = Canvas::new(img_w as u32, img_h as u32);
        canvas.rounded_rect(corner_radius as f64, [255, 255, 255, 242]);
        for (i, (line, tw)) in lines.iter().zip(&widths).enumerate() {
            let x = (img_w - tw).div_euclid(2);
            let y = pad_y + i as i64 * line_spacing;
            font.draw(&mut canvas, x, y, line, [0, 0, 0, 255]);
        }
        std::fs::write(output_path, canvas.encode_png().map_err(io_render_error)?).map_err(io_render_error)?;
        Ok(Some((img_w, img_h)))
    }

    /// The channel URL banner as a transparent PNG (drawn here, not with FFmpeg's
    /// drawtext, which the bundled FFmpeg builds omit). Returns (path, width, height).
    pub(crate) fn banner_overlay_image(&self, request: &RenderRequest, font_size: i64) -> Result<Option<(PathBuf, i64, i64)>> {
        let (Some(platform), Some(url)) = (request.banner_platform.as_deref(), request.banner_channel_url.as_deref())
        else {
            return Ok(None);
        };
        if platform.is_empty() || url.is_empty() {
            return Ok(None);
        }
        // Shown as typed, minus the scheme: "https://youtube.com/@me" reads as YOUTUBE.COM/@ME.
        let url = url.trim();
        let lower = url.to_ascii_lowercase();
        let url = ["https://", "http://"].iter().find(|p| lower.starts_with(**p)).map_or(url, |p| &url[p.len()..]);
        let text = url.trim_end_matches('/').to_uppercase();
        let text = text.trim();
        let path = sibling(&request.output_path, &format!("banner-{}-{}.png", request.start_time_ms, request.end_time_ms));
        let Some(font) = self.font_path.as_deref().and_then(|p| TextFont::load(p, font_size as f64)) else {
            tracing::warn!("No banner font available; skipping the channel banner");
            return Ok(None);
        };
        let bbox = font.bbox(text);
        let shadow = 2;
        let width = bbox.2 - bbox.0 + shadow + 4;
        let height = bbox.3 - bbox.1 + shadow + 4;
        let mut canvas = Canvas::new(width.max(1) as u32, height.max(1) as u32);
        let origin = (2 - bbox.0, 2 - bbox.1);
        font.draw(&mut canvas, origin.0 + shadow, origin.1 + shadow, text, [0, 0, 0, 128]);
        font.draw(&mut canvas, origin.0, origin.1, text, [255, 255, 255, 255]);
        std::fs::write(&path, canvas.encode_png().map_err(io_render_error)?).map_err(io_render_error)?;
        Ok(Some((path, width, height)))
    }

    /// ASS captions on the edited timeline, positioned per shot (9:16). With the
    /// window-time `plan`, each caption group also moves off any face it would cover.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_captions(
        &self,
        request: &RenderRequest,
        target_width: i64,
        target_height: i64,
        window_start_ms: i64,
        time_map: &TimeMap,
        out_plan: &ClipLayoutPlan,
        is_landscape: bool,
        plan: Option<&ClipLayoutPlan>,
    ) -> Result<Option<PathBuf>> {
        if !request.include_captions || request.transcript_segments.is_empty() {
            return Ok(None);
        }
        let segments = remap_segments(&request.transcript_segments, window_start_ms, time_map);
        if segments.is_empty() {
            return Ok(None);
        }

        let mut anchors: Option<Vec<(i64, i64, i64)>> = None;
        if !is_landscape {
            let (src_w, src_h) = (out_plan.source_width as i64, out_plan.source_height as i64);
            let mut a: Vec<(i64, i64, i64)> = out_plan
                .shots
                .iter()
                .map(|shot| {
                    let (alignment, y) = caption_anchor(shot, src_w, src_h, target_width, target_height);
                    (shot.end_ms, alignment, y)
                })
                .collect();
            if let Some(last) = a.last_mut() {
                last.0 = 1_000_000_000;
            }
            anchors = Some(a);
        }

        let mut placer = None;
        if let (Some(anchors), Some(plan)) = (&anchors, plan) {
            if !anchors.is_empty() {
                let zones = face_zones(plan, time_map, target_width, target_height);
                if !zones.is_empty() {
                    placer = Some(CaptionPlacer::new(anchors.clone(), zones, target_width, target_height));
                }
            }
        }

        let caption_path = sibling(&request.output_path, &format!("clip-{}-{}.ass", request.start_time_ms, request.end_time_ms));
        let mut style = request.caption_style.clone().unwrap_or_else(|| captions::caption_style_for(captions::DEFAULT_CAPTION_PRESET));
        if is_landscape {
            style = landscape_caption_style(&style, target_height);
        }
        let caption_req = CaptionRequest {
            clip_start_ms: window_start_ms,
            clip_end_ms: window_start_ms + time_map.output_ms,
            style: &style,
            output_width: target_width,
            output_height: target_height,
            video_region: None,
            anchors: anchors.as_deref(),
            emphasis_words: &request.emphasis_words,
            fonts_dir: self.fonts_dir.as_deref(),
        };
        let mut place_fn;
        let placer_ref: Option<captions::Placer> = match placer.as_mut() {
            Some(p) => {
                place_fn = |s: i64, e: i64, w: i64, h: i64| p.place(s, e, w, h);
                Some(&mut place_fn)
            }
            None => None,
        };
        captions::generate_captions(&segments, &caption_req, &caption_path, placer_ref).map_err(io_render_error)
    }

    /// SRT sidecar on the edited timeline, for longform uploads. Failures only log.
    pub fn write_subtitles(&self, request: &RenderRequest, window_start_ms: i64, time_map: &TimeMap) -> Option<PathBuf> {
        if !request.longform || request.transcript_segments.is_empty() {
            return None;
        }
        let result = (|| -> Result<Option<PathBuf>> {
            let speed = request.video_speed;
            let mut segments = remap_segments(&request.transcript_segments, window_start_ms, time_map);
            let scale = |t: i64| -> Result<i64> { Ok(window_start_ms + scaled_duration_ms(t - window_start_ms, speed)?) };
            for seg in &mut segments {
                seg.start_time_ms = scale(seg.start_time_ms)?;
                seg.end_time_ms = (seg.start_time_ms + 1).max(scale(seg.end_time_ms)?);
                for w in &mut seg.words {
                    w.start_time_ms = scale(w.start_time_ms)?;
                    w.end_time_ms = (w.start_time_ms + 1).max(scale(w.end_time_ms)?);
                }
            }
            let path = request.output_path.with_extension("srt");
            let end = window_start_ms + scaled_duration_ms(time_map.output_ms, speed)?;
            captions::generate_srt(&segments, window_start_ms, end, &path).map_err(io_render_error)
        })();
        result.unwrap_or_else(|e| {
            tracing::warn!("Subtitle sidecar failed; the clip is unaffected: {e}");
            None
        })
    }

    // ------------------------------------------------------------------
    // Probes
    // ------------------------------------------------------------------

    fn probe_args(&self, head: &[&str], path: &Path) -> Vec<String> {
        head.iter()
            .map(|s| s.to_string())
            .chain(MEDIA_INPUT_OPTIONS.iter().map(|s| s.to_string()))
            .chain([path.to_string_lossy().into_owned()])
            .collect()
    }

    async fn probe(&self, args: &[String], cancel: &CancellationToken) -> Result<MediaOutput> {
        self.runner.run(&self.ffprobe, args, PROBE_TIMEOUT, MAX_TOOL_OUTPUT_BYTES, cancel).await
    }

    /// Source frame rate as an FFmpeg rational, capped at MAX_OUTPUT_FPS; "30" when unknown.
    pub async fn probe_fps(&self, video_path: &Path, cancel: &CancellationToken) -> Result<String> {
        let args = self.probe_args(
            &["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=avg_frame_rate,r_frame_rate", "-of", "json"],
            video_path,
        );
        match self.probe(&args, cancel).await {
            Err(EngineError::Cancelled) => Err(EngineError::Cancelled),
            Err(e) => {
                tracing::warn!("Failed to probe frame rate, using 30 fps: {e}");
                Ok("30".into())
            }
            Ok(out) => Ok(parse_probe_fps(&out.stdout).unwrap_or_else(|| "30".into())),
        }
    }

    /// Actual video dimensions, or (0, 0) when the probe fails.
    pub async fn get_video_dimensions(&self, video_path: &Path, cancel: &CancellationToken) -> Result<(i64, i64)> {
        let args = self.probe_args(
            &["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=s=x:p=0"],
            video_path,
        );
        match self.probe(&args, cancel).await {
            Err(EngineError::Cancelled) => Err(EngineError::Cancelled),
            Err(e) => {
                tracing::warn!("Failed to probe video dimensions: {e}");
                Ok((0, 0))
            }
            Ok(out) => Ok(parse_dimensions(&out.stdout).unwrap_or_else(|| {
                tracing::warn!("Failed to probe video dimensions");
                (0, 0)
            })),
        }
    }

    /// Whether the source has an audio stream (the graph maps [0:a] only if so).
    pub async fn has_audio(&self, video_path: &Path, cancel: &CancellationToken) -> Result<bool> {
        let args = self.probe_args(
            &["-v", "error", "-select_streams", "a", "-show_entries", "stream=index", "-of", "csv=p=0"],
            video_path,
        );
        let out = self.probe(&args, cancel).await.map_err(map_render_err)?;
        if !out.success() {
            return Err(render_error("Could not verify the source audio track"));
        }
        Ok(!String::from_utf8_lossy(&out.stdout).trim().is_empty())
    }

    /// First loudnorm pass over the render window; the linear second-pass filter,
    /// or None to fall back to single-pass.
    pub async fn measure_loudness(
        &self,
        video_path: &Path,
        start_ms: i64,
        duration_ms: i64,
        cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        let args = measure_loudness_args(video_path, start_ms, duration_ms);
        let out = match self.runner.run(&self.ffmpeg, &args, MEDIA_TIMEOUT, MAX_TOOL_OUTPUT_BYTES, cancel).await {
            Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
            Err(e) => {
                tracing::warn!("Loudness measurement failed; using single-pass normalization: {e}");
                return Ok(None);
            }
            Ok(out) => out,
        };
        let text = String::from_utf8_lossy(&out.stderr);
        let found = regex::Regex::new(r#"\{[^{}]*"input_i"[^{}]*\}"#)
            .ok()
            .and_then(|re| re.find(&text).map(|m| m.as_str().to_string()));
        let measured = match (out.success(), found) {
            (true, Some(json)) => serde_json::from_str::<serde_json::Value>(&json).ok(),
            _ => None,
        };
        let Some(measured) = measured else {
            tracing::warn!("Loudness measurement failed; using single-pass normalization: no loudnorm measurement in output");
            return Ok(None);
        };
        let loudness = measured_loudness_filter(&measured);
        if loudness.is_some() {
            tracing::info!("Two-pass loudness normalization enabled for this clip");
        }
        Ok(loudness)
    }

    // ------------------------------------------------------------------
    // FFmpeg
    // ------------------------------------------------------------------

    /// Encoder arguments: VideoToolbox on macOS, software H.264 elsewhere.
    /// Keyframes every 2 s keep long clips seekable.
    pub async fn video_codec_args(&self, out_w: i64, out_h: i64, fps: &str, cancel: &CancellationToken) -> Result<Vec<String>> {
        if let Some(args) = &self.video_codec_override {
            return Ok(args.clone());
        }
        if self.videotoolbox {
            return Ok(video_codec_args(true, out_w, out_h, fps));
        }
        self.software_codec_args(out_w, out_h, fps, cancel).await
    }

    /// Software H.264: libx264 where FFmpeg has it (GPL builds), else the LGPL bundles' own
    /// encoder (`lgpl_software_codec_args`).
    pub async fn software_codec_args(&self, out_w: i64, out_h: i64, fps: &str, cancel: &CancellationToken) -> Result<Vec<String>> {
        let x264 = *self
            .x264
            .get_or_try_init(|| async {
                let probe: Vec<String> = [
                    "-hide_banner", "-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=64x64:r=30",
                    "-c:v", "libx264", "-preset", FFMPEG_PRESET, "-frames:v", "1", "-f", "null", "-",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect();
                match self.runner.run(&self.ffmpeg, &probe, PROBE_TIMEOUT, MAX_TOOL_OUTPUT_BYTES, cancel).await {
                    Err(EngineError::Cancelled) => Err(EngineError::Cancelled),
                    out => Ok(out.is_ok_and(|o| o.success())),
                }
            })
            .await?;
        Ok(if x264 { video_codec_args(false, out_w, out_h, fps) } else { lgpl_software_codec_args(out_w, out_h, fps) })
    }

    /// The first hardware encoder that passes a test encode, probed once per renderer.
    pub async fn hw_encoder(&self, cancel: &CancellationToken) -> Result<Option<HwEncoder>> {
        let found = self
            .hw_encoder
            .get_or_try_init(|| async {
                for &encoder in HwEncoder::candidates() {
                    match self.runner.run(&self.ffmpeg, &encoder.probe_args(), PROBE_TIMEOUT, MAX_TOOL_OUTPUT_BYTES, cancel).await {
                        Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
                        Ok(out) if out.success() => {
                            tracing::info!("Hardware video encoder: {encoder:?}");
                            return Ok(Some(encoder));
                        }
                        _ => tracing::debug!("Hardware encoder {encoder:?} unavailable"),
                    }
                }
                tracing::info!("No hardware video encoder; encoding in software");
                Ok(None)
            })
            .await?;
        Ok(*found)
    }

    /// Run FFmpeg over the render window [start, start + duration) with a filter graph,
    /// then verify the export's timing. A failed export never stays on disk.
    /// A failed hardware encode (GPU, or VideoToolbox) is retried once in software.
    pub async fn run_ffmpeg_complex(&self, job: &FfmpegJob<'_>, cancel: &CancellationToken) -> Result<()> {
        let (w, h) = job.output_size;
        let hw = if self.video_codec_override.is_some() || self.videotoolbox { None } else { self.hw_encoder(cancel).await? };
        let mut result = match hw {
            Some(encoder) => {
                let mut args = build_ffmpeg_args(job, &encoder.codec_args(job.fps));
                encoder.adapt_args(&mut args);
                self.encode_and_validate(job, &args, cancel).await
            }
            None => self.encode_and_validate(job, &build_ffmpeg_args(job, &self.video_codec_args(w, h, job.fps, cancel).await?), cancel).await,
        };
        let hardware = hw.is_some() || (self.videotoolbox && self.video_codec_override.is_none());
        if let (true, Err(EngineError::Failed(detail))) = (hardware, &result) {
            if detail != DISK_FULL {
                tracing::warn!("Hardware encode failed; retrying in software: {detail}");
                result = match self.software_codec_args(w, h, job.fps, cancel).await {
                    Ok(args) => self.encode_and_validate(job, &build_ffmpeg_args(job, &args), cancel).await,
                    Err(e) => Err(e),
                };
            }
        }
        if result.is_err() {
            let _ = std::fs::remove_file(job.output_path);
        }
        result
    }

    async fn encode_and_validate(&self, job: &FfmpegJob<'_>, args: &[String], cancel: &CancellationToken) -> Result<()> {
        self.run_cmd(args, cancel).await?;
        self.validate_output_timing(
            job.output_path,
            job.output_duration_ms.unwrap_or(job.duration_ms),
            job.fps,
            job.audio_label.is_some(),
            cancel,
        )
        .await
    }

    /// Run FFmpeg; long filter graphs go through a private script file.
    pub async fn run_cmd(&self, args: &[String], cancel: &CancellationToken) -> Result<()> {
        let run = |args: Vec<String>| async move {
            self.runner.run(&self.ffmpeg, &args, MEDIA_TIMEOUT, MAX_TOOL_OUTPUT_BYTES, cancel).await.map_err(map_render_err)
        };
        let graph_index = args.iter().position(|a| a == "-filter_complex");
        let out = match graph_index {
            Some(i) if args.get(i + 1).map(|g| g.len() > MAX_INLINE_FILTER_GRAPH_BYTES).unwrap_or(false) => {
                // A private (0600) file, removed when `script` drops - after FFmpeg exits,
                // or right after the child is killed on cancellation.
                let mut builder = tempfile::Builder::new();
                builder.prefix("clipward-filter-").suffix(".ffgraph");
                let file = match &self.temp_dir {
                    Some(dir) => builder.tempfile_in(dir),
                    None => builder.tempfile(),
                }
                .map_err(io_render_error)?;
                std::fs::write(file.path(), &args[i + 1]).map_err(io_render_error)?;
                let script = file.into_temp_path();
                let mut script_args = args.to_vec();
                script_args[i] = "-/filter_complex".into();
                script_args[i + 1] = script.to_string_lossy().into_owned();
                let mut out = run(script_args.clone()).await?;
                // FFmpeg 6 predates file-backed option values; FFmpeg 9 removed the
                // older script option. Retry only when the option itself is unknown.
                if !out.success() {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if stderr.contains("Unrecognized option '/filter_complex'.")
                        && stderr.contains("Error splitting the argument list: Option not found")
                    {
                        script_args[i] = "-filter_complex_script".into();
                        out = run(script_args).await?;
                    }
                }
                drop(script);
                out
            }
            _ => run(args.to_vec()).await?,
        };
        if !out.success() {
            let text = if out.stderr.is_empty() {
                "Unknown error".to_string()
            } else {
                pyfmt::tail_chars(&String::from_utf8_lossy(&out.stderr), 1000).to_string()
            };
            return Err(render_error(format!("FFmpeg failed: {text}")));
        }
        Ok(())
    }

    /// Check the encoded presentation clock before publishing an export.
    pub async fn validate_output_timing(
        &self,
        output_path: &Path,
        duration_ms: i64,
        fps: &str,
        with_audio: bool,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let failed = |detail: String| {
            tracing::warn!("Export timing rejected: {detail}");
            render_error("Export timing validation failed; the clip was not saved")
        };
        let mut args: Vec<String> = ["-v", "error"].iter().map(|s| s.to_string()).collect();
        args.extend(MEDIA_INPUT_OPTIONS.iter().map(|s| s.to_string()));
        args.extend(
            ["-show_entries", "stream=codec_type,start_time,duration", "-of", "json"].iter().map(|s| s.to_string()),
        );
        args.push(output_path.to_string_lossy().into_owned());
        let out = match self.probe(&args, cancel).await {
            Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
            Err(e) => return Err(failed(e.to_string())),
            Ok(out) => out,
        };
        if !out.success() {
            return Err(failed("probe failed".into()));
        }
        check_stream_timing(&out.stdout, duration_ms, fps, with_audio).map_err(failed)?;
        if with_audio {
            self.validate_audio_packets(output_path, cancel).await.map_err(|e| match e {
                EngineError::Cancelled => EngineError::Cancelled,
                other => failed(other.to_string()),
            })?;
        }
        tracing::info!(
            "Export timing verified: {:.3}s at {} fps, audio={}",
            duration_ms as f64 / 1000.0,
            fps,
            with_audio
        );
        Ok(())
    }

    /// Verify the continuous 48 kHz AAC clock, with bounded memory.
    async fn validate_audio_packets(&self, output_path: &Path, cancel: &CancellationToken) -> Result<()> {
        let mut args: Vec<String> = ["-v", "error"].iter().map(|s| s.to_string()).collect();
        args.extend(MEDIA_INPUT_OPTIONS.iter().map(|s| s.to_string()));
        args.extend(
            [
                "-select_streams",
                "a:0",
                "-show_entries",
                "packet=pts_time,duration_time:packet_side_data=",
                "-of",
                "compact=p=0:nk=0",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        args.push(output_path.to_string_lossy().into_owned());
        let mut checker = AudioPacketChecker::default();
        let mut on_line = |line: &[u8]| checker.line(line);
        let out = self.runner.run_lines(&self.ffprobe, &args, PROBE_TIMEOUT, 512, &mut on_line, cancel).await?;
        checker.finish(out.code).map_err(EngineError::Failed)
    }
}

/// Options for one FFmpeg render run.
pub struct FfmpegJob<'a> {
    pub input_path: &'a Path,
    /// More (start_ms, duration_ms) reads of `input_path`, as inputs 1.. before `extra_inputs`.
    pub more_sources: &'a [(i64, i64)],
    /// (file, start_ms, duration_ms) sound reads, inputs after `more_sources`.
    pub audio_inputs: &'a [(PathBuf, i64, i64)],
    pub output_path: &'a Path,
    pub start_time_ms: i64,
    pub duration_ms: i64,
    pub filter_complex: &'a str,
    pub audio_label: Option<&'a str>,
    pub extra_inputs: &'a [PathBuf],
    pub fps: &'a str,
    pub output_size: (i64, i64),
    pub output_duration_ms: Option<i64>,
    /// FFmpeg writes `-progress` key=value lines here (out_time_us=...), for a progress bar.
    pub progress_path: Option<&'a Path>,
}

/// The exact FFmpeg argument list (without the program) for a render.
/// The graph must output [out] and, with `audio_label`, that audio label.
pub fn build_ffmpeg_args(job: &FfmpegJob, codec_args: &[String]) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let mut cmd: Vec<String> = vec![s("-nostdin"), s("-nostats"), s("-y")];
    for (start_ms, duration_ms) in std::iter::once((job.start_time_ms, job.duration_ms)).chain(job.more_sources.iter().copied()) {
        cmd.extend([
            s("-accurate_seek"),
            s("-ss"),
            format!("{:.6}", start_ms as f64 / 1000.0),
            // Input option: bound the source read to the window.
            s("-t"),
            format!("{:.6}", duration_ms as f64 / 1000.0),
        ]);
        cmd.extend(MEDIA_INPUT_OPTIONS.iter().map(|v| s(v)));
        cmd.push(s("-i"));
        cmd.push(job.input_path.to_string_lossy().into_owned());
    }
    for (path, start_ms, duration_ms) in job.audio_inputs {
        cmd.extend([s("-ss"), format!("{:.6}", *start_ms as f64 / 1000.0), s("-t"), format!("{:.6}", *duration_ms as f64 / 1000.0)]);
        cmd.extend(MEDIA_INPUT_OPTIONS.iter().map(|v| s(v)));
        cmd.push(s("-i"));
        cmd.push(path.to_string_lossy().into_owned());
    }
    for extra in job.extra_inputs {
        cmd.extend([s("-loop"), s("1"), s("-protocol_whitelist"), s("file,pipe,fd"), s("-i")]);
        cmd.push(extra.to_string_lossy().into_owned());
    }
    cmd.extend([s("-filter_complex"), s(job.filter_complex), s("-map"), s("[out]")]);
    cmd.extend(codec_args.iter().cloned());
    cmd.extend([s("-pix_fmt"), s("yuv420p"), s("-r"), s(job.fps), s("-movflags"), s("+faststart")]);
    match job.audio_label {
        Some(label) => cmd.extend([s("-map"), s(label), s("-c:a"), s("aac"), s("-b:a"), s("192k")]),
        None => cmd.push(s("-an")),
    }
    if !job.extra_inputs.is_empty() {
        cmd.push(s("-shortest"));
    }
    if let Some(progress) = job.progress_path {
        cmd.extend([s("-progress"), progress.to_string_lossy().into_owned()]);
    }
    cmd.push(job.output_path.to_string_lossy().into_owned());
    cmd
}

/// Landscape H.264 bitrates (Mbps) by output height at 30 fps.
fn landscape_bitrate_mbps(out_h: i64) -> i64 {
    match out_h {
        1080 => 12,
        1440 => 20,
        2160 => 45,
        _ => 12,
    }
}

/// Keyframe interval arguments: every 2 s.
fn gop_args(fps: &str) -> Vec<String> {
    let rate = Rational::parse(fps).map(|r| r.to_f64()).unwrap_or(30.0);
    vec!["-g".to_string(), format!("{}", 1.max(pyfmt::round(rate * 2.0)))]
}

/// Hardware H.264 encoders probed where VideoToolbox isn't used (Windows, Linux).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwEncoder {
    Nvenc,
    Qsv,
    Amf,
    Vaapi,
}

/// Render node the VAAPI encoder opens (the first GPU).
pub const VAAPI_DEVICE: &str = "/dev/dri/renderD128";

impl HwEncoder {
    /// Probe order for this OS.
    pub fn candidates() -> &'static [HwEncoder] {
        use HwEncoder::*;
        if cfg!(windows) {
            &[Nvenc, Qsv, Amf]
        } else if cfg!(target_os = "linux") {
            &[Nvenc, Vaapi, Qsv]
        } else {
            &[]
        }
    }

    /// Constant-quality settings aimed at libx264 CRF 20.
    // ponytail: vendor quality presets, not yet compared with libx264 on real GPUs; retune after side-by-side checks.
    pub fn codec_args(self, fps: &str) -> Vec<String> {
        let args: &[&str] = match self {
            HwEncoder::Nvenc => {
                &["-c:v", "h264_nvenc", "-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", "21", "-b:v", "0", "-profile:v", "high"]
            }
            HwEncoder::Qsv => &["-c:v", "h264_qsv", "-preset", "slow", "-global_quality", "21", "-profile:v", "high"],
            HwEncoder::Amf => {
                &["-c:v", "h264_amf", "-quality", "quality", "-rc", "cqp", "-qp_i", "20", "-qp_p", "22", "-qp_b", "24", "-profile:v", "high"]
            }
            HwEncoder::Vaapi => &["-c:v", "h264_vaapi", "-rc_mode", "CQP", "-qp", "21", "-profile:v", "high"],
        };
        let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        args.extend(gop_args(fps));
        args
    }

    /// Fit an FFmpeg argument list (graph output [out], `-pix_fmt yuv420p`) to this encoder.
    pub fn adapt_args(self, args: &mut Vec<String>) {
        let pix_fmt = args.iter().position(|a| a == "-pix_fmt");
        match (self, pix_fmt) {
            (HwEncoder::Qsv, Some(i)) => args[i + 1] = "nv12".into(),
            (HwEncoder::Vaapi, _) => {
                // VAAPI encodes GPU surfaces: upload the finished frames.
                if let Some(i) = pix_fmt {
                    args.drain(i..i + 2);
                }
                if let Some(i) = args.iter().position(|a| a == "-filter_complex") {
                    let graph = &args[i + 1];
                    if let Some(cut) = graph.rfind("[out]") {
                        args[i + 1] = format!("{},format=nv12,hwupload[out]", &graph[..cut]);
                    }
                }
                args.splice(0..0, ["-vaapi_device".to_string(), VAAPI_DEVICE.to_string()]);
            }
            _ => {}
        }
    }

    /// A three-frame test encode: succeeds only if the encoder, driver and GPU all work.
    pub fn probe_args(self) -> Vec<String> {
        let mut args: Vec<String> = [
            "-hide_banner", "-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=1280x720:r=30",
            "-filter_complex", "[0:v]null[out]", "-map", "[out]",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        args.extend(self.codec_args("30"));
        args.extend(["-pix_fmt", "yuv420p", "-frames:v", "3", "-f", "null", "-"].iter().map(|s| s.to_string()));
        self.adapt_args(&mut args);
        args
    }
}

/// Encoder arguments; `videotoolbox` is the desktop macOS build.
pub fn video_codec_args(videotoolbox: bool, out_w: i64, out_h: i64, fps: &str) -> Vec<String> {
    let rate = Rational::parse(fps).map(|r| r.to_f64()).unwrap_or(30.0);
    let gop = gop_args(fps);
    let s = |v: &str| v.to_string();
    let mut args = if videotoolbox {
        if out_w > out_h {
            let base = landscape_bitrate_mbps(out_h);
            let mbps = if rate > 31.0 { pyfmt::fmt_g(base as f64 * 1.5, 6) } else { base.to_string() };
            vec![s("-c:v"), s("h264_videotoolbox"), s("-profile:v"), s("high"), s("-b:v"), format!("{mbps}M")]
        } else {
            vec![s("-c:v"), s("h264_videotoolbox"), s("-b:v"), s("8M")]
        }
    } else {
        vec![s("-c:v"), s("libx264"), s("-preset"), s(FFMPEG_PRESET), s("-crf"), FFMPEG_CRF.to_string()]
    };
    args.extend(gop);
    args
}

/// Software H.264 in the LGPL bundles, which have no libx264. macOS: Apple's own encoder
/// through VideoToolbox, at the hardware settings. Elsewhere OpenH264 (BtbN's LGPL build).
pub fn lgpl_software_codec_args(out_w: i64, out_h: i64, fps: &str) -> Vec<String> {
    let s = |v: &str| v.to_string();
    if cfg!(target_os = "macos") {
        let mut args = video_codec_args(true, out_w, out_h, fps);
        args.extend([s("-require_sw"), s("1")]);
        return args;
    }
    let mut args = OPENH264_ARGS.iter().map(|v| s(v)).collect::<Vec<_>>();
    args.extend(gop_args(fps));
    args
}

pub fn measure_loudness_args(video_path: &Path, start_ms: i64, duration_ms: i64) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let mut args = vec![
        s("-nostdin"),
        s("-nostats"),
        s("-hide_banner"),
        s("-nostats"),
        s("-ss"),
        format!("{:.3}", start_ms as f64 / 1000.0),
        s("-t"),
        format!("{:.3}", duration_ms as f64 / 1000.0),
    ];
    args.extend(MEDIA_INPUT_OPTIONS.iter().map(|v| s(v)));
    args.extend([
        s("-i"),
        video_path.to_string_lossy().into_owned(),
        s("-map"),
        s("0:a:0"),
        s("-vn"),
        s("-af"),
        format!("{AUDIO_FORMAT},{AUDIO_SYNC},loudnorm=I=-14:TP=-1.5:LRA=11:print_format=json"),
        s("-f"),
        s("null"),
        s("-"),
    ]);
    args
}

/// Append image overlays to a graph ending in [captioned]; output is [out].
/// Returns the full filter_complex and the extra input paths in input order.
pub fn compose_overlays(graph: &str, overlays: &[Overlay], video_filter: &str) -> (String, Vec<PathBuf>) {
    compose_overlays_from(graph, overlays, video_filter, 1)
}

/// `compose_overlays` with the overlay images as inputs `first_input`.. (after the video inputs).
pub fn compose_overlays_from(graph: &str, overlays: &[Overlay], video_filter: &str, first_input: usize) -> (String, Vec<PathBuf>) {
    if overlays.is_empty() {
        return (format!("{graph};[captioned]{video_filter}[out]"), Vec::new());
    }
    let mut parts = vec![graph.to_string()];
    let mut current = "captioned".to_string();
    for (index, overlay) in overlays.iter().enumerate() {
        let index = index + 1;
        let input = index - 1 + first_input;
        let label = if index == overlays.len() { "composited".to_string() } else { format!("ov{index}") };
        let mut image = format!("[{input}:v]");
        if let Some(image_filter) = &overlay.filter {
            parts.push(format!("[{input}:v]{image_filter}[img{index}]"));
            image = format!("[img{index}]");
        }
        let enable = overlay.enable.as_ref().map(|e| format!(":enable='{e}'")).unwrap_or_default();
        parts.push(format!(
            "[{current}]{image}overlay=x='{}':y='{}':shortest=1{enable}[{label}]",
            overlay.x, overlay.y
        ));
        current = label;
    }
    parts.push(format!("[composited]{video_filter}[out]"));
    (parts.join(";"), overlays.iter().map(|o| o.path.clone()).collect())
}

/// Escape a file path for use inside an FFmpeg filter argument.
pub fn escape_filter_path(path: &str) -> String {
    escape_filter_path_for(path, cfg!(windows))
}

pub fn escape_filter_path_for(path: &str, windows: bool) -> String {
    let mut escaped = path.replace('\\', "/");
    let bytes: Vec<char> = escaped.chars().collect();
    if windows && bytes.len() >= 2 && bytes[1] == ':' {
        escaped = format!("{}\\:{}", bytes[0], bytes[2..].iter().collect::<String>());
    }
    let escaped = escaped.replace('\'', "'\\''").replace('[', "\\[").replace(']', "\\]");
    format!("'{escaped}'")
}

pub(crate) fn sibling(output_path: &Path, name: &str) -> PathBuf {
    output_path.parent().map(|p| p.join(name)).unwrap_or_else(|| PathBuf::from(name))
}

pub fn compute_padded_range(padding_ms: i64, start_time_ms: i64, duration_ms: i64) -> (i64, i64) {
    if padding_ms <= 0 {
        return (start_time_ms, duration_ms);
    }
    let adjusted_start = (start_time_ms - padding_ms).max(0);
    let start_adjustment = start_time_ms - adjusted_start;
    (adjusted_start, duration_ms + start_adjustment + padding_ms)
}

/// Whether tight pacing will cut anything (it needs word timings).
fn paces(request: &RenderRequest) -> bool {
    request.pacing == TIGHT && !request.transcript_segments.is_empty()
}

/// Window intervals to keep. `plan` is None when shot content is unknown.
pub fn keep_intervals(
    request: &RenderRequest,
    plan: Option<&ClipLayoutPlan>,
    window_start_ms: i64,
    window_ms: i64,
) -> Vec<(i64, i64)> {
    if !paces(request) {
        return vec![(0, window_ms)];
    }
    let words = window_words(&request.transcript_segments, window_start_ms, window_ms);
    let protected = reaction_intervals(&request.transcript_segments, window_start_ms, window_ms);
    compute_keep_intervals(&words, window_ms, plan, &protected, request.longform)
}

/// The planner's skips in window time, clipped to the window.
pub fn window_skips(request: &RenderRequest, window_start_ms: i64, window_ms: i64) -> Vec<(i64, i64)> {
    request
        .skip_ranges_ms
        .iter()
        .filter_map(|&(start, end)| {
            let (s, e) = ((start - window_start_ms).max(0), window_ms.min(end - window_start_ms));
            (e > s).then_some((s, e))
        })
        .collect()
}

/// Chapters moved onto the edited timeline; the first starts at 0.
pub fn output_chapters(request: &RenderRequest, window_start_ms: i64, time_map: &TimeMap) -> Result<Vec<(i64, String)>> {
    let mut chapters: Vec<(i64, String)> = Vec::new();
    let total = scaled_duration_ms(time_map.output_ms, request.video_speed)?;
    for (t_ms, title) in &request.chapters {
        let out = scaled_duration_ms(time_map.to_output_clamped((t_ms - window_start_ms).max(0)), request.video_speed)?;
        if let Some(last) = chapters.last() {
            if out - last.0 < 10_000 {
                continue; // YouTube needs 10 s+ per chapter
            }
        }
        if out < total - 10_000 {
            chapters.push((out, title.clone()));
        }
    }
    if let Some(first) = chapters.first_mut() {
        first.0 = 0;
    }
    Ok(chapters)
}

// Pixel-sized caption style fields, scaled together for landscape output.
/// The preset resized for a 16:9 frame (~5% of the height, longer phrases).
pub fn landscape_caption_style(style: &CaptionStyle, out_h: i64) -> CaptionStyle {
    let mut scaled = style.clone();
    let factor = out_h as f64 / 1080.0 * 0.65;
    let scale = |v: i64| if v != 0 { 0.max(pyfmt::round(v as f64 * factor)) } else { v };
    scaled.font_size = scale(style.font_size);
    scaled.outline_width = scale(style.outline_width);
    scaled.shadow_blur = scale(style.shadow_blur);
    scaled.shadow_offset = scale(style.shadow_offset);
    scaled.shadow_spread = scale(style.shadow_spread);
    scaled.highlight_box_padding = scale(style.highlight_box_padding);
    scaled.glow_radius = scale(style.glow_radius);
    scaled.glow_blur = scale(style.glow_blur);
    scaled.line_box_padding = scale(style.line_box_padding);
    scaled.letter_spacing = scale(style.letter_spacing);
    scaled.max_words_per_line = style.max_words_per_line.max(6);
    scaled
}

/// ffprobe `avg_frame_rate` / `r_frame_rate` JSON -> output rate, or None for "30".
pub fn parse_probe_fps(stdout: &[u8]) -> Option<String> {
    let text = if stdout.is_empty() { b"{}".as_slice() } else { stdout };
    let json: serde_json::Value = serde_json::from_slice(text).ok()?;
    let stream = json.get("streams").and_then(|s| s.get(0))?;
    for key in ["avg_frame_rate", "r_frame_rate"] {
        let raw = stream.get(key).and_then(|v| v.as_str()).unwrap_or("0/0");
        let (num, den) = match raw.split_once('/') {
            Some((n, d)) => (n, d),
            None => (raw, ""),
        };
        if den.is_empty() {
            continue;
        }
        let (num, den): (i64, i64) = (num.trim().parse().ok()?, den.trim().parse().ok()?);
        if den == 0 || num == 0 {
            continue;
        }
        let rate = Rational::new(num, den)?;
        // 10 <= rate <= MAX_OUTPUT_FPS + 0.5, exactly.
        let (n, d) = (rate.num as i128, rate.den as i128);
        if 10 * d <= n && 2 * n <= (2 * MAX_OUTPUT_FPS as i128 + 1) * d {
            return Some(rate.to_string());
        }
        if n > MAX_OUTPUT_FPS as i128 * d {
            return Some(MAX_OUTPUT_FPS.to_string());
        }
    }
    None
}

/// `WxH` from ffprobe csv output, validated.
pub fn parse_dimensions(stdout: &[u8]) -> Option<(i64, i64)> {
    let text = String::from_utf8_lossy(stdout);
    let mut dims = text.trim().split('x');
    let w: i64 = dims.next()?.trim().parse().ok()?;
    let h: i64 = dims.next()?.trim().parse().ok()?;
    if w <= 0 || h <= 0 || w > u32::MAX as i64 || h > u32::MAX as i64 {
        return None;
    }
    crate::media::validate_video_dimensions(w as u32, h as u32).ok()?;
    Some((w, h))
}

/// Stream start/duration check of an export (the first half of _validate_output_timing).
pub fn check_stream_timing(stdout: &[u8], duration_ms: i64, fps: &str, with_audio: bool) -> std::result::Result<(), String> {
    let json: serde_json::Value = serde_json::from_slice(stdout).map_err(|_| "invalid probe output".to_string())?;
    let streams = json.get("streams").and_then(|s| s.as_array()).ok_or("invalid probe output")?;
    let expected = duration_ms as f64 / 1000.0;
    let rate = Rational::parse(fps).filter(|r| r.num > 0).ok_or("invalid frame rate")?;
    let kinds: &[&str] = if with_audio { &["video", "audio"] } else { &["video"] };
    for kind in kinds {
        let matches: Vec<&serde_json::Value> =
            streams.iter().filter(|s| s.get("codec_type").and_then(|v| v.as_str()) == Some(kind)).collect();
        if matches.len() != 1 {
            return Err(format!("missing or ambiguous {kind} track"));
        }
        let field = |name: &str| matches[0].get(name).and_then(pyfmt::json_float).ok_or(format!("invalid {kind} timing"));
        let start = field("start_time")?;
        let duration = field("duration")?;
        // One video frame, plus one AAC packet for audio, plus mux rounding.
        let frame = 1.0 / rate.to_f64();
        let tolerance = (if *kind == "video" { frame } else { frame + 1024.0 / 48000.0 }) + 0.003;
        if !(start.is_finite() && duration.is_finite() && duration > 0.0) {
            return Err(format!("invalid {kind} timing"));
        }
        if start.abs() > 0.003 || (start + duration - expected).abs() > tolerance {
            return Err(format!(
                "{kind} timing differs from the edit: start={start:.6}s, duration={duration:.6}s, expected={expected:.6}s, tolerance={tolerance:.6}s"
            ));
        }
    }
    Ok(())
}

/// Streaming check that AAC packets form one continuous 48 kHz clock.
#[derive(Debug, Default)]
pub struct AudioPacketChecker {
    previous: Option<f64>,
}

impl AudioPacketChecker {
    const PACKET_SECONDS: f64 = 1024.0 / 48000.0;
    const TOLERANCE: f64 = 1.0 / 48000.0 + 0.000002;

    pub fn line(&mut self, line: &[u8]) -> std::result::Result<(), String> {
        let text = String::from_utf8_lossy(line);
        let mut pts = None;
        let mut dur = None;
        for part in text.trim().split('|') {
            if let Some((k, v)) = part.split_once('=') {
                match k {
                    "pts_time" => pts = Some(v.to_string()),
                    "duration_time" => dur = Some(v.to_string()),
                    _ => {}
                }
            }
        }
        let parse = |v: Option<String>| v.as_deref().and_then(pyfmt::parse_float).ok_or("invalid AAC packet timing");
        let start = parse(pts)?;
        let duration = parse(dur)?;
        if !(start.is_finite() && duration.is_finite() && duration > 0.0 && duration <= Self::PACKET_SECONDS + Self::TOLERANCE)
        {
            return Err("invalid AAC packet duration".into());
        }
        if let Some(prev) = self.previous {
            if (start - prev - Self::PACKET_SECONDS).abs() > Self::TOLERANCE {
                return Err("discontinuous AAC packet timestamps".into());
            }
        }
        self.previous = Some(start);
        Ok(())
    }

    pub fn finish(&self, returncode: i32) -> std::result::Result<(), String> {
        if returncode != 0 || self.previous.is_none() {
            return Err("could not verify AAC packet timestamps".into());
        }
        Ok(())
    }
}

// ------------------------------------------------------------------
// Many clips
// ------------------------------------------------------------------

/// Render clips in parallel (at most `max_concurrent`), reporting progress
/// 50% -> 90% as each finishes. Results keep the input order; one failed clip
/// does not stop the others. Cancellation stops them all.
pub async fn render_clips(
    renderer: &Renderer,
    requests: &[RenderRequest],
    max_concurrent: usize,
    progress: Option<crate::types::ProgressFn>,
    cancel: &CancellationToken,
) -> Vec<Result<RenderResult>> {
    use std::sync::atomic::{AtomicU32, Ordering};
    // A semaphore and join_all rather than stream::buffered: buffered's closure makes
    // the future's Send bound higher-ranked, which tokio::spawn callers cannot satisfy.
    let total = requests.len() as u32;
    let finished = AtomicU32::new(0);
    let slots = tokio::sync::Semaphore::new(max_concurrent.max(1));
    let mut jobs = Vec::with_capacity(requests.len());
    for request in requests {
        let (finished, slots, progress) = (&finished, &slots, progress.as_ref());
        jobs.push(async move {
            let _slot = slots.acquire().await.map_err(|_| EngineError::Cancelled)?;
            let result = renderer.render_clip(request, cancel).await;
            let done = finished.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(progress) = progress {
                progress(crate::types::Progress {
                    stage: crate::types::Stage::Rendering,
                    percent: 50.0 + 40.0 * done as f64 / total.max(1) as f64,
                    step: format!("Rendered {done} of {total} clip{}", if total != 1 { "s" } else { "" }),
                    clips_done: done,
                    clips_total: total,
                });
            }
            result
        });
    }
    futures::future::join_all(jobs).await
}
