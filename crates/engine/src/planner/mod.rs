//! Clip planner: asks the configured AI provider for the most clip-worthy moments
//! of a transcript (or, for silent videos, of sampled frames) and validates them.
//!
//! Port of `services/intelligence_planner.py`, `services/visual_clip_sampling.py`
//! and the planning step of `ai_clipping_pipeline.py`.
pub mod boundaries;
pub mod config;
pub mod parse;
pub mod prompts;
pub mod visual;

#[cfg(test)]
mod tests;

pub use config::{
    calculate_optimal_clip_count, is_longform, resolve_clip_duration_bounds, ClippingMode,
    PlannerSettings, DURATION_RANGES,
};
pub use visual::VisionFrame;

use crate::error::{EngineError, Result};
use crate::llm::{self, ChatRequest, ContentPart};
use crate::types::{
    AiSettings, ClipJobRequest, ClipPlan, ClipPlanSegment, EngineConfig, PlanningCosts, Progress,
    ProgressFn, Stage, Transcript, TranscriptSegment,
};
use base64::Engine as _;
use parse::{finalize_clips, parse_clip_plan_response, ParseContext, PlanError};
use prompts::{FrameImage, UserPart};
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;
use tokio_util::sync::CancellationToken;

/// Raised when a text-only provider is asked to plan a video without speech.
pub const VISUAL_PLANNING_UNSUPPORTED: &str = "Selected planner requires a video with speech";
/// The pipeline's error when the plan comes back empty.
pub const NO_CLIPS_FOUND: &str =
    "No clip-worthy moments found (visual evidence may be insufficient, \
or the selected time range is too short for the chosen clip length)";
/// The error when a campaign brief was set and no moment satisfied it.
pub const NO_CLIPS_MATCH_BRIEF: &str = "No moments match the campaign brief";

/// At most this many frames go to the planner.
const MAX_FRAMES_SENT: usize = 48;

/// Plans clips for one job. Silent videos (empty transcript) fall back to
/// planning from sampled frames. Fails with [`NO_CLIPS_FOUND`] when nothing qualifies.
pub async fn plan_clips(
    transcript: &Transcript,
    req: &ClipJobRequest,
    cfg: &EngineConfig,
    video: &Path,
    duration_seconds: f64,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> Result<ClipPlan> {
    let settings = PlannerSettings::for_job(&cfg.ai, req.requirements.as_deref());
    plan_clips_with(
        &settings,
        transcript,
        req,
        cfg,
        video,
        duration_seconds,
        progress,
        cancel,
    )
    .await
}

fn report(progress: &ProgressFn, percent: f64, step: &str) {
    progress(Progress {
        stage: Stage::Planning,
        percent,
        step: step.into(),
        clips_done: 0,
        clips_total: 0,
    });
}

#[allow(clippy::too_many_arguments)]
pub async fn plan_clips_with(
    settings: &PlannerSettings,
    transcript: &Transcript,
    req: &ClipJobRequest,
    cfg: &EngineConfig,
    video: &Path,
    duration_seconds: f64,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> Result<ClipPlan> {
    // An end past the video is clamped to the video end.
    let end_time = req.end_time_seconds.map(|e| {
        if e > duration_seconds {
            duration_seconds
        } else {
            e
        }
    });
    let mut frames = Vec::new();
    if transcript.segments.is_empty() {
        report(progress, 25.0, "Analyzing video frames...");
        frames = visual::sample_visual_planning_frames(
            &cfg.ffmpeg,
            video,
            duration_seconds,
            &cfg.work_dir,
            req.start_time_seconds,
            end_time,
            cancel,
        )
        .await;
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        tracing::info!("Visual-only planning has {} sampled frames", frames.len());
        if !visual::has_visual_change(&cfg.ffmpeg, &frames, cancel).await {
            tracing::info!("Visual-only planning skipped: insufficient visible change");
            frames.clear();
        }
    }
    report(progress, 30.0, "Planning viral clips...");
    let input = PlanInput {
        transcript: &transcript.segments,
        video_duration: Some(duration_seconds),
        max_clips: req.max_clips.map(i64::from),
        auto_clip_count: req.auto_clip_count,
        min_duration_seconds: None,
        max_duration_seconds: None,
        duration_ranges: req.duration_ranges.as_deref(),
        frames: &frames,
        start_time_seconds: req.start_time_seconds,
        end_time_seconds: end_time,
        aspect_ratio: &req.aspect_ratio,
        drop_outside_range: false,
    };
    let plan = plan_in_parts(settings, &cfg.ai, &input, progress, cancel).await?;
    tracing::info!("Planned {} clips", plan.segments.len());
    if plan.segments.is_empty() {
        let brief = !settings.clip_requirements.trim().is_empty();
        return Err(EngineError::msg(if brief { NO_CLIPS_MATCH_BRIEF } else { NO_CLIPS_FOUND }));
    }
    Ok(ClipPlan {
        clips: plan.segments,
        costs: plan.costs.unwrap_or_default(),
        planning_source: if frames.is_empty() {
            "transcript"
        } else {
            "visual"
        }
        .into(),
        insights: plan.insights,
        visual_frame_count: frames.len() as u32,
    })
}

/// A transcript range longer than this is planned in parallel parts, so no single
/// prompt carries a whole 3-4 h livestream (context limits, CLI timeouts, and models
/// skimming the middle of very long prompts).
const SPLIT_OVER_SECONDS: f64 = 75.0 * 60.0;
/// Target length of one part. ponytail: fixed 1 h; shrink it if small-context local
/// models still overflow.
const PART_SECONDS: f64 = 3600.0;
/// Each part asks for this many times its share of clips, so the merge has spares to rank.
const PART_CLIP_SURPLUS: f64 = 1.5;

/// (start, end) seconds of each planning part, or None to plan the range in one call.
/// Parts are equal (no short tail), each extended by the longest clip into the next so a
/// clip crossing a seam is still whole in one part.
fn planning_parts(start: f64, end: f64, max_clip_seconds: i64) -> Option<Vec<(f64, f64)>> {
    let span = end - start;
    // A NaN span gives n = 0 below, so it also stays one call.
    if span <= SPLIT_OVER_SECONDS {
        return None;
    }
    let overlap = max_clip_seconds as f64;
    let n = (span / PART_SECONDS.max(3.0 * overlap)).ceil() as usize;
    if n < 2 {
        return None;
    }
    let step = span / n as f64;
    Some(
        (0..n)
            .map(|i| {
                let s = start + step * i as f64;
                (s, (s + step + overlap).min(end))
            })
            .collect(),
    )
}

/// [`plan`], split into parallel ~1 h parts for long transcripts and merged by score.
pub async fn plan_in_parts(
    settings: &PlannerSettings,
    ai: &AiSettings,
    input: &PlanInput<'_>,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> Result<PlanResponse> {
    use futures::StreamExt;

    let start = input.start_time_seconds.unwrap_or(0.0).max(0.0);
    let end = input.end_time_seconds.filter(|e| *e != 0.0).or_else(|| {
        input
            .video_duration
            .filter(|d| *d > 0.0)
            .or_else(|| input.transcript.last().map(|s| s.end_time_ms as f64 / 1000.0))
    });
    let (min_clip, max_clip) = resolve_clip_duration_bounds(
        input.duration_ranges,
        input.min_duration_seconds,
        input.max_duration_seconds,
    );
    let parts = match end {
        Some(end) if !input.transcript.is_empty() => planning_parts(start, end, max_clip),
        _ => None,
    };
    let (Some(parts), Some(end)) = (parts, end) else {
        return plan(settings, ai, input, cancel).await;
    };

    // The whole range's clip count, as one call would have picked it.
    let span = end - start;
    let (start_ms, end_ms) = ((start * 1000.0) as i64, (end * 1000.0) as i64);
    let in_range: Vec<TranscriptSegment> = input
        .transcript
        .iter()
        .filter(|s| s.start_time_ms >= start_ms && s.end_time_ms <= end_ms)
        .cloned()
        .collect();
    let words = count_words(&in_range);
    let wpm = (words > 0).then(|| words as f64 / (span / 60.0));
    let user_max = input.max_clips.filter(|m| *m != 0).unwrap_or(config::MAX_SUGGESTED_CLIPS);
    let total = calculate_optimal_clip_count(span, Some(user_max), input.auto_clip_count, wpm)
        .min(1i64.max((span / min_clip as f64).floor() as i64));
    let n = parts.len();
    let per_part = 1i64.max((total as f64 / n as f64 * PART_CLIP_SURPLUS).ceil() as i64);
    tracing::info!("Planning {span:.0}s in {n} parallel parts ({per_part} clips each, {total} kept)");

    // A local server runs one model at a time; hosted APIs and CLIs take a few at once.
    let concurrency = if ai.provider == "local" { 1 } else { 3 };
    let step = |done: usize| format!("Planning clips · {done} of {n} parts done");
    report(progress, 30.0, &step(0));
    let mut results = futures::stream::iter(parts.into_iter().map(|(s, e)| {
        let part = PlanInput {
            start_time_seconds: Some(s),
            end_time_seconds: Some(e),
            // per_part is a ceiling; "Let AI decide" still leaves the number out of the prompt.
            max_clips: Some(per_part),
            drop_outside_range: true,
            ..input.clone()
        };
        async move { plan(settings, ai, &part, cancel).await }
    }))
    .buffer_unordered(concurrency);

    let (mut done, mut succeeded) = (0, 0);
    let mut segments = Vec::new();
    let mut insights = Vec::new();
    let mut costs: Option<PlanningCosts> = None;
    let mut first_error = None;
    while let Some(result) = results.next().await {
        done += 1;
        // Rendering starts at 50%, so planning fills 30 -> 50.
        report(progress, 30.0 + 20.0 * done as f64 / n as f64, &step(done));
        match result {
            Ok(part) => {
                succeeded += 1;
                segments.extend(part.segments);
                insights.extend(part.insights);
                if let Some(p) = part.costs {
                    let c = costs.get_or_insert_with(|| PlanningCosts {
                        provider: p.provider.clone(),
                        model: p.model.clone(),
                        ..Default::default()
                    });
                    c.prompt_tokens += p.prompt_tokens;
                    c.completion_tokens += p.completion_tokens;
                    c.estimated_cost_usd = parse::round_to(c.estimated_cost_usd + p.estimated_cost_usd, 6);
                    c.attempts += p.attempts;
                    c.cost_incomplete |= p.cost_incomplete;
                }
            }
            Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
            // Other parts' clips still make a usable plan.
            Err(e) => {
                tracing::warn!("A planning part failed, skipping it: {e}");
                first_error.get_or_insert(e);
            }
        }
    }
    if succeeded == 0 {
        if let Some(e) = first_error {
            return Err(e);
        }
    }
    Ok(PlanResponse {
        // Best first; drops the copies of a moment two overlapping parts both picked.
        segments: finalize_clips(segments, total),
        insights: (!insights.is_empty()).then(|| insights.join("\n\n")),
        costs,
    })
}

/// Inputs of one planning call (IntelligencePlannerService.plan_clips arguments).
#[derive(Debug, Clone, Default)]
pub struct PlanInput<'a> {
    pub transcript: &'a [TranscriptSegment],
    /// Source duration from metadata, when known.
    pub video_duration: Option<f64>,
    pub max_clips: Option<i64>,
    pub auto_clip_count: bool,
    pub min_duration_seconds: Option<i64>,
    pub max_duration_seconds: Option<i64>,
    pub duration_ranges: Option<&'a [String]>,
    pub frames: &'a [VisionFrame],
    pub start_time_seconds: Option<f64>,
    pub end_time_seconds: Option<f64>,
    pub aspect_ratio: &'a str,
    /// Set on the parts of a split plan: clips outside the part's range are dropped.
    pub drop_outside_range: bool,
}

/// Planner result; an empty `segments` with `insights` explains a skipped plan.
#[derive(Debug, Clone, Default)]
pub struct PlanResponse {
    pub segments: Vec<ClipPlanSegment>,
    pub insights: Option<String>,
    pub costs: Option<PlanningCosts>,
}

impl PlanResponse {
    fn empty(insights: impl Into<String>) -> Self {
        Self {
            segments: vec![],
            insights: Some(insights.into()),
            costs: None,
        }
    }
}

fn count_words(transcript: &[TranscriptSegment]) -> usize {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\b\w+\b").unwrap());
    transcript
        .iter()
        .map(|s| re.find_iter(&s.text).count())
        .sum()
}

/// (effective duration seconds, source label) used for clip count scaling.
fn resolve_effective_duration(
    input: &PlanInput,
    transcript: &[TranscriptSegment],
) -> (f64, &'static str) {
    let metadata = input.video_duration.filter(|d| *d > 0.0);
    if input.start_time_seconds.is_some() || input.end_time_seconds.is_some() {
        let start = input.start_time_seconds.unwrap_or(0.0).max(0.0);
        let end = input.end_time_seconds.unwrap_or_else(|| {
            metadata.unwrap_or_else(|| {
                transcript
                    .last()
                    .map(|s| s.end_time_ms as f64 / 1000.0)
                    .unwrap_or(start)
            })
        });
        return ((end - start).max(0.0), "time_range");
    }
    if let Some(d) = metadata {
        return (d, "metadata");
    }
    if let (Some(first), Some(last)) = (transcript.first(), transcript.last()) {
        return (
            ((last.end_time_ms - first.start_time_ms) as f64 / 1000.0).max(0.0),
            "transcript",
        );
    }
    (0.0, "fallback")
}

fn load_frames(frames: &[VisionFrame]) -> Vec<FrameImage> {
    frames
        .iter()
        .filter_map(|frame| match std::fs::read(&frame.path) {
            Ok(data) => {
                let ext = frame
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let mime = match ext.as_str() {
                    "png" => "image/png",
                    "webp" => "image/webp",
                    "gif" => "image/gif",
                    _ => "image/jpeg",
                };
                Some(FrameImage {
                    timestamp_ms: frame.timestamp_ms,
                    mime: mime.into(),
                    base64: base64::engine::general_purpose::STANDARD.encode(data),
                })
            }
            Err(_) => {
                tracing::warn!("Failed to load a planning frame");
                None
            }
        })
        .collect()
}

/// Everything sent to the model, before provider formatting.
#[derive(Debug, Clone)]
pub struct PlannerMessages {
    pub system: String,
    pub user: Vec<UserPart>,
    pub frames: Vec<FrameImage>,
}

/// The chat request for one planning attempt.
pub fn build_request(
    settings: &PlannerSettings,
    messages: &PlannerMessages,
    longform: bool,
) -> ChatRequest {
    let user = messages
        .user
        .iter()
        .map(|part| match part {
            UserPart::Text(t) => ContentPart::Text(t.clone()),
            UserPart::Image(i) => {
                let f = &messages.frames[*i];
                ContentPart::Image {
                    mime: f.mime.clone(),
                    base64: f.base64.clone(),
                }
            }
        })
        .collect();
    let advanced = settings.clipping_mode == ClippingMode::Advanced;
    ChatRequest {
        model: settings.planner_model.clone(),
        fallback_models: settings.planner_fallback_models(),
        system: Some(messages.system.clone()),
        user,
        max_tokens: settings.planner_max_output_tokens,
        schema: Some(("clip_plan".into(), prompts::clip_plan_schema(longform))),
        // Advanced accepts models without configurable reasoning: use their defaults.
        reasoning_effort: (!advanced).then(|| settings.planner_reasoning_effort.clone()),
        temperature: None,
        response_healing: true,
        require_parameters: true,
        image_detail: Some("low".into()),
        base_url: settings.base_url.clone(),
    }
}

/// Plan clips (IntelligencePlannerService.plan_clips).
pub async fn plan(
    settings: &PlannerSettings,
    ai: &AiSettings,
    input: &PlanInput<'_>,
    cancel: &CancellationToken,
) -> Result<PlanResponse> {
    let mut transcript: Vec<TranscriptSegment> = input.transcript.to_vec();
    let mut frames: Vec<VisionFrame> = input.frames.to_vec();
    if transcript.is_empty() && frames.is_empty() {
        tracing::warn!("No transcript or frames provided for clip planning");
        return Ok(PlanResponse::empty("No content provided for analysis"));
    }
    let (start_limit, end_limit) = (input.start_time_seconds, input.end_time_seconds);
    if start_limit.is_some() || end_limit.is_some() {
        let start_ms = (start_limit.unwrap_or(0.0) * 1000.0) as i64;
        // Python `(end or inf)`: an end of 0 means "no end".
        let end_ms = end_limit
            .filter(|e| *e != 0.0)
            .map(|e| (e * 1000.0) as i64)
            .unwrap_or(i64::MAX);
        transcript.retain(|s| s.start_time_ms >= start_ms && s.end_time_ms <= end_ms);
        frames.retain(|f| f.timestamp_ms >= start_ms && f.timestamp_ms <= end_ms);
    }
    if transcript.is_empty() && frames.len() < 3 {
        return Ok(PlanResponse::empty(
            "Visual-only planning needs at least three sampled frames",
        ));
    }

    let (effective_duration, duration_source) = resolve_effective_duration(input, &transcript);
    let word_count = count_words(&transcript);
    let words_per_minute = (effective_duration > 0.0 && word_count > 0)
        .then(|| word_count as f64 / (effective_duration / 60.0));
    let user_max_clips = input
        .max_clips
        .filter(|m| *m != 0)
        .unwrap_or(config::MAX_SUGGESTED_CLIPS);
    let mut clip_count = calculate_optimal_clip_count(
        effective_duration,
        Some(user_max_clips),
        input.auto_clip_count,
        words_per_minute,
    );

    // One set of bounds for the prompt, the parser and the end-snap cap.
    let (min_duration, max_duration) = resolve_clip_duration_bounds(
        input.duration_ranges,
        input.min_duration_seconds,
        input.max_duration_seconds,
    );
    // Clips can't overlap, so the range only fits so many of the minimum length.
    if effective_duration > 0.0 {
        let max_fit = 1i64.max((effective_duration / min_duration as f64).floor() as i64);
        if clip_count > max_fit {
            tracing::info!("Clip count capped {clip_count} -> {max_fit}");
            clip_count = max_fit;
        }
    }
    let longform = !transcript.is_empty() && is_longform(input.aspect_ratio, min_duration);
    // A range shorter than the shortest allowed clip can't yield a clip.
    if duration_source != "fallback" && effective_duration < min_duration as f64 {
        let reason = format!(
            "Selected range ({effective_duration:.0}s) is shorter than the minimum clip length ({min_duration}s)"
        );
        tracing::warn!("{reason}; skipping clip planning");
        return Ok(PlanResponse::empty(reason));
    }

    let mut system = if longform {
        prompts::build_longform_system_prompt(clip_count, min_duration, max_duration)
    } else if !transcript.is_empty() {
        // "Let AI decide": no number in the prompt; clip_count stays a silent ceiling.
        prompts::build_system_prompt(
            (!input.auto_clip_count).then_some(clip_count),
            Some(min_duration),
            Some(max_duration),
            input.duration_ranges,
        )
    } else {
        prompts::build_visual_only_system_prompt(
            clip_count,
            Some(min_duration),
            Some(max_duration),
            input.duration_ranges,
        )
    };
    if !settings.clip_requirements.trim().is_empty() {
        system.push_str(&prompts::campaign_brief_section(
            &settings.clip_requirements,
        ));
    }
    let transcript_text = prompts::build_transcript_text(&transcript);

    let mut frames_to_send: Vec<VisionFrame> =
        frames.iter().take(MAX_FRAMES_SENT).cloned().collect();
    if !settings.planner_supports_images {
        if transcript.is_empty() {
            return Err(EngineError::msg(VISUAL_PLANNING_UNSUPPORTED));
        }
        frames_to_send.clear();
    }
    let frame_images = load_frames(&frames_to_send);
    if transcript.is_empty() && frame_images.len() < 3 {
        return Ok(PlanResponse::empty(
            "Visual-only planning could not load enough sampled frames",
        ));
    }
    let mut visual_frame_times: Vec<f64> = if transcript.is_empty() {
        frame_images
            .iter()
            .map(|f| f.timestamp_ms as f64 / 1000.0)
            .collect()
    } else {
        vec![]
    };
    visual_frame_times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if transcript.is_empty() && !visual_frame_times.windows(2).any(|w| w[1] - w[0] <= 45.0) {
        return Ok(PlanResponse::empty(
            "Sampled visual frames are too far apart to support a short clip",
        ));
    }

    let video_duration = input.video_duration;
    let duration_for_prompt = video_duration
        .filter(|d| *d != 0.0)
        .unwrap_or(effective_duration);
    let user = prompts::build_user_parts(
        &transcript_text,
        &frame_images,
        clip_count,
        !input.auto_clip_count,
        &transcript,
        duration_for_prompt,
        longform,
    );
    let messages = PlannerMessages {
        system,
        user,
        frames: frame_images,
    };
    let request = build_request(settings, &messages, longform);
    let ctx = ParseContext {
        min_duration: Some(min_duration),
        max_duration: Some(max_duration),
        duration_ranges: input.duration_ranges,
        start_limit,
        end_limit,
        transcript: &transcript,
        video_duration,
        longform,
        visual_frame_times: &visual_frame_times,
        drop_outside_range: input.drop_outside_range,
    };
    tracing::info!(
        "Calling planner model {} (fallbacks: {:?}) for clip planning ({} transcript segments, {} frames, {} clips)",
        request.model,
        request.fallback_models,
        transcript.len(),
        messages.frames.len(),
        clip_count
    );

    const MAX_ATTEMPTS: u32 = 3;
    let (mut prompt_tokens, mut completion_tokens, mut cost) = (0u64, 0u64, 0.0f64);
    let mut cost_incomplete = false;
    let mut served_by = request.model.clone();
    let mut attempts = 0;
    for attempt in 0..MAX_ATTEMPTS {
        attempts += 1;
        let outcome: std::result::Result<parse::ParsedPlan, PlanError> =
            match llm::chat(ai, &request, cancel).await {
                Err(e) if e.cancelled => return Err(EngineError::Cancelled),
                Err(e) => Err(PlanError {
                    message: e.message,
                    retryable: e.retryable,
                }),
                Ok(reply) => {
                    served_by = if reply.model.is_empty() {
                        request.model.clone()
                    } else {
                        reply.model.clone()
                    };
                    prompt_tokens += reply.prompt_tokens;
                    completion_tokens += reply.completion_tokens;
                    match reply.cost {
                        Some(c) => cost += c,
                        None => {
                            let mut pricing = Some(llm::model_pricing(&served_by));
                            if settings.clipping_mode == ClippingMode::Advanced {
                                pricing = settings
                                    .planner_input_price
                                    .zip(settings.planner_output_price);
                            }
                            match pricing {
                                // Estimate only this response; keep reported charges from other attempts.
                                Some((input, output)) => {
                                    cost += reply.prompt_tokens as f64 * input
                                        + reply.completion_tokens as f64 * output
                                }
                                None => cost_incomplete = true,
                            }
                        }
                    }
                    parse_clip_plan_response(
                        reply.text.as_deref(),
                        reply.finish_reason.as_deref(),
                        &ctx,
                        settings,
                    )
                }
            };
        match outcome {
            Ok(parsed) => {
                let segments = finalize_clips(parsed.segments, clip_count);
                let costs = PlanningCosts {
                    provider: settings.ai_provider.clone(),
                    model: served_by.clone(),
                    prompt_tokens,
                    completion_tokens,
                    estimated_cost_usd: parse::round_to(cost, 6),
                    attempts,
                    cost_incomplete,
                };
                tracing::info!(
                    "Planning API cost: ${cost:.6} ({} tokens, {attempts} attempt(s), model={served_by})",
                    prompt_tokens + completion_tokens
                );
                return Ok(PlanResponse {
                    segments,
                    insights: parsed.insights,
                    costs: Some(costs),
                });
            }
            Err(e) => {
                if !e.retryable || attempt == MAX_ATTEMPTS - 1 {
                    tracing::error!(
                        "Clip planning failed after {attempts} attempt(s): {}",
                        e.message
                    );
                    return Err(EngineError::msg(e.message));
                }
                let delay = settings.retry_base_delay * 2u32.pow(attempt);
                tracing::warn!(
                    "Clip planning attempt {attempts} failed ({}); retrying in {delay:?}",
                    e.message
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = cancel.cancelled() => return Err(EngineError::Cancelled),
                }
            }
        }
    }
    unreachable!("the last attempt returns")
}
