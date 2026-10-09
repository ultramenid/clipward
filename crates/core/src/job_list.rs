//! The UI's copy of the session's jobs (use-job-store.ts): snapshots apply in
//! revision order, finished jobs stay bounded, and the focused job is cleared when
//! it goes away.
use crate::jobs::JobSnapshot;
use crate::shared::{is_active_job_status, MAX_FINISHED_JOBS};
use std::collections::HashMap;

#[derive(Default)]
pub struct JobList {
    pub jobs: HashMap<String, JobSnapshot>,
    /// The job the Jobs page shows in detail, or None for the list.
    pub focused: Option<String>,
}

impl JobList {
    /// Apply a snapshot; an older revision than the one held is ignored. True when
    /// the list changed.
    pub fn upsert(&mut self, job: JobSnapshot) -> bool {
        let newer = self
            .jobs
            .get(&job.id)
            .is_none_or(|current| job.revision >= current.revision);
        if newer {
            self.jobs.insert(job.id.clone(), job);
            self.bound();
        }
        newer
    }

    /// Merge a full list (startup), keeping any newer live update already held.
    pub fn hydrate(&mut self, list: Vec<JobSnapshot>) {
        for job in list {
            if self
                .jobs
                .get(&job.id)
                .is_none_or(|current| job.revision >= current.revision)
            {
                self.jobs.insert(job.id.clone(), job);
            }
        }
        self.bound();
    }

    pub fn remove(&mut self, id: &str) {
        self.jobs.remove(id);
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
        }
    }

    /// Keep the newest MAX_FINISHED_JOBS finished jobs; active ones always stay.
    fn bound(&mut self) {
        let mut finished: Vec<(String, String)> = self
            .jobs
            .values()
            .filter(|job| !is_active_job_status(&job.status))
            .map(|job| {
                let at = job.finished_at.clone().unwrap_or_else(|| job.queued_at.clone());
                (at, job.id.clone())
            })
            .collect();
        finished.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, id) in finished.into_iter().skip(MAX_FINISHED_JOBS) {
            self.jobs.remove(&id);
        }
        if self.focused.as_ref().is_some_and(|id| !self.jobs.contains_key(id)) {
            self.focused = None;
        }
    }

    /// Queued and running jobs, oldest first (the order they run in).
    pub fn active(&self) -> Vec<&JobSnapshot> {
        let mut active: Vec<&JobSnapshot> = self
            .jobs
            .values()
            .filter(|job| is_active_job_status(&job.status))
            .collect();
        active.sort_by(|a, b| a.queued_at.cmp(&b.queued_at));
        active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clip_engine::ClipJobRequest;

    fn snapshot(id: &str, revision: u64, status: &str) -> JobSnapshot {
        let request: ClipJobRequest = serde_json::from_value(serde_json::json!({
            "videoUrl": "/source.mp4", "autoClipCount": true, "aspectRatio": "9:16",
            "layoutStyle": "smart", "layoutVision": false, "pacing": "natural",
            "includeCaptions": true, "captionPreset": "pop"
        }))
        .expect("a minimal request");
        JobSnapshot {
            id: id.into(),
            revision,
            request,
            status: status.into(),
            percent: 0.0,
            step: String::new(),
            clips_done: 0,
            clips_total: 0,
            error: None,
            error_hint: None,
            failure_code: None,
            failure_stage: None,
            http_status: None,
            output: None,
            output_dir: format!("/clips/{id}"),
            queued_at: "2026-01-01T00:00:00.000Z".into(),
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn job_snapshots_apply_in_revision_order_so_a_late_update_cannot_undo_a_newer_one() {
        let mut list = JobList::default();
        list.upsert(JobSnapshot { percent: 75.0, ..snapshot("one", 3, "rendering") });
        list.upsert(JobSnapshot { percent: 20.0, ..snapshot("one", 2, "transcribing") });
        assert_eq!(list.jobs["one"].status, "rendering");
        assert_eq!(list.jobs["one"].percent, 75.0);
        list.upsert(snapshot("one", 4, "cancelled"));
        list.upsert(JobSnapshot { percent: 90.0, ..snapshot("one", 3, "rendering") });
        assert_eq!(list.jobs["one"].status, "cancelled");
    }

    #[test]
    fn hydrating_adds_missing_jobs_without_replacing_newer_live_updates() {
        let mut list = JobList::default();
        // An update pushed while the list request was in flight is newer than the list.
        list.upsert(JobSnapshot { percent: 100.0, ..snapshot("live", 5, "completed") });
        list.hydrate(vec![
            JobSnapshot { percent: 80.0, ..snapshot("live", 4, "rendering") },
            snapshot("other", 1, "pending"),
        ]);
        assert_eq!(list.jobs["live"].status, "completed");
        assert_eq!(list.jobs["other"].status, "pending");
    }

    #[test]
    fn removing_the_focused_job_returns_the_jobs_page_to_its_list() {
        let mut list = JobList::default();
        list.upsert(snapshot("one", 1, "failed"));
        list.focused = Some("one".into());
        list.remove("one");
        assert_eq!(list.focused, None);
        assert!(!list.jobs.contains_key("one"));
    }

    #[test]
    fn finished_job_snapshots_stay_bounded_while_active_jobs_remain_visible() {
        let mut list = JobList::default();
        list.upsert(snapshot("active", 1, "rendering"));
        for index in 0..60 {
            list.upsert(JobSnapshot {
                finished_at: Some(format!("2026-01-01T00:{index:02}:00.000Z")),
                ..snapshot(&format!("done-{index}"), 1, "completed")
            });
        }
        assert_eq!(list.jobs.len(), 51);
        assert!(list.jobs.contains_key("active"));
        assert!(!list.jobs.contains_key("done-0"));
        assert!(list.jobs.contains_key("done-59"));
    }

    #[test]
    fn active_jobs_list_oldest_first() {
        let mut list = JobList::default();
        list.upsert(JobSnapshot { queued_at: "2026-01-02T00:00:00.000Z".into(), ..snapshot("b", 1, "queued") });
        list.upsert(snapshot("a", 1, "rendering"));
        list.upsert(snapshot("c", 1, "completed"));
        let ids: Vec<&str> = list.active().iter().map(|j| j.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }
}
