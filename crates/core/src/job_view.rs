//! A run's results as the UI reads them (port of src/shared/job-output.ts, the job
//! helpers in src/shared/jobs.ts and the result notices in ClipList.tsx).
//!
//! `parse_job_output` checks external and persisted pipeline output with the same
//! rules as the shell's `job_output::parse_job_output` (which it runs) and returns typed
//! structs. Every field the checks may drop is an `Option`.
use crate::jobs::JobSnapshot;
use crate::shared::is_active_job_status;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ClipArtifact {
    #[serde(deserialize_with = "index")]
    pub clip_index: u64,
    pub s3_url: String,
    pub duration_ms: f64,
    pub start_time_ms: f64,
    pub end_time_ms: f64,
    pub virality_score: f64,
    /// A known layout, or "" for anything else.
    pub layout_type: String,
    pub summary: Option<String>,
    pub tags: Vec<String>,
    /// Set when smart framing failed and a letterbox fallback produced the clip.
    pub render_fallback: Option<String>,
    /// The clip has an edit file and can open in the editor (runs from before the editor can't).
    #[serde(default)]
    pub editable: bool,
    /// Edited in the editor since its last export: the clip file doesn't have those edits yet.
    #[serde(default)]
    pub unexported: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct JobOutput {
    pub job_id: String,
    pub source_video_url: String,
    pub source_video_title: String,
    pub source_video_duration_seconds: f64,
    pub total_clips: usize,
    pub clips: Vec<ClipArtifact>,
    pub transcript_url: Option<String>,
    pub plan_url: Option<String>,
    pub processing_time_seconds: f64,
    pub metrics: Option<Metrics>,
    pub created_at: Option<String>,
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Metrics {
    pub smart_framing_available: Option<bool>,
    /// available | no_speech | failed
    pub transcription_status: Option<String>,
    /// visual | transcript | manual
    pub planning_source: Option<String>,
    /// enabled | disabled_by_request | unavailable_without_transcript
    pub captions_status: Option<String>,
    pub visual_frame_count: Option<f64>,
    pub requested_settings: Option<RequestedSettings>,
    pub planned_clip_count: Option<f64>,
    pub rendered_clip_count: Option<f64>,
    pub failed_clip_count: Option<f64>,
    pub uploaded_clip_count: Option<f64>,
    pub source_video_size_bytes: Option<f64>,
    pub rendered_output_bytes: Option<f64>,
    pub peak_rss_mb: Option<f64>,
    pub analysis_duration_seconds: Option<f64>,
    pub stage_durations_seconds: Option<BTreeMap<String, f64>>,
    pub clip_layouts: Option<Vec<ClipLayout>>,
    pub api_costs: Option<ApiCosts>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct RequestedSettings {
    pub ai_provider: Option<String>,
    pub planner_model: Option<String>,
    pub transcription_model: Option<String>,
    pub aspect_ratio: Option<String>,
    pub layout_style: Option<String>,
    pub layout_vision_enabled: Option<bool>,
    pub pacing: Option<String>,
    pub video_speed: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ClipLayout {
    #[serde(deserialize_with = "index")]
    pub clip_index: u64,
    pub layout_type: Option<String>,
    /// smart | classic | whole_frame_auto | fallback
    pub framing_status: Option<String>,
    pub pacing_removed_ms: Option<f64>,
    pub render_fallback: Option<String>,
    /// Shots the engine recorded, before the first 256 were kept.
    pub shot_count: Option<usize>,
    pub shots: Option<Vec<Shot>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Shot {
    pub start_ms: f64,
    pub end_ms: f64,
    pub layout: Option<String>,
    pub source: Option<String>,
    pub screen_box: Option<[f64; 4]>,
    pub screen_focus: Option<[f64; 4]>,
    pub cam_box: Option<[f64; 4]>,
    pub people: Option<Vec<[f64; 4]>>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ApiCosts {
    pub total_estimated_cost_usd: f64,
    pub cost_incomplete: Option<bool>,
    pub transcription: Option<CostSection>,
    pub planning: Option<CostSection>,
    pub layout_vision: Option<CostSection>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct CostSection {
    pub cost_incomplete: Option<bool>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub estimated_cost_usd: Option<f64>,
    pub audio_duration_seconds: Option<f64>,
    pub prompt_tokens: Option<f64>,
    pub completion_tokens: Option<f64>,
    pub total_tokens: Option<f64>,
    pub attempts: Option<f64>,
}

/// Clip indexes are checked as safe integers, which JSON may write as `3.0`.
fn index<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    Ok(f64::deserialize(deserializer)? as u64)
}

/// Validate external and persisted pipeline output before the UI shows it.
pub fn parse_job_output(value: &Value) -> Option<JobOutput> {
    let safe = crate::job_output::parse_job_output(value)?;
    let mut output: JobOutput = serde_json::from_value(safe).ok()?;
    // The shell's check drops the editor flags; they are only ever `true` or absent.
    for (clip, raw) in output.clips.iter_mut().zip(value["clips"].as_array()?) {
        clip.editable = raw["editable"] == true;
        clip.unexported = raw["unexported"] == true;
    }
    Some(output)
}

/// Clips the user typed in manual clip mode: no virality scores, so no ranking.
pub fn is_manual_run(output: &JobOutput) -> bool {
    planning_source(output) == Some("manual")
}

fn planning_source(output: &JobOutput) -> Option<&str> {
    output.metrics.as_ref()?.planning_source.as_deref()
}

pub fn is_job_active(job: &JobSnapshot) -> bool {
    is_active_job_status(&job.status)
}

/// Queued and running jobs, oldest first (the order they run in).
pub fn active_jobs(jobs: &[JobSnapshot]) -> Vec<&JobSnapshot> {
    let mut active: Vec<&JobSnapshot> = jobs.iter().filter(|job| is_job_active(job)).collect();
    active.sort_by(|a, b| a.queued_at.cmp(&b.queued_at));
    active
}

/// Persisted runs may contain incomplete cost data; only display complete sections.
pub fn read_costs(costs: Option<&ApiCosts>) -> Option<ApiCosts> {
    let costs = costs?;
    let amount = costs.total_estimated_cost_usd;
    if !amount.is_finite() || amount < 0.0 {
        return None;
    }
    fn money(value: Option<f64>) -> bool {
        value.is_some_and(|n| n.is_finite() && n >= 0.0)
    }
    fn keep(
        section: &Option<CostSection>,
        complete: fn(&CostSection) -> bool,
    ) -> Option<CostSection> {
        section.clone().filter(complete)
    }
    Some(ApiCosts {
        total_estimated_cost_usd: amount,
        cost_incomplete: (costs.cost_incomplete == Some(true)).then_some(true),
        transcription: keep(&costs.transcription, |s| {
            s.provider.is_some()
                && s.model.is_some()
                && money(s.audio_duration_seconds)
                && money(s.estimated_cost_usd)
        }),
        planning: keep(&costs.planning, |s| {
            s.model.is_some()
                && money(s.total_tokens)
                && money(s.attempts)
                && money(s.estimated_cost_usd)
        }),
        layout_vision: keep(&costs.layout_vision, |s| {
            s.model.is_some() && money(s.estimated_cost_usd)
        }),
    })
}

/// Explain when a vertical clip kept its whole frame or fell back during rendering.
pub fn framing_problem(output: &JobOutput, vertical: bool) -> Option<String> {
    if !vertical || output.clips.is_empty() {
        return None;
    }
    let metrics = output.metrics.as_ref();
    let requested = metrics.and_then(|m| m.requested_settings.as_ref());
    if requested.is_some_and(|r| r.layout_style.as_deref() != Some("auto")) {
        return None;
    }
    if metrics.and_then(|m| m.smart_framing_available) == Some(false) {
        return Some("Smart framing was unavailable, so every clip used the classic whole-frame layout. OpenCV or the face model is missing from the engine; run Settings → System check.".into());
    }
    let mut notices = Vec::new();
    let fallbacks = output
        .clips
        .iter()
        .filter(|clip| {
            clip.render_fallback
                .as_deref()
                .is_some_and(|f| !f.is_empty())
        })
        .count();
    if fallbacks > 0 {
        notices.push(format!("{fallbacks} of {} clips couldn't render with smart framing and used the classic whole-frame layout instead. The log has the details.", output.clips.len()));
    }
    let whole_frame: BTreeSet<u64> = metrics
        .and_then(|m| m.clip_layouts.as_ref())
        .into_iter()
        .flatten()
        .filter(|layout| layout.framing_status.as_deref() == Some("whole_frame_auto"))
        .map(|layout| layout.clip_index)
        .filter(|index| output.clips.iter().any(|clip| clip.clip_index == *index))
        .collect();
    if !whole_frame.is_empty() {
        let shown: Vec<String> = whole_frame
            .iter()
            .take(5)
            .map(|index| (index + 1).to_string())
            .collect();
        let more = if whole_frame.len() > 5 {
            format!(" and {} more", whole_frame.len() - 5)
        } else {
            String::new()
        };
        let plural = if whole_frame.len() == 1 { "" } else { "s" };
        notices.push(format!("Smart framing kept the whole frame for clip{plural} {}{more}. Review the framing if you expected a closer crop.", shown.join(", ")));
    }
    (!notices.is_empty()).then(|| notices.join(" "))
}

pub fn source_analysis_notice(output: &JobOutput) -> Option<&'static str> {
    if planning_source(output) != Some("visual") {
        return None;
    }
    match output.metrics.as_ref()?.transcription_status.as_deref() {
        Some("no_speech") => Some("No speech was detected. Clips were selected from sampled video frames, and spoken-word captions are unavailable for this run."),
        Some("failed") => Some("Transcription failed. Clips were selected from sampled video frames, and spoken-word captions are unavailable for this run."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn clip(index: u64) -> Value {
        json!({
            "clip_index": index, "s3_url": format!("/tmp/clip-{index}.mp4"), "duration_ms": 5000,
            "start_time_ms": index * 5000, "end_time_ms": (index + 1) * 5000, "virality_score": 0.5
        })
    }

    fn requested(output: &JobOutput) -> &RequestedSettings {
        output
            .metrics
            .as_ref()
            .unwrap()
            .requested_settings
            .as_ref()
            .unwrap()
    }

    #[test]
    fn saved_run_speed_is_retained_while_invalid_speed_metadata_is_discarded() {
        let output = parse_job_output(
            &json!({ "clips": [], "metrics": { "requested_settings": { "video_speed": 1.5 } } }),
        )
        .unwrap();
        assert_eq!(requested(&output).video_speed, Some(1.5));
        // JSON has no Infinity; json! writes it as null.
        for video_speed in [
            json!("2"),
            Value::Null,
            json!(f64::INFINITY),
            json!(0),
            json!(3),
        ] {
            let output = parse_job_output(&json!({ "clips": [], "metrics": { "requested_settings": { "video_speed": video_speed } } })).unwrap();
            assert_eq!(requested(&output).video_speed, None);
        }
    }

    #[test]
    fn clip_list_explains_when_smart_framing_intentionally_keeps_the_whole_frame() {
        let output = parse_job_output(&json!({
            "clips": [clip(0), clip(1)],
            "metrics": { "clip_layouts": [
                { "clip_index": 0, "framing_status": "whole_frame_auto" },
                { "clip_index": 0, "framing_status": "whole_frame_auto" },
                { "clip_index": 9, "framing_status": "whole_frame_auto" }
            ] }
        }))
        .unwrap();
        assert!(framing_problem(&output, true)
            .unwrap()
            .contains("whole frame for clip 1"));
        assert_eq!(framing_problem(&output, false), None);
        let classic = parse_job_output(&json!({ "clips": [clip(0)], "metrics": {
            "smart_framing_available": false, "requested_settings": { "aspect_ratio": "9:16", "layout_style": "fit" }
        } }))
        .unwrap();
        assert_eq!(framing_problem(&classic, true), None);
    }

    #[test]
    fn framing_notices_count_fallbacks_and_list_at_most_five_clips() {
        let mut clips: Vec<Value> = (0..7).map(clip).collect();
        clips[2]["render_fallback"] = json!("letterbox");
        let layouts: Vec<Value> = (0..7)
            .rev()
            .map(|i| json!({ "clip_index": i, "framing_status": "whole_frame_auto" }))
            .collect();
        let output = parse_job_output(&json!({ "clips": clips, "metrics": {
            "clip_layouts": layouts, "requested_settings": { "layout_style": "auto" } } }))
        .unwrap();
        assert_eq!(
            framing_problem(&output, true).unwrap(),
            "1 of 7 clips couldn't render with smart framing and used the classic whole-frame layout instead. The log has the details. \
             Smart framing kept the whole frame for clips 1, 2, 3, 4, 5 and 2 more. Review the framing if you expected a closer crop."
        );
        let unavailable = parse_job_output(
            &json!({ "clips": [clip(0)], "metrics": { "smart_framing_available": false } }),
        )
        .unwrap();
        assert!(framing_problem(&unavailable, true)
            .unwrap()
            .starts_with("Smart framing was unavailable"));
        // Settings recorded without a framing style read as classic, as in the renderer.
        let unstyled = parse_job_output(&json!({ "clips": [clip(0)], "metrics": {
            "smart_framing_available": false, "requested_settings": {} } }))
        .unwrap();
        assert_eq!(framing_problem(&unstyled, true), None);
    }

    #[test]
    fn visual_only_runs_disclose_unavailable_captions_and_preserve_analysis_status() {
        let output = parse_job_output(&json!({ "clips": [], "metrics": {
            "transcription_status": "no_speech", "planning_source": "visual", "visual_frame_count": 12,
            "captions_status": "unavailable_without_transcript"
        } }))
        .unwrap();
        let metrics = output.metrics.as_ref().unwrap();
        assert_eq!(metrics.visual_frame_count, Some(12.0));
        assert_eq!(
            metrics.captions_status.as_deref(),
            Some("unavailable_without_transcript")
        );
        assert!(source_analysis_notice(&output)
            .unwrap()
            .contains("No speech was detected"));
        let failed = parse_job_output(&json!({ "clips": [], "metrics": {
            "transcription_status": "failed", "planning_source": "visual" } }))
        .unwrap();
        assert!(source_analysis_notice(&failed)
            .unwrap()
            .starts_with("Transcription failed."));
        let spoken = parse_job_output(&json!({ "clips": [], "metrics": {
            "transcription_status": "failed", "planning_source": "transcript" } }))
        .unwrap();
        assert_eq!(source_analysis_notice(&spoken), None);
    }

    #[test]
    fn manual_runs_keep_their_planning_source() {
        let clip = json!({ "clip_index": 0, "s3_url": "/tmp/clip-0.mp4", "duration_ms": 5000, "start_time_ms": 0, "end_time_ms": 5000, "virality_score": 0 });
        let output = parse_job_output(&json!({ "clips": [clip], "metrics": {
            "planning_source": "manual", "requested_settings": { "ai_provider": "none" } } }))
        .unwrap();
        assert_eq!(
            output.metrics.as_ref().unwrap().planning_source.as_deref(),
            Some("manual")
        );
        assert_eq!(requested(&output).ai_provider.as_deref(), Some("none"));
        assert!(is_manual_run(&output));
        let transcript = parse_job_output(
            &json!({ "clips": [], "metrics": { "planning_source": "transcript" } }),
        )
        .unwrap();
        assert!(!is_manual_run(&transcript));
        let other =
            parse_job_output(&json!({ "clips": [], "metrics": { "planning_source": "other" } }))
                .unwrap();
        assert_eq!(other.metrics.unwrap().planning_source, None);
    }

    #[test]
    fn historical_clip_results_reject_paths_duplicate_indexes_and_impossible_time_ranges() {
        let clip = json!({
            "clip_index": 0, "s3_url": "file:///clips/clip.mp4", "duration_ms": 6000,
            "start_time_ms": 10_000, "end_time_ms": 16_000, "virality_score": 0.8
        });
        let valid = parse_job_output(&json!({ "job_id": "old-run", "clips": [clip] })).unwrap();
        assert_eq!(valid.clips[0].summary, None);
        assert_eq!(valid.clips[0].render_fallback, None);
        assert_eq!(valid.total_clips, 1);
        // Defaults for missing fields.
        assert_eq!(valid.source_video_title, "Untitled video");
        assert_eq!(valid.source_video_duration_seconds, 0.0);
        assert_eq!(valid.metrics, None);
        assert_eq!(valid.created_at, None);
        let with = |patch: Value| {
            let mut bad = clip.clone();
            for (key, value) in patch.as_object().unwrap() {
                bad[key] = value.clone();
            }
            json!({ "clips": [bad] })
        };
        assert_eq!(parse_job_output(&with(json!({ "s3_url": " " }))), None);
        assert_eq!(
            parse_job_output(&json!({ "clips": [clip, clip.clone()] })),
            None
        );
        assert_eq!(
            parse_job_output(&with(json!({ "start_time_ms": 20_000 }))),
            None
        );
        assert_eq!(parse_job_output(&json!({ "clips": "nope" })), None);
        assert_eq!(parse_job_output(&json!([])), None);
    }

    #[test]
    fn clip_results_retain_bounded_framing_pacing_and_complete_cost_metrics() {
        let output = parse_job_output(&json!({
            "job_id": "run-one", "clips": [], "metrics": {
                "analysis_duration_seconds": 120,
                "smart_framing_available": true, "failed_clip_count": 1,
                "stage_durations_seconds": { "rendering": 12.5, "invalid/name": 99 },
                "clip_layouts": [{ "clip_index": 0, "layout_type": "screen_cam", "pacing_removed_ms": 1200,
                    "render_fallback": "letterbox", "shots": [{ "start_ms": 0, "end_ms": 5000,
                        "layout": "screen_cam", "source": "vision", "cam_box": [0.5, 0.5, 0.4, 0.4],
                        "screen_box": [0, 0, 1, 1], "secret": "ignored" }] }],
                "api_costs": { "total_estimated_cost_usd": 0.04,
                    "planning": { "provider": "openrouter", "model": "example", "prompt_tokens": 100,
                        "completion_tokens": 20, "total_tokens": 120, "attempts": 1, "estimated_cost_usd": 0.03, "secret": "ignored" } },
                "secret": "ignored"
            }
        }))
        .unwrap();
        let metrics = output.metrics.unwrap();
        let layout = &metrics.clip_layouts.as_ref().unwrap()[0];
        assert_eq!(layout.pacing_removed_ms, Some(1200.0));
        assert_eq!(layout.shot_count, Some(1));
        let shot = &layout.shots.as_ref().unwrap()[0];
        assert_eq!(shot.cam_box, Some([0.5, 0.5, 0.4, 0.4]));
        assert_eq!(shot.screen_box, Some([0.0, 0.0, 1.0, 1.0]));
        let planning = metrics
            .api_costs
            .as_ref()
            .unwrap()
            .planning
            .as_ref()
            .unwrap();
        assert_eq!(planning.prompt_tokens, Some(100.0));
        assert_eq!(planning.completion_tokens, Some(20.0));
        let stages = metrics.stage_durations_seconds.as_ref().unwrap();
        assert_eq!(stages.get("invalid/name"), None);
        assert_eq!(stages.get("rendering"), Some(&12.5));
        assert_eq!(metrics.analysis_duration_seconds, Some(120.0));
    }

    #[test]
    fn editor_flags_count_only_when_true() {
        let mut flagged = clip(0);
        flagged["editable"] = json!(true);
        flagged["unexported"] = json!("yes");
        let output = parse_job_output(&json!({ "clips": [flagged, clip(1)] })).unwrap();
        assert!(output.clips[0].editable);
        assert!(!output.clips[0].unexported);
        assert!(!output.clips[1].editable);
        // Float indexes that are whole numbers are valid.
        let mut float = clip(0);
        float["clip_index"] = json!(2.0);
        assert_eq!(
            parse_job_output(&json!({ "clips": [float] }))
                .unwrap()
                .clips[0]
                .clip_index,
            2
        );
    }

    #[test]
    fn costs_show_only_complete_sections() {
        let output = parse_job_output(&json!({ "clips": [], "metrics": { "api_costs": {
            "total_estimated_cost_usd": 0.05, "cost_incomplete": false,
            "transcription": { "provider": "openai", "model": "whisper-1", "audio_duration_seconds": 60, "estimated_cost_usd": 0.006 },
            "planning": { "model": "m", "total_tokens": 10, "estimated_cost_usd": 0.04 },
            "layout_vision": { "model": "v", "estimated_cost_usd": 0.004 }
        } } }))
        .unwrap();
        let costs = read_costs(output.metrics.unwrap().api_costs.as_ref()).unwrap();
        assert_eq!(costs.cost_incomplete, None);
        assert!(costs.transcription.is_some());
        assert_eq!(costs.planning, None, "no attempts recorded");
        assert!(costs.layout_vision.is_some());
        assert_eq!(read_costs(None), None);
        let partial = ApiCosts {
            cost_incomplete: Some(true),
            ..ApiCosts::default()
        };
        assert_eq!(
            read_costs(Some(&partial)).unwrap().cost_incomplete,
            Some(true)
        );
        let negative = ApiCosts {
            total_estimated_cost_usd: -1.0,
            ..ApiCosts::default()
        };
        assert_eq!(read_costs(Some(&negative)), None);
    }
}
