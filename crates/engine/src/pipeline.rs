//! Orchestration: download -> transcribe -> plan -> render -> result.json.
//! Port of ai_clipping_pipeline.py (local mode) and bridge_runner.py.
use crate::download;
use crate::error::{EngineError, Result};
use crate::layout;
use crate::llm;
use crate::planner;
use crate::render::{self, RenderRequest, Renderer};
use crate::transcribe;
use crate::types::{ClipJobRequest, ClipPlanSegment, EngineConfig, Progress, ProgressFn, Stage};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// Name of the manifest the app reads back (the Electron engine called it job_output.json).
pub const RESULT_FILE: &str = "result.json";

/// Runs one job and returns the JobOutput JSON (src/shared/job-output.ts), also saved as
/// `result.json` in `cfg.output_dir`. Clips, transcript.json and plan.json land there too;
/// `cfg.work_dir` holds the download and intermediates and is removed by the caller.
pub async fn run_job(
    req: ClipJobRequest,
    cfg: EngineConfig,
    progress: ProgressFn,
    cancel: CancellationToken,
) -> Result<serde_json::Value> {
    let mut job = Job { req, cfg, progress, cancel, saved: None, started: Instant::now(), stages: BTreeMap::new() };
    match job.run().await {
        Ok(output) => Ok(output),
        Err(EngineError::Cancelled) => Err(EngineError::Cancelled),
        // Clips and a manifest were already saved: optional bookkeeping must not fail the run.
        Err(e) => match job.saved.take() {
            Some(saved) => {
                tracing::warn!("Local clips saved; final bookkeeping failed ({e})");
                Ok(saved)
            }
            None => Err(e),
        },
    }
}

struct Job {
    req: ClipJobRequest,
    cfg: EngineConfig,
    progress: ProgressFn,
    cancel: CancellationToken,
    saved: Option<Value>,
    started: Instant,
    stages: BTreeMap<&'static str, f64>,
}

impl Job {
    fn report(&self, stage: Stage, percent: f64, step: &str, clips_done: u32, clips_total: u32) {
        (self.progress)(Progress { stage, percent, step: step.into(), clips_done, clips_total });
    }

    fn check_cancel(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        Ok(())
    }

    async fn run(&mut self) -> Result<Value> {
        self.report(Stage::Pending, 0.0, "Starting pipeline...", 0, 0);
        std::fs::create_dir_all(&self.cfg.work_dir)?;
        std::fs::create_dir_all(&self.cfg.output_dir)?;
        let job_id = self
            .cfg
            .output_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        // 1. Download
        self.report(Stage::Downloading, 5.0, "Downloading video...", 0, 0);
        let t = Instant::now();
        let source = download::download(&self.req.video_url, &self.cfg, None, &self.progress, &self.cancel).await?;
        self.stages.insert("download", t.elapsed().as_secs_f64());
        let meta = source.metadata.clone();
        let video_duration = meta.duration_seconds;
        if self.req.end_time_seconds.is_some_and(|end| end > video_duration) {
            self.req.end_time_seconds = Some(video_duration);
        }
        self.check_cancel()?;

        // 2. Transcribe (no audio track -> empty transcript and visual-only planning)
        self.report(Stage::Transcribing, 15.0, "Transcribing audio...", 0, 0);
        let t = Instant::now();
        let range = match (self.req.start_time_seconds, self.req.end_time_seconds) {
            (None, None) => None,
            (start, end) => Some((start.unwrap_or(0.0), end.unwrap_or(f64::INFINITY))),
        };
        let transcript = transcribe::transcribe(&source.path, &self.cfg, range, &self.progress, &self.cancel).await?;
        self.stages.insert("transcription", t.elapsed().as_secs_f64());
        let captions_available = !transcript.segments.is_empty();
        let transcription_status = if captions_available { "available" } else { "no_speech" };
        let transcript_path = save_json(
            &self.cfg.output_dir,
            "transcript",
            &json!({
                "segments": transcript.segments,
                "full_text": transcript.full_text,
                "language": transcript.language,
                "status": transcription_status,
                "captions_available": captions_available,
            }),
        )?;
        self.check_cancel()?;

        // 3. Plan (the planner samples frames itself when there is no speech)
        let t = Instant::now();
        let mut plan =
            planner::plan_clips(&transcript, &self.req, &self.cfg, &source.path, video_duration, &self.progress, &self.cancel)
                .await?;
        self.stages.insert("planning", t.elapsed().as_secs_f64());
        if plan.clips.is_empty() {
            return Err(EngineError::msg(
                "No clip-worthy moments found (visual evidence may be insufficient, \
                 or the selected time range is too short for the chosen clip length)",
            ));
        }
        let target_platform = if self.req.aspect_ratio == "16:9" { "youtube" } else { "tiktok" };
        let plan_path = save_json(
            &self.cfg.output_dir,
            "plan",
            &json!({
                "segments": plan.clips,
                "total_clips": plan.clips.len(),
                "target_platform": target_platform,
                "insights": plan.insights,
                "planning_source": plan.planning_source,
            }),
        )?;
        self.check_cancel()?;

        // 4. Render (parallel; one failed clip does not sink the job)
        let total = plan.clips.len() as u32;
        let plural = if total == 1 { "" } else { "s" };
        self.report(Stage::Rendering, 50.0, &format!("Rendering {total} clip{plural}..."), 0, total);
        let (min_s, _) = planner::resolve_clip_duration_bounds(self.req.duration_ranges.as_deref(), None, None);
        let longform = planner::is_longform(&self.req.aspect_ratio, min_s);
        let clips_dir = self.cfg.work_dir.join("clips");
        std::fs::create_dir_all(&clips_dir)?;
        let requests: Vec<RenderRequest> = plan
            .clips
            .iter()
            .enumerate()
            .map(|(i, segment)| {
                RenderRequest::for_segment(
                    &self.req,
                    segment,
                    &transcript.segments,
                    captions_available,
                    longform,
                    &source.path,
                    &clips_dir.join(format!("clip_{i:02}.mp4")),
                    meta.width,
                    meta.height,
                )
            })
            .collect();
        let renderer = Renderer::new(&self.cfg);
        let t = Instant::now();
        let results = render::render_clips(
            &renderer,
            &requests,
            render::max_concurrent_renders(),
            Some(self.progress.clone()),
            &self.cancel,
        )
        .await;
        self.stages.insert("rendering", t.elapsed().as_secs_f64());
        self.check_cancel()?;

        let mut rendered = Vec::new();
        let mut clip_layouts = Vec::new();
        let mut layout_vision_cost = 0.0;
        let mut first_error = None;
        let mut failed = 0usize;
        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(result) => {
                    let segment = &mut plan.clips[i];
                    result.apply_to(segment);
                    layout_vision_cost += result.layout_cost_usd;
                    clip_layouts.push(result.clip_layout_entry(rendered.len(), &requests[i]));
                    rendered.push((result, segment.clone()));
                }
                Err(e) => {
                    tracing::error!("Clip {} failed to render, skipping it: {e}", i + 1);
                    failed += 1;
                    first_error.get_or_insert(e);
                }
            }
        }
        if rendered.is_empty() {
            return Err(first_error.unwrap_or_else(|| EngineError::msg("video render failed")));
        }

        // 5. Save clips into the run folder and commit a usable manifest first.
        self.report(Stage::Rendering, 90.0, "Saving clips locally...", total, total);
        let t = Instant::now();
        let clips = save_clips(&self.cfg.output_dir, &rendered)?;
        self.stages.insert("local_save", t.elapsed().as_secs_f64());
        let file_url = |p: &Path| Value::from(format!("file://{}", p.display()));
        let mut output = json!({
            "job_id": job_id,
            "source_video_url": self.req.video_url,
            "source_video_title": meta.title,
            "source_video_duration_seconds": video_duration,
            "total_clips": clips.len(),
            "clips": clips,
            "user_id": null,
            "transcript_url": file_url(&transcript_path),
            "plan_url": file_url(&plan_path),
            "processing_time_seconds": self.started.elapsed().as_secs_f64(),
            "metrics": {
                "planned_clip_count": plan.clips.len(),
                "rendered_clip_count": rendered.len(),
                "failed_clip_count": failed,
                "uploaded_clip_count": rendered.len(),
            },
        });
        save_json(&self.cfg.output_dir, "result", &output)?;
        self.saved = Some(output.clone());

        // Metrics and API costs.
        let mut api_costs = Map::new();
        let pc = &plan.costs;
        api_costs.insert(
            "planning".into(),
            json!({
                "provider": pc.provider,
                "model": pc.model,
                "prompt_tokens": pc.prompt_tokens,
                "completion_tokens": pc.completion_tokens,
                "total_tokens": pc.prompt_tokens + pc.completion_tokens,
                "estimated_cost_usd": pc.estimated_cost_usd,
                "attempts": pc.attempts,
                "cost_incomplete": pc.cost_incomplete,
            }),
        );
        let mut total_cost = pc.estimated_cost_usd;
        if layout_vision_cost > 0.0 {
            api_costs.insert(
                "layout_vision".into(),
                json!({
                    "provider": self.cfg.ai.provider,
                    "model": planner_model(&self.cfg),
                    "estimated_cost_usd": round6(layout_vision_cost),
                }),
            );
            total_cost += layout_vision_cost;
        }
        let incomplete = api_costs.values().any(|v| v["cost_incomplete"].as_bool().unwrap_or(false));
        api_costs.insert("total_estimated_cost_usd".into(), json!(round6(total_cost)));
        api_costs.insert("cost_incomplete".into(), json!(incomplete));

        let analysis_end = self.req.end_time_seconds.unwrap_or(video_duration);
        let rendered_output_bytes: u64 = rendered.iter().map(|(r, _)| r.file_size_bytes).sum();
        let source_size = std::fs::metadata(&source.path).map(|m| m.len()).unwrap_or(0);
        let metrics = json!({
            "analysis_duration_seconds": (analysis_end - self.req.start_time_seconds.unwrap_or(0.0)).max(0.0),
            "requested_settings": {
                "ai_provider": self.cfg.ai.provider,
                "planner_model": planner_model(&self.cfg),
                "transcription_model": transcribe::whisper_model_label(&self.cfg.whisper_model_path),
                "aspect_ratio": self.req.aspect_ratio,
                "layout_style": self.req.layout_style,
                "layout_vision_enabled": self.req.layout_vision && llm::supports_images(&self.cfg.ai.provider),
                "pacing": self.req.pacing,
                "video_speed": self.req.video_speed,
            },
            "transcription_status": transcription_status,
            "planning_source": plan.planning_source,
            "visual_frame_count": plan.visual_frame_count,
            "captions_status": if !captions_available {
                "unavailable_without_transcript"
            } else if self.req.include_captions {
                "enabled"
            } else {
                "disabled_by_request"
            },
            "stage_durations_seconds": self.stages.iter().map(|(k, v)| (k.to_string(), json!((v * 1000.0).round() / 1000.0))).collect::<Map<_, _>>(),
            "planned_clip_count": plan.clips.len(),
            "rendered_clip_count": rendered.len(),
            "failed_clip_count": failed,
            "uploaded_clip_count": rendered.len(),
            "source_video_size_bytes": source_size,
            "rendered_output_bytes": rendered_output_bytes,
            "clip_layouts": clip_layouts,
            "smart_framing_available": layout::layout_available(&self.cfg),
            "api_costs": api_costs,
        });
        output["metrics"] = metrics;
        output["processing_time_seconds"] = json!(self.started.elapsed().as_secs_f64());
        save_json(&self.cfg.output_dir, "result", &output)?;
        self.report(Stage::Rendering, 100.0, "Processing complete!", total, total);
        Ok(output)
    }
}

fn planner_model(cfg: &EngineConfig) -> String {
    if !cfg.ai.model.is_empty() {
        return cfg.ai.model.clone();
    }
    match cfg.ai.provider.as_str() {
        "openrouter" => llm::DEFAULT_OPENROUTER_MODEL.into(),
        _ => "provider default".into(),
    }
}

fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

/// Writes `<dir>/<name>.json` atomically (temp file + fsync + rename).
fn save_json(dir: &Path, name: &str, data: &Value) -> Result<PathBuf> {
    use std::io::Write;
    let path = dir.join(format!("{name}.json"));
    let mut tmp = tempfile::Builder::new().prefix(&format!(".{name}.")).suffix(".tmp").tempfile_in(dir)?;
    let bytes = serde_json::to_vec_pretty(data).map_err(|_| EngineError::msg("could not encode json"))?;
    tmp.write_all(&bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(&path).map_err(|e| EngineError::from(e.error))?;
    Ok(path)
}

/// Links (or copies across volumes) rendered clips into the run folder as clip_NN.mp4,
/// with .srt sidecars and YouTube upload notes, and returns the ClipArtifact list.
fn save_clips(out_dir: &Path, rendered: &[(render::RenderResult, ClipPlanSegment)]) -> Result<Vec<Value>> {
    let mut clips = Vec::with_capacity(rendered.len());
    for (i, (result, segment)) in rendered.iter().enumerate() {
        let dest = out_dir.join(format!("clip_{i:02}.mp4"));
        let _ = std::fs::remove_file(&dest);
        if std::fs::hard_link(&result.output_path, &dest).is_err() {
            std::fs::copy(&result.output_path, &dest)?;
        }
        let subtitle_url = match &segment.subtitle_path {
            Some(srt) if Path::new(srt).is_file() => {
                let srt_dest = out_dir.join(format!("clip_{i:02}.srt"));
                std::fs::copy(srt, &srt_dest)?;
                Value::from(format!("file://{}", srt_dest.display()))
            }
            _ => Value::Null,
        };
        if segment.description.is_some() || !segment.output_chapters.is_empty() {
            write_upload_notes(&out_dir.join(format!("clip_{i:02}.youtube.txt")), segment);
        }
        clips.push(json!({
            "clip_index": i,
            "s3_url": format!("file://{}", dest.display()),
            "duration_ms": result.duration_ms,
            "start_time_ms": segment.start_time_ms,
            "end_time_ms": segment.end_time_ms,
            "virality_score": segment.virality_score,
            "layout_type": segment.layout_type,
            "summary": segment.summary,
            "tags": segment.tags,
            "render_fallback": segment.render_fallback,
            "description": segment.description,
            "chapters": chapter_dicts(segment),
            "subtitle_url": subtitle_url,
        }));
    }
    Ok(clips)
}

/// Chapters on the clip's timeline, or null when YouTube would reject them (fewer than three).
fn chapter_dicts(segment: &ClipPlanSegment) -> Value {
    if segment.output_chapters.len() < 3 {
        return Value::Null;
    }
    segment.output_chapters.iter().map(|(t, title)| json!({ "time_ms": t, "title": title })).collect()
}

fn stamp(ms: i64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Title, description, chapters and tags ready to paste into an upload.
fn upload_notes(segment: &ClipPlanSegment) -> String {
    let mut lines = vec![segment.summary.clone().unwrap_or_default(), String::new()];
    if let Some(description) = &segment.description {
        lines.extend([description.clone(), String::new()]);
    }
    if segment.output_chapters.len() >= 3 {
        lines.extend(segment.output_chapters.iter().map(|(t, title)| format!("{} {title}", stamp(*t))));
        lines.push(String::new());
    }
    if !segment.tags.is_empty() {
        lines.push(format!("Tags: {}", segment.tags.join(", ")));
    }
    format!("{}\n", lines.join("\n").trim())
}

fn write_upload_notes(path: &Path, segment: &ClipPlanSegment) {
    if let Err(e) = std::fs::write(path, upload_notes(segment)) {
        tracing::warn!("Could not write upload notes: {e}");
    }
}

/// Known failure classes -> (markers, message, hint). Port of bridge_runner.FAILURES; first match wins.
const FAILURES: &[(&[&str], &str, &str)] = &[
    (
        &["selected planner requires a video with speech"],
        "The selected AI provider cannot analyze a video without speech.",
        "CLI providers read text only. Use an API key provider in Settings for silent videos.",
    ),
    (
        &["cli is not logged in"],
        "The AI CLI is not logged in.",
        "Open a terminal, run the CLI once and sign in, then retry.",
    ),
    (
        &["cli was not found", "cli could not be started"],
        "The selected AI CLI could not be started.",
        "Open Settings → AI provider and pick a detected provider.",
    ),
    (
        &["cli timed out"],
        "The AI CLI took too long to plan clips.",
        "Retry, or trim the video to a shorter range.",
    ),
    (
        &["local ai server is not reachable"],
        "The local AI server is not reachable.",
        "Start Ollama or LM Studio (or your server) and check the server URL in Settings → AI provider.",
    ),
    (
        &["local model not found"],
        "The local AI server does not have the selected model.",
        "Pull or load the model first (for example `ollama pull qwen2.5:14b`), then pick it in Settings → AI provider.",
    ),
    (
        &["local model context is too small"],
        "The local model's context window is too small for this video's transcript.",
        "Use a model with a larger context (or raise the server's context length), or set a shorter start and end time.",
    ),
    (
        &["local model is too large"],
        "The local model does not fit in this computer's memory.",
        "Choose a smaller model or quantization, or close other apps, then retry.",
    ),
    (
        &["local ai server timed out"],
        "The local AI model took too long to plan clips.",
        "Use a smaller or faster model, or set a shorter start and end time.",
    ),
    (
        &["local ai endpoint was not found"],
        "The local AI server has no chat endpoint at that URL.",
        "Use the server's OpenAI-compatible base URL, which usually ends in /v1 (Ollama: http://localhost:11434/v1).",
    ),
    (
        &["unsupported twitch source"],
        "Choose a public, completed Twitch VOD.",
        "Copy the video link from a saved Twitch video. Live channels, collections and Twitch clips are not supported.",
    ),
    (
        &["twitch vod is not completed"],
        "This Twitch video is still live or processing.",
        "Wait until the broadcast has ended and its saved video is ready, then retry.",
    ),
    (
        &["twitch vod duration is invalid or too long"],
        "This Twitch video has no usable duration or exceeds the six hour limit.",
        "Choose a completed video under six hours, or trim a downloaded file before adding it.",
    ),
    (
        &["twitch vod unavailable"],
        "The Twitch VOD could not be downloaded.",
        "Check that the saved video plays while signed out. Deleted, expired and subscriber-only videos are not supported. You can also use a local file.",
    ),
    (
        &["not enough disk space to save clips", "no space left on device", "disk quota exceeded"],
        "There is not enough free disk space to finish this video.",
        "Free space on your startup disk and the output drive, then retry. Source videos can use several GB while clipping.",
    ),
    (
        &["whisper model is not downloaded"],
        "The offline transcription model is not downloaded.",
        "Open Settings → Transcription and download a Whisper model, then retry.",
    ),
    (
        &["local transcription failed", "whisper.cpp is not installed"],
        "Offline transcription failed.",
        "Run Settings → System check. A smaller Whisper model uses less memory; try one if this persists.",
    ),
    (
        &["audio extraction failed", "audio duration could not be determined", "transcription audio preparation failed"],
        "Clipward could not prepare this video's audio for transcription.",
        "Run Settings → System check. If the tools are ready, report this run with its failure code.",
    ),
    (
        &["transcription audio chunk exceeded the size limit"],
        "The transcription audio exceeded Clipward's size limit.",
        "Set a shorter start and end time, or report this run so the chunk size can be adjusted.",
    ),
    (
        &["transcription failed"],
        "Audio transcription failed.",
        "Check that the video has a playable audio track, then retry.",
    ),
    (
        &["video render failed"],
        "Clip rendering failed.",
        "Run Settings → System check. If all tools are ready, report this run so the render can be diagnosed.",
    ),
    (
        &["http error 403", "sign in to confirm", "blocking this request"],
        "The video service refused the download.",
        "Update Clipward and retry. If it keeps happening, download the video yourself and clip it as a local file.",
    ),
    (
        &["video unavailable", "private video", "members-only", "has been removed", "not available in your country"],
        "This video is private, removed or unavailable in your region.",
        "Check the link opens in a signed-out browser window, or clip a local file instead.",
    ),
    (
        &["exceeds maximum allowed duration"],
        "This video is longer than Clipward can process.",
        "Choose a shorter source video, or trim a downloaded file before adding it.",
    ),
    (
        &["no clip-worthy moments"],
        "Clipward couldn't find any clips in this video.",
        "No clear spoken or visual moment met the selected clip length. If you set a start and end time, widen it or pick a shorter clip length.",
    ),
    (
        &["out of credits", "quota exceeded"],
        "Your AI provider account is out of credits.",
        "Add credits to the provider account or raise the key's limit.",
    ),
    (
        &["(401)", "status_code: 401", "unauthorized", "invalid api key", "invalid_api_key"],
        "An API key was rejected.",
        "Check the AI provider key in Settings.",
    ),
    (
        &["timed out", "connection", "name resolution", "network is unreachable"],
        "A network request failed.",
        "Check your internet connection and retry.",
    ),
    (
        &["video download failed"],
        "The video could not be downloaded.",
        "Check the link opens in a browser and your connection works, then retry. You can also clip a local file instead.",
    ),
];

const FALLBACK: (&str, &str) = (
    "The clipping pipeline failed.",
    "Check System check and retry. If it persists, report the steps that reproduce it.",
);

/// Fixed, safe UI text (message, hint) for an engine error. Port of bridge_runner.describe_failure:
/// raw error text can hold URLs, credentials and paths, so only these fixed strings reach the UI.
pub fn describe_failure(err: &EngineError) -> (String, String) {
    let (message, hint) = match err {
        EngineError::Cancelled => ("Pipeline cancelled by user", ""),
        EngineError::Failed(text) => describe_text(text),
    };
    (message.into(), hint.into())
}

fn describe_text(text: &str) -> (&'static str, &'static str) {
    let text = text.to_lowercase();
    FAILURES
        .iter()
        .find(|(markers, _, _)| markers.iter().any(|m| text.contains(m)))
        .map(|&(_, message, hint)| (message, hint))
        .unwrap_or(FALLBACK)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the real pipeline on a generated local video (skipped without ffmpeg): the
    /// source is probed in place, then transcription stops on the missing model with
    /// the fixed UI message, and the local source is left untouched.
    #[tokio::test]
    async fn local_job_reaches_transcription() {
        let (Ok(ffmpeg), Ok(ffprobe)) = (which::which("ffmpeg"), which::which("ffprobe")) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("talk.mp4");
        let status = std::process::Command::new(&ffmpeg)
            .args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=2"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-shortest", "-pix_fmt", "yuv420p"])
            .arg(&video)
            .status()
            .unwrap();
        assert!(status.success());
        let cfg = EngineConfig {
            ffmpeg,
            ffprobe,
            ytdlp: None,
            assets_dir: dir.path().join("assets"),
            whisper_model_path: dir.path().join("missing.bin"),
            ai: Default::default(),
            custom_vocabulary: vec![],
            work_dir: dir.path().join("work"),
            output_dir: dir.path().join("run"),
        };
        let req: ClipJobRequest = serde_json::from_value(json!({
            "videoUrl": video.to_string_lossy(), "maxClips": null, "autoClipCount": true,
            "durationRanges": null, "aspectRatio": "9:16", "layoutStyle": "auto", "layoutVision": false,
            "pacing": "tight", "includeCaptions": true, "captionPreset": "pop",
            "startTimeSeconds": null, "endTimeSeconds": null, "bannerPlatform": null, "bannerChannelUrl": null
        }))
        .unwrap();
        let stages = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = stages.clone();
        let progress: ProgressFn = std::sync::Arc::new(move |p: Progress| seen.lock().unwrap().push(p.stage));
        let err = run_job(req, cfg, progress, CancellationToken::new()).await.unwrap_err();
        assert_eq!(describe_failure(&err).0, "The offline transcription model is not downloaded.");
        let stages = stages.lock().unwrap();
        assert!(stages.contains(&Stage::Downloading) && stages.contains(&Stage::Transcribing));
        assert!(video.is_file());
    }

    /// The app shell spawns run_job on a multi-threaded runtime, so its future must be Send.
    #[allow(dead_code)]
    fn run_job_future_is_send(req: ClipJobRequest, cfg: EngineConfig, progress: ProgressFn) {
        fn assert_send<T: Send>(_: T) {}
        assert_send(run_job(req, cfg, progress, CancellationToken::new()));
    }

    #[test]
    fn upload_notes_match_python_format() {
        let segment = ClipPlanSegment {
            summary: Some("Title".into()),
            description: Some("About it".into()),
            output_chapters: vec![(0, "Intro".into()), (65_000, "Middle".into()), (3_725_000, "End".into())],
            tags: vec!["a".into(), "b".into()],
            ..Default::default()
        };
        assert_eq!(upload_notes(&segment), "Title\n\nAbout it\n\n0:00 Intro\n1:05 Middle\n1:02:05 End\n\nTags: a, b\n");
        assert_eq!(chapter_dicts(&segment)[1], json!({"time_ms": 65000, "title": "Middle"}));
        let short = ClipPlanSegment { output_chapters: vec![(0, "x".into())], ..Default::default() };
        assert!(chapter_dicts(&short).is_null());
    }

    fn message(text: &str) -> String {
        describe_failure(&EngineError::msg(text)).0
    }

    #[test]
    fn maps_known_failures_to_fixed_text() {
        assert_eq!(message("YouTube download failed. Last error: ERROR: unable to download video data: HTTP Error 403: Forbidden"), "The video service refused the download.");
        let (m, h) = describe_failure(&EngineError::msg("socks5h://user:secret-pass@10.0.0.1:1 /Users/someone/private.mp4"));
        assert_eq!(m, "The clipping pipeline failed.");
        assert!(!h.contains("secret-pass"));
        assert_eq!(message("No clip-worthy moments found (the video may have no speech)"), "Clipward couldn't find any clips in this video.");
        assert_eq!(message("Whisper model is not downloaded"), "The offline transcription model is not downloaded.");
        assert_eq!(message("Local transcription failed"), "Offline transcription failed.");
        assert_eq!(message("Claude Code CLI is not logged in (unauthorized)"), "The AI CLI is not logged in.");
        assert_eq!(message("Codex CLI timed out"), "The AI CLI took too long to plan clips.");
        assert_eq!(message("Video render failed"), "Clip rendering failed.");
        assert_eq!(message("Not enough disk space to save clips"), "There is not enough free disk space to finish this video.");
        assert_eq!(message("io error: No space left on device (os error 28)"), "There is not enough free disk space to finish this video.");
        assert_eq!(message("Video download failed"), "The video could not be downloaded.");
        assert!(describe_failure(&EngineError::msg("twitch vod unavailable")).1.contains("signed out"));
    }
}
