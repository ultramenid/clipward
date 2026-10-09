//! Manual clip mode: the user typed the moments, so no AI planner runs. The ranges
//! become plan segments exactly as typed (no sentence snapping), clamped to the video.
use crate::error::{EngineError, Result};
use crate::types::{ClipPlan, ClipPlanSegment, ManualClip, PlanningCosts};

/// Marker for `pipeline::describe_failure` when no typed range overlaps the video.
pub const OUTSIDE_VIDEO: &str = "manual clip is outside the video";
/// Shortest and longest manual clip, in seconds (`clipward_core::shared` re-exports these).
pub const MIN_CLIP_SECONDS: f64 = 3.0;
pub const MAX_CLIP_SECONDS: f64 = 900.0;
pub const MAX_CLIPS: usize = 50;
pub const MAX_TITLE_CHARS: usize = 120;
/// `planning_source` for plans built from the user's own ranges.
pub const PLANNING_SOURCE: &str = "manual";
/// Layout the AI planner gives every segment before rendering picks the real one.
const DEFAULT_LAYOUT: &str = "center_crop";

/// Source seconds covering every manual clip (earliest start -> latest end), so only
/// that part of the video is transcribed. None when there is nothing usable.
pub fn transcription_range(clips: &[ManualClip]) -> Option<(f64, f64)> {
    let usable = clips.iter().filter(|c| c.start_seconds.is_finite() && c.end_seconds.is_finite());
    let (start, end) = usable.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), c| {
        (lo.min(c.start_seconds), hi.max(c.end_seconds))
    });
    (start.is_finite() && end > start).then(|| (start.max(0.0), end))
}

/// Builds the plan from the user's ranges. Each range is clamped to the video; one that
/// ends up outside it (or shorter than [`MIN_CLIP_SECONDS`]) is skipped with a warning.
/// Fails with [`OUTSIDE_VIDEO`] when no range is left.
pub fn build_plan(clips: &[ManualClip], video_duration_seconds: f64) -> Result<ClipPlan> {
    let limit = if video_duration_seconds.is_finite() && video_duration_seconds > 0.0 {
        video_duration_seconds
    } else {
        f64::INFINITY
    };
    let mut segments = Vec::with_capacity(clips.len());
    for (i, clip) in clips.iter().enumerate() {
        if !clip.start_seconds.is_finite() || !clip.end_seconds.is_finite() {
            tracing::warn!("Skipping manual clip {}: its times are not numbers", i + 1);
            continue;
        }
        let start = clip.start_seconds.max(0.0);
        let end = clip.end_seconds.min(limit);
        if end - start < MIN_CLIP_SECONDS {
            tracing::warn!(
                "Skipping manual clip {}: {:.1}-{:.1}s is outside the {:.1}s video or too short once clamped",
                i + 1,
                clip.start_seconds,
                clip.end_seconds,
                video_duration_seconds
            );
            continue;
        }
        if end < clip.end_seconds {
            tracing::warn!("Manual clip {} ends after the video; trimmed to {end:.1}s", i + 1);
        }
        let title = clip.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
        segments.push(ClipPlanSegment {
            start_time_ms: (start * 1000.0).round() as i64,
            end_time_ms: (end * 1000.0).round() as i64,
            virality_score: 0.0,
            layout_type: DEFAULT_LAYOUT.into(),
            summary: Some(title.map_or_else(|| format!("Clip {}", i + 1), str::to_string)),
            ..Default::default()
        });
    }
    if segments.is_empty() {
        return Err(EngineError::msg(format!(
            "{OUTSIDE_VIDEO}: no manual clip fits the {video_duration_seconds:.1}s video"
        )));
    }
    Ok(ClipPlan {
        clips: segments,
        costs: PlanningCosts { provider: "none".into(), model: "none".into(), ..Default::default() },
        planning_source: PLANNING_SOURCE.into(),
        insights: None,
        visual_frame_count: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(start: f64, end: f64, title: Option<&str>) -> ManualClip {
        ManualClip { start_seconds: start, end_seconds: end, title: title.map(str::to_string) }
    }

    #[test]
    fn keeps_exact_boundaries_titles_and_order() {
        let plan = build_plan(&[clip(12.345, 42.0, Some("  Hook  ")), clip(5.0, 20.0, None), clip(10.0, 30.0, Some(""))], 60.0)
            .unwrap();
        assert_eq!(plan.planning_source, "manual");
        assert_eq!(plan.costs.provider, "none");
        assert_eq!(plan.costs.estimated_cost_usd, 0.0);
        let spans: Vec<_> = plan.clips.iter().map(|c| (c.start_time_ms, c.end_time_ms)).collect();
        // Overlaps are allowed and nothing is snapped or reordered.
        assert_eq!(spans, [(12_345, 42_000), (5_000, 20_000), (10_000, 30_000)]);
        let titles: Vec<_> = plan.clips.iter().map(|c| c.summary.clone().unwrap()).collect();
        assert_eq!(titles, ["Hook", "Clip 2", "Clip 3"]);
        let first = &plan.clips[0];
        assert_eq!(first.virality_score, 0.0);
        assert_eq!(first.layout_type, "center_crop");
        assert!(first.tags.is_empty() && first.emphasis_words.is_empty());
        assert!(first.skip_ranges_ms.is_empty() && first.chapters.is_empty());
    }

    #[test]
    fn clamps_to_the_video_and_skips_ranges_outside_it() {
        let plan = build_plan(&[clip(50.0, 90.0, None), clip(70.0, 80.0, None), clip(58.5, 70.0, None)], 60.0).unwrap();
        // The second is past the end; the third is under 3 s once clamped.
        assert_eq!(plan.clips.len(), 1);
        assert_eq!((plan.clips[0].start_time_ms, plan.clips[0].end_time_ms), (50_000, 60_000));
        assert_eq!(plan.clips[0].summary.as_deref(), Some("Clip 1"));
    }

    #[test]
    fn fails_with_the_marker_when_every_clip_is_outside() {
        let err = build_plan(&[clip(100.0, 120.0, None), clip(f64::NAN, 5.0, None)], 60.0).unwrap_err();
        assert!(err.to_string().contains(OUTSIDE_VIDEO));
        assert!(build_plan(&[], 60.0).is_err());
    }

    #[test]
    fn unknown_duration_does_not_clamp() {
        let plan = build_plan(&[clip(0.0, 400.0, None)], 0.0).unwrap();
        assert_eq!(plan.clips[0].end_time_ms, 400_000);
    }

    #[test]
    fn transcription_range_covers_all_clips() {
        assert_eq!(transcription_range(&[clip(30.0, 40.0, None), clip(5.0, 20.0, None), clip(35.0, 90.0, None)]), Some((5.0, 90.0)));
        assert_eq!(transcription_range(&[]), None);
    }
}
