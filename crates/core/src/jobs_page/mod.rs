//! The Jobs page's logic (JobsPage.tsx, JobProgress.tsx): history filters, search
//! and counts, queue positions, timers and the progress stepper. The wait-screen
//! games are in `arcade`.
use crate::file_manager::JobHistoryEntry;
use crate::format::js_trim;
use crate::jobs::JobSnapshot;
use std::collections::HashSet;

pub mod arcade;

/// The Previous panel's filter chips: "all" or a history status, and its label.
pub const FILTERS: [(&str, &str); 6] = [
    ("all", "All"),
    ("completed", "Completed"),
    ("failed", "Failed"),
    ("cancelled", "Cancelled"),
    ("interrupted", "Interrupted"),
    ("incomplete", "Older unfinished"),
];

/// A history status as the rows write it.
pub fn status_label(status: &str) -> &'static str {
    match status {
        "completed" => "Completed",
        "running" => "Running",
        "failed" => "Failed",
        "cancelled" => "Cancelled",
        "interrupted" => "Interrupted",
        _ => "Unfinished",
    }
}

/// Earlier runs: queued and running jobs show in Active, so their disk records
/// (and any other still-running record) would duplicate them.
pub fn previous<'a>(entries: &'a [JobHistoryEntry], live: &HashSet<&str>) -> Vec<&'a JobHistoryEntry> {
    entries
        .iter()
        .filter(|entry| !live.contains(entry.job_id.as_str()) && entry.status != "running")
        .collect()
}

/// The runs a filter chip and the search box (title or job id, any case) let through.
pub fn visible<'a>(previous: &[&'a JobHistoryEntry], filter: &str, query: &str) -> Vec<&'a JobHistoryEntry> {
    let query = js_trim(query).to_lowercase();
    previous
        .iter()
        .copied()
        .filter(|entry| {
            (filter == "all" || entry.status == filter)
                && (query.is_empty()
                    || entry.video_title.to_lowercase().contains(&query)
                    || entry.job_id.to_lowercase().contains(&query))
        })
        .collect()
}

/// How many runs a filter chip holds.
pub fn count(previous: &[&JobHistoryEntry], filter: &str) -> usize {
    previous
        .iter()
        .filter(|entry| filter == "all" || entry.status == filter)
        .count()
}

/// A queued job's place in line (1-based), from the active jobs in run order.
pub fn queue_position(active: &[&JobSnapshot], id: &str) -> Option<usize> {
    active
        .iter()
        .filter(|job| job.status == "queued")
        .position(|job| job.id == id)
        .map(|index| index + 1)
}

/// Milliseconds since an ISO timestamp, never negative; 0 when unreadable.
pub fn elapsed_ms(since: &str, now_ms: i64) -> f64 {
    chrono::DateTime::parse_from_rfc3339(since)
        .map(|at| (now_ms - at.timestamp_millis()).max(0) as f64)
        .unwrap_or(0.0)
}

/// Milliseconds since the epoch, now.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// When a job's timer starts: its start, or its queueing while it waits.
pub fn timer_start(job: &JobSnapshot) -> &str {
    job.started_at.as_deref().unwrap_or(&job.queued_at)
}

/// History dates the engine couldn't read are written as the epoch.
pub fn is_dated(date: &str) -> bool {
    !date.starts_with("1970-")
}

/// One stage of the progress stepper: label, Lucide icon and the job statuses it covers.
pub struct Step {
    pub label: &'static str,
    pub icon: &'static str,
    pub statuses: &'static [&'static str],
}

pub const STEPS: [Step; 4] = [
    Step { label: "Download", icon: "download", statuses: &["downloading"] },
    Step { label: "Transcribe", icon: "file-text", statuses: &["transcribing"] },
    Step { label: "Find moments", icon: "sparkles", statuses: &["planning"] },
    Step { label: "Render", icon: "film", statuses: &["rendering", "uploading"] },
];

/// Manual runs skip "Find moments": the user typed them.
pub fn steps_for(manual: bool) -> Vec<&'static Step> {
    STEPS
        .iter()
        .filter(|step| !(manual && step.statuses.contains(&"planning")))
        .collect()
}

/// The step a status is in: all of them done when completed, None before the first.
pub fn step_index(steps: &[&Step], status: &str) -> Option<usize> {
    if status == "completed" {
        return Some(steps.len());
    }
    steps.iter().position(|step| step.statuses.contains(&status))
}

/// A job status as the progress views name it (STAGE_LABELS in JobProgress.tsx).
pub fn stage_label(status: &str) -> &'static str {
    match status {
        "queued" => "Queued",
        "pending" => "Starting",
        "downloading" => "Downloading",
        "transcribing" => "Transcribing",
        "planning" => "Finding moments",
        "rendering" => "Rendering clips",
        "uploading" => "Saving clips",
        "completed" => "Done",
        "failed" => "Failed",
        "cancelled" => "Cancelled",
        _ => "Working",
    }
}

/// The Active panel's summary line.
pub fn active_summary(running: usize, queued: usize, max_parallel: usize) -> String {
    let queued = if queued > 0 { format!(" · {queued} queued") } else { String::new() };
    format!("{running} running{queued} · up to {max_parallel} at once")
}

/// The confirm dialog's body before a job is deleted.
pub fn delete_body(title: &str, active: bool) -> String {
    if active {
        format!("“{title}” stops, and its folder is deleted with any clips made so far. This can’t be undone.")
    } else {
        format!("“{title}” and its clips are deleted from disk. This can’t be undone.")
    }
}

/// "6 clips" in a finished run's row.
pub fn clip_count(count: usize) -> String {
    format!("{count} clip{}", if count == 1 { "" } else { "s" })
}

/// "3 of 5 clips rendered" under the progress ring.
pub fn clips_rendered(done: u32, total: u32) -> String {
    format!("{done} of {total} clip{} rendered", if total == 1 { "" } else { "s" })
}

/// The failure card's last line.
pub fn failure_code_line(code: &str, http_status: Option<u16>) -> String {
    match http_status {
        Some(status) if status > 0 => format!("Failure code: {code} · HTTP {status}"),
        _ => format!("Failure code: {code}"),
    }
}

/// The whole percent the progress ring writes.
pub fn whole_percent(percent: f64) -> f64 {
    percent.min(100.0).round()
}

/// Two history reads hold the same runs, field for field (the record has no `PartialEq`).
pub fn same_entries(a: &[JobHistoryEntry], b: &[JobHistoryEntry]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.job_id == b.job_id
                && a.date == b.date
                && a.video_title == b.video_title
                && a.clip_count == b.clip_count
                && a.status == b.status
                && a.output_dir == b.output_dir
                && a.total_cost_usd == b.total_cost_usd
                && a.finished_at == b.finished_at
                && a.duration_ms == b.duration_ms
                && a.error_message == b.error_message
        })
}

/// A CSS `cubic-bezier(x1, y1, x2, y2)` timing function, for animations that
/// should ease as the stylesheet did (Tailwind's `ease-out` is (0, 0, 0.2, 1)).
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32) -> impl Fn(f32) -> f32 + Copy {
    move |t: f32| {
        let t = t.clamp(0.0, 1.0);
        let curve = |a: f32, b: f32, s: f32| {
            let r = 1.0 - s;
            3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
        };
        // Solve x(s) = t by bisection; the curve is monotonic in x for 0 <= x1, x2 <= 1.
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if curve(x1, x2, mid) < t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        curve(y1, y2, (lo + hi) / 2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, title: &str, status: &str) -> JobHistoryEntry {
        JobHistoryEntry {
            job_id: id.into(),
            date: "2026-01-02T03:04:05.000Z".into(),
            video_title: title.into(),
            clip_count: 0,
            status: status.into(),
            output_dir: format!("/runs/{id}"),
            total_cost_usd: None,
            finished_at: None,
            duration_ms: None,
            error_message: None,
        }
    }

    #[test]
    fn filters_searches_and_counts_previous_runs() {
        let entries = vec![
            entry("a1", "Podcast Ep 1", "completed"),
            entry("b2", "Talk", "failed"),
            entry("c3", "Live now", "running"),
            entry("d4", "Queued one", "incomplete"),
            entry("e5", "Another podcast", "completed"),
        ];
        let live: HashSet<&str> = ["d4"].into();
        let previous = previous(&entries, &live);
        let ids: Vec<&str> = previous.iter().map(|e| e.job_id.as_str()).collect();
        assert_eq!(ids, ["a1", "b2", "e5"]);
        assert_eq!(count(&previous, "all"), 3);
        assert_eq!(count(&previous, "completed"), 2);
        assert_eq!(count(&previous, "interrupted"), 0);
        assert_eq!(visible(&previous, "all", "  PODCAST ").len(), 2);
        assert_eq!(visible(&previous, "failed", "podcast").len(), 0);
        assert_eq!(visible(&previous, "all", "B2")[0].job_id, "b2");
        assert_eq!(visible(&previous, "completed", "").len(), 2);
    }

    #[test]
    fn steps_skip_planning_for_manual_runs() {
        let ai = steps_for(false);
        let manual = steps_for(true);
        assert_eq!(ai.len(), 4);
        assert_eq!(manual.iter().map(|s| s.label).collect::<Vec<_>>(), ["Download", "Transcribe", "Render"]);
        assert_eq!(step_index(&ai, "planning"), Some(2));
        assert_eq!(step_index(&manual, "uploading"), Some(2));
        assert_eq!(step_index(&ai, "completed"), Some(4));
        assert_eq!(step_index(&ai, "queued"), None);
    }

    #[test]
    fn elapsed_and_dates() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05.000Z").unwrap();
        let now = at.timestamp_millis() + 61_500;
        assert_eq!(elapsed_ms("2026-01-02T03:04:05.000Z", now), 61_500.0);
        assert_eq!(elapsed_ms("2026-01-02T03:04:05.000Z", now - 100_000), 0.0);
        assert_eq!(elapsed_ms("garbage", now), 0.0);
        assert!(!is_dated("1970-01-01T00:00:00.000Z"));
        assert!(is_dated("2026-01-02T03:04:05.000Z"));
        assert_eq!(status_label("incomplete"), "Unfinished");
    }

    #[test]
    fn labels_and_copy() {
        assert_eq!(stage_label("uploading"), "Saving clips");
        assert_eq!(stage_label("planning"), "Finding moments");
        assert_eq!(stage_label("weird"), "Working");
        assert_eq!(active_summary(2, 0, 2), "2 running · up to 2 at once");
        assert_eq!(active_summary(1, 3, 2), "1 running · 3 queued · up to 2 at once");
        assert!(delete_body("Talk", true).starts_with("“Talk” stops"));
        assert_eq!(delete_body("Talk", false), "“Talk” and its clips are deleted from disk. This can’t be undone.");
        assert_eq!(clip_count(1), "1 clip");
        assert_eq!(clip_count(0), "0 clips");
        assert_eq!(clips_rendered(1, 1), "1 of 1 clip rendered");
        assert_eq!(clips_rendered(2, 5), "2 of 5 clips rendered");
        assert_eq!(failure_code_line("E1", Some(429)), "Failure code: E1 · HTTP 429");
        assert_eq!(failure_code_line("E1", None), "Failure code: E1");
        assert_eq!(whole_percent(42.5), 43.0);
        assert_eq!(whole_percent(130.0), 100.0);
    }

    #[test]
    fn queue_positions_count_only_queued_jobs() {
        let job = |id: &str, status: &str| {
            let request: clip_engine::ClipJobRequest = serde_json::from_value(serde_json::json!({
                "videoUrl": "/source.mp4", "autoClipCount": true, "aspectRatio": "9:16",
                "layoutStyle": "smart", "layoutVision": false, "pacing": "natural",
                "includeCaptions": true, "captionPreset": "pop"
            }))
            .unwrap();
            JobSnapshot {
                id: id.into(), revision: 1, request, status: status.into(), percent: 0.0, step: String::new(),
                clips_done: 0, clips_total: 0, error: None, error_hint: None, failure_code: None,
                failure_stage: None, http_status: None, output: None, output_dir: String::new(),
                queued_at: String::new(), started_at: None, finished_at: None,
            }
        };
        let jobs = [job("a", "rendering"), job("b", "queued"), job("c", "pending"), job("d", "queued")];
        let refs: Vec<&JobSnapshot> = jobs.iter().collect();
        assert_eq!(queue_position(&refs, "b"), Some(1));
        assert_eq!(queue_position(&refs, "d"), Some(2));
        assert_eq!(queue_position(&refs, "a"), None);
    }

    #[test]
    fn entries_compare_field_for_field() {
        let a = vec![entry("a1", "One", "completed")];
        let mut b = a.clone();
        assert!(same_entries(&a, &b));
        b[0].clip_count = 3;
        assert!(!same_entries(&a, &b));
        assert!(!same_entries(&a, &[]));
    }

    #[test]
    fn css_timing_functions() {
        let linear = cubic_bezier(0.0, 0.0, 1.0, 1.0);
        assert!((linear(0.3) - 0.3).abs() < 1e-3);
        let ease_out = cubic_bezier(0.0, 0.0, 0.2, 1.0);
        assert!(ease_out(0.0).abs() < 1e-4);
        assert!((ease_out(1.0) - 1.0).abs() < 1e-4);
        assert!(ease_out(0.5) > 0.8, "front-loaded");
        assert!(ease_out(-1.0).abs() < 1e-4 && (ease_out(2.0) - 1.0).abs() < 1e-4);
    }
}
