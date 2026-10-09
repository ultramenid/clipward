//! The Library page and clip list rules as pure functions (port of the logic in
//! LibraryPage.tsx, ClipList.tsx, ClipCard.tsx and RunStats.tsx), so the GPUI views
//! stay thin and these stay tested.
use crate::file_manager::{ExportResult, JobHistoryEntry};
use crate::job_view::{is_manual_run, ClipArtifact, JobOutput};
use chrono::{DateTime, Local};

/// Runs whose title contains the trimmed query, ignoring case; all when empty.
pub fn filter_runs<'a>(entries: &'a [JobHistoryEntry], query: &str) -> Vec<&'a JobHistoryEntry> {
    let query = crate::format::js_trim(query).to_lowercase();
    entries
        .iter()
        .filter(|entry| query.is_empty() || entry.video_title.to_lowercase().contains(&query))
        .collect()
}

/// The Library header's description: "3 runs · 12 clips", or the empty pitch.
pub fn library_summary(entries: &[JobHistoryEntry]) -> String {
    if entries.is_empty() {
        return "Every run you finish lands here.".into();
    }
    let clips: usize = entries.iter().map(|entry| entry.clip_count).sum();
    let plural = if entries.len() == 1 { "" } else { "s" };
    format!("{} run{plural} · {clips} clips", entries.len())
}

/// The highest-scoring clip; the first one on a tie.
pub fn best_clip(clips: &[ClipArtifact]) -> Option<&ClipArtifact> {
    clips
        .iter()
        .fold(None, |top: Option<&ClipArtifact>, clip| match top {
            Some(top) if clip.virality_score <= top.virality_score => Some(top),
            _ => Some(clip),
        })
}

/// file:///a/b.mp4 → /a/b.mp4; plain paths pass through. The engine writes the raw
/// path after the scheme (not percent-encoded), so it isn't decoded.
pub fn clip_file_path(url: &str) -> &str {
    url.strip_prefix("file://").unwrap_or(url)
}

/// Where a thumbnail is taken: the clip's middle, when its length is known.
pub fn thumbnail_seek(clip: &ClipArtifact) -> Option<f64> {
    (clip.duration_ms > 0.0).then(|| clip.duration_ms / 2000.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipSort {
    Score,
    Timeline,
}

/// The order clips list in: best first, or by position in the source. Manual runs
/// have no scores, so they always list in timeline order.
pub fn sorted_clips(output: &JobOutput, sort: ClipSort) -> Vec<&ClipArtifact> {
    let mut clips: Vec<&ClipArtifact> = output.clips.iter().collect();
    if sort == ClipSort::Score && !is_manual_run(output) {
        clips.sort_by(|a, b| b.virality_score.total_cmp(&a.virality_score));
    } else {
        clips.sort_by(|a, b| a.start_time_ms.total_cmp(&b.start_time_ms));
    }
    clips
}

/// The clip that gets the "Top pick" chip: only for scored runs of two or more.
pub fn top_pick(output: &JobOutput) -> Option<u64> {
    if is_manual_run(output) || output.clips.len() < 2 {
        return None;
    }
    best_clip(&output.clips).map(|clip| clip.clip_index)
}

/// "Clip 3" when the clip has no summary.
pub fn clip_title(clip: &ClipArtifact) -> String {
    match clip.summary.as_deref() {
        Some(summary) if !summary.is_empty() => summary.into(),
        _ => format!("Clip {}", clip.clip_index + 1),
    }
}

/// How the engine framed a vertical clip (its dominant layout).
pub fn layout_label(layout_type: &str) -> Option<&'static str> {
    Some(match layout_type {
        "talking_head" => "Speaker",
        "two_shot" => "Two people",
        "screen_cam" => "Screen + webcam",
        "screen" | "fit" => "Whole frame",
        "center_crop" => "Center crop",
        _ => return None,
    })
}

/// The badge for a clip smart framing failed to render.
pub fn fallback_label(render_fallback: Option<&str>) -> Option<&'static str> {
    match render_fallback? {
        "" => None,
        "letterbox_natural" => Some("Fallback: whole frame, no cuts"),
        _ => Some("Fallback: whole frame"),
    }
}

/// Whether the run's clips are vertical: the format the run asked for, else the
/// first thumbnail's shape (older runs don't record it), else vertical.
pub fn is_vertical(output: &JobOutput, measured_aspect: Option<f32>) -> bool {
    let requested = output
        .metrics
        .as_ref()
        .and_then(|m| m.requested_settings.as_ref())
        .and_then(|s| s.aspect_ratio.as_deref());
    match requested {
        Some("9:16") => true,
        Some("16:9") => false,
        _ => measured_aspect.map_or(true, |aspect| aspect < 1.0),
    }
}

/// The export speed when the run sped its clips up.
pub fn video_speed(output: &JobOutput) -> Option<f64> {
    output
        .metrics
        .as_ref()?
        .requested_settings
        .as_ref()?
        .video_speed
        .filter(|speed| *speed > 1.0)
}

/// The select-all checkbox: Some(true) all, Some(false) none, None some.
pub fn select_all_state(selected: usize, total: usize) -> Option<bool> {
    match selected {
        0 => Some(false),
        n if n == total => Some(true),
        _ => None,
    }
}

/// What an export reports: Ok(notice) when clips were copied, Err(message) when
/// none could be, None when the user cancelled the folder picker.
pub fn export_outcome(result: &ExportResult) -> Option<Result<String, &'static str>> {
    if result.success {
        let plural = if result.count == 1 { "" } else { "s" };
        let dest = result
            .dest_dir
            .as_deref()
            .map(|dir| format!(" to {}", crate::format::basename(dir)))
            .unwrap_or_default();
        let failed = if result.failed_count > 0 {
            format!("; {} could not be copied", result.failed_count)
        } else {
            String::new()
        };
        Some(Ok(format!(
            "Exported {} clip{plural}{dest}{failed}",
            result.count
        )))
    } else if result.failed_count > 0 {
        Some(Err("No clips could be copied to that folder."))
    } else {
        None
    }
}

/// Ease-out cubic count-up: `target` scaled by how far `elapsed_ms - delay_ms` is
/// through `duration_ms`.
pub fn count_up(target: f64, elapsed_ms: f64, delay_ms: f64, duration_ms: f64) -> f64 {
    let t = ((elapsed_ms - delay_ms) / duration_ms).clamp(0.0, 1.0);
    if t >= 1.0 {
        target
    } else {
        target * (1.0 - (1.0 - t).powi(3))
    }
}

/// RunStats' "Created" cell: ("3:42 PM", "Today" | "Yesterday" | "Wed, Oct 8, 2025").
pub fn created_labels(iso: &str) -> Option<(String, String)> {
    let date = DateTime::parse_from_rfc3339(iso)
        .ok()?
        .with_timezone(&Local);
    Some(created_labels_at(&date, &Local::now()))
}

fn created_labels_at(date: &DateTime<Local>, now: &DateTime<Local>) -> (String, String) {
    let day = match (now.date_naive() - date.date_naive()).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ => date.format("%a, %b %-d, %Y").to_string(),
    };
    (date.format("%-I:%M %p").to_string(), day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job_view::parse_job_output;
    use chrono::TimeZone;
    use serde_json::{json, Value};

    fn entry(title: &str, clips: usize) -> JobHistoryEntry {
        JobHistoryEntry {
            job_id: title.into(),
            date: String::new(),
            video_title: title.into(),
            clip_count: clips,
            status: "completed".into(),
            output_dir: String::new(),
            total_cost_usd: None,
            finished_at: None,
            duration_ms: None,
            error_message: None,
        }
    }

    fn clip(index: u64, start: u64, score: f64) -> Value {
        json!({ "clip_index": index, "s3_url": format!("file:///lib/clip_{index}.mp4"), "duration_ms": 4000,
            "start_time_ms": start, "end_time_ms": start + 4000, "virality_score": score })
    }

    fn run(clips: Vec<Value>, metrics: Value) -> JobOutput {
        parse_job_output(&json!({ "clips": clips, "metrics": metrics })).unwrap()
    }

    #[test]
    fn search_trims_and_ignores_case() {
        let entries = [entry("Podcast Ep 1", 3), entry("Keynote", 2)];
        assert_eq!(filter_runs(&entries, "  podCAST ").len(), 1);
        assert_eq!(filter_runs(&entries, "").len(), 2);
        assert!(filter_runs(&entries, "zzz").is_empty());
        assert_eq!(library_summary(&entries), "2 runs · 5 clips");
        assert_eq!(library_summary(&entries[..1]), "1 run · 3 clips");
        assert_eq!(library_summary(&[]), "Every run you finish lands here.");
    }

    #[test]
    fn sorting_ranks_scored_runs_and_keeps_manual_runs_in_order() {
        let output = run(
            vec![clip(0, 9000, 0.5), clip(1, 0, 0.9), clip(2, 4000, 0.9)],
            json!({}),
        );
        let order = |sort| {
            sorted_clips(&output, sort)
                .iter()
                .map(|c| c.clip_index)
                .collect::<Vec<_>>()
        };
        assert_eq!(order(ClipSort::Score), [1, 2, 0], "stable on ties");
        assert_eq!(order(ClipSort::Timeline), [1, 2, 0]);
        assert_eq!(best_clip(&output.clips).unwrap().clip_index, 1);
        assert_eq!(top_pick(&output), Some(1));

        let manual = run(
            vec![clip(0, 9000, 0.0), clip(1, 0, 0.0)],
            json!({ "planning_source": "manual" }),
        );
        let order: Vec<u64> = sorted_clips(&manual, ClipSort::Score)
            .iter()
            .map(|c| c.clip_index)
            .collect();
        assert_eq!(order, [1, 0]);
        assert_eq!(top_pick(&manual), None);
        assert_eq!(
            top_pick(&run(vec![clip(0, 0, 0.9)], json!({}))),
            None,
            "one clip has no top pick"
        );
    }

    #[test]
    fn selection_checkbox_states() {
        assert_eq!(select_all_state(0, 4), Some(false));
        assert_eq!(select_all_state(2, 4), None);
        assert_eq!(select_all_state(4, 4), Some(true));
        assert_eq!(select_all_state(0, 0), Some(false));
    }

    #[test]
    fn export_reports_copies_failures_and_cancels() {
        let result = |success, count, failed_count, dest: Option<&str>| ExportResult {
            success,
            count,
            failed_count,
            dest_dir: dest.map(Into::into),
        };
        assert_eq!(
            export_outcome(&result(true, 2, 1, Some("/Users/me/Exports"))),
            Some(Ok(
                "Exported 2 clips to Exports; 1 could not be copied".into()
            ))
        );
        assert_eq!(
            export_outcome(&result(true, 1, 0, None)),
            Some(Ok("Exported 1 clip".into()))
        );
        assert_eq!(
            export_outcome(&result(false, 0, 3, None)),
            Some(Err("No clips could be copied to that folder."))
        );
        assert_eq!(export_outcome(&result(false, 0, 0, None)), None);
    }

    #[test]
    fn clip_labels_and_shape() {
        let output = run(
            vec![clip(2, 0, 0.5)],
            json!({ "requested_settings": { "aspect_ratio": "16:9", "video_speed": 1.5 } }),
        );
        assert_eq!(clip_title(&output.clips[0]), "Clip 3");
        assert_eq!(clip_file_path(&output.clips[0].s3_url), "/lib/clip_2.mp4");
        assert_eq!(clip_file_path("/plain.mp4"), "/plain.mp4");
        assert_eq!(thumbnail_seek(&output.clips[0]), Some(2.0));
        assert!(!is_vertical(&output, Some(0.5)), "requested format wins");
        assert_eq!(video_speed(&output), Some(1.5));
        let old = run(vec![], json!({}));
        assert!(is_vertical(&old, None));
        assert!(!is_vertical(&old, Some(16.0 / 9.0)));
        assert_eq!(video_speed(&old), None);
        assert_eq!(layout_label("two_shot"), Some("Two people"));
        assert_eq!(layout_label("toString"), None);
        assert_eq!(
            fallback_label(Some("letterbox_natural")),
            Some("Fallback: whole frame, no cuts")
        );
        assert_eq!(
            fallback_label(Some("letterbox")),
            Some("Fallback: whole frame")
        );
        assert_eq!(fallback_label(None), None);
    }

    #[test]
    fn count_up_eases_out_and_lands() {
        assert_eq!(count_up(10.0, 0.0, 70.0, 700.0), 0.0);
        assert_eq!(count_up(10.0, 420.0, 70.0, 700.0), 8.75);
        assert_eq!(count_up(10.0, 2000.0, 70.0, 700.0), 10.0);
    }

    #[test]
    fn created_cell_names_recent_days() {
        let now = Local.with_ymd_and_hms(2026, 10, 9, 18, 0, 0).unwrap();
        let at = |d, h, m| Local.with_ymd_and_hms(2026, 10, d, h, m, 0).unwrap();
        assert_eq!(
            created_labels_at(&at(9, 15, 42), &now),
            ("3:42 PM".into(), "Today".into())
        );
        assert_eq!(created_labels_at(&at(8, 9, 5), &now).1, "Yesterday");
        assert_eq!(
            created_labels_at(&at(1, 0, 5), &now),
            ("12:05 AM".into(), "Thu, Oct 1, 2026".into())
        );
        assert_eq!(created_labels("not a date"), None);
    }
}
