//! Every clipping job in this app session: queued, running and finished.
//!
//! Up to MAX_PARALLEL_JOBS run at once, each as an in-process tokio task calling
//! `clip_engine::pipeline::run_job`; the rest wait in FIFO order. The renderer
//! mirrors this list through `jobs:update` snapshots, so a reload picks the jobs
//! back up with `jobs_list`.
use crate::file_manager::OUTPUT_FILES;
use crate::job_output::parse_job_output;
use crate::logger;
use crate::run_history::{finish_run_record, FailureDetails};
use crate::settings::vocabulary_terms;
use crate::shared::{is_active_job_status, MAX_FINISHED_JOBS, MAX_PARALLEL_JOBS};
use crate::tools::{resolve_binary, tool_available, Tool};
use crate::Core;
use clip_engine::{ClipJobRequest, EngineConfig, EngineError, Progress, ProgressFn, Stage};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: String,
    pub revision: u64,
    pub request: ClipJobRequest,
    pub status: String,
    pub percent: f64,
    pub step: String,
    pub clips_done: u32,
    pub clips_total: u32,
    pub error: Option<String>,
    pub error_hint: Option<String>,
    pub failure_code: Option<String>,
    pub failure_stage: Option<String>,
    pub http_status: Option<u16>,
    pub output: Option<Value>,
    pub output_dir: String,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

struct Tracked {
    snapshot: JobSnapshot,
    output_directory: PathBuf,
    cancel: CancellationToken,
}

#[derive(Default)]
struct Inner {
    /// Oldest first.
    jobs: Vec<Tracked>,
    queue: VecDeque<String>,
    /// Jobs whose engine task has started and not yet returned.
    running: HashSet<String>,
    quitting: bool,
}

impl Inner {
    fn get_mut(&mut self, id: &str) -> Option<&mut Tracked> {
        self.jobs.iter_mut().find(|job| job.snapshot.id == id)
    }
}

#[derive(Default)]
pub struct JobManager {
    inner: Mutex<Inner>,
}

static UNSAFE_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)https?://|[\\/]|(?:token|secret|key)\s*[:=]").unwrap());

/// Engine text shown in the UI: never a URL, path or credential.
fn safe_text(value: &str) -> String {
    if value.chars().count() > 300 || UNSAFE_TEXT.is_match(value) {
        "The clipping engine could not complete this step.".into()
    } else {
        value.to_string()
    }
}

fn stage_status(stage: Stage) -> &'static str {
    match stage {
        Stage::Pending => "pending",
        Stage::Downloading => "downloading",
        Stage::Transcribing => "transcribing",
        Stage::Planning => "planning",
        Stage::Rendering => "rendering",
    }
}

fn emit(core: &Core, snapshots: Vec<JobSnapshot>) {
    for snapshot in &snapshots {
        (core.on_job)(snapshot);
    }
}

/// How a job ended, with the fields to patch onto its snapshot.
pub enum Outcome {
    Completed(Value),
    Failed {
        message: String,
        hint: Option<String>,
    },
    Cancelled,
}

impl JobManager {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Apply `patch` to an active job and return the new snapshot to broadcast.
    fn patch(
        inner: &mut Inner,
        id: &str,
        patch: impl FnOnce(&mut JobSnapshot),
    ) -> Option<JobSnapshot> {
        let job = inner.get_mut(id)?;
        patch(&mut job.snapshot);
        job.snapshot.revision += 1;
        Some(job.snapshot.clone())
    }

    fn finish_locked(inner: &mut Inner, id: &str, outcome: Outcome) -> Option<JobSnapshot> {
        let snapshot = Self::patch(inner, id, |s| {
            s.finished_at = Some(crate::paths::iso_now());
            match outcome {
                Outcome::Completed(output) => {
                    let total = output["total_clips"].as_u64().unwrap_or(0) as u32;
                    s.status = "completed".into();
                    s.output = Some(output);
                    s.percent = 100.0;
                    s.step = "Complete".into();
                    s.clips_done = total;
                    s.clips_total = total;
                }
                Outcome::Failed { message, hint } => {
                    s.status = "failed".into();
                    s.error = Some(message);
                    s.error_hint = hint;
                    s.step = "Failed".into();
                }
                Outcome::Cancelled => {
                    s.status = "cancelled".into();
                    s.step = "Cancelled".into();
                }
            }
        });
        // Keep only the newest finished jobs in this session; older runs remain on disk.
        let finished = inner
            .jobs
            .iter()
            .filter(|j| !is_active_job_status(&j.snapshot.status))
            .count();
        let mut excess = finished.saturating_sub(MAX_FINISHED_JOBS);
        inner.jobs.retain(|job| {
            if excess > 0 && !is_active_job_status(&job.snapshot.status) {
                excess -= 1;
                false
            } else {
                true
            }
        });
        snapshot
    }

    /// Track a new job (its run record already exists) and start it as soon as a slot is free.
    pub fn enqueue(
        &self,
        app: &Arc<Core>,
        id: String,
        request: ClipJobRequest,
        output_directory: PathBuf,
    ) -> JobSnapshot {
        let snapshot = JobSnapshot {
            id: id.clone(),
            revision: 0,
            request,
            status: "queued".into(),
            percent: 0.0,
            step: "Waiting for a free slot".into(),
            clips_done: 0,
            clips_total: 0,
            error: None,
            error_hint: None,
            failure_code: None,
            failure_stage: None,
            http_status: None,
            output: None,
            output_dir: output_directory.join(&id).to_string_lossy().into_owned(),
            queued_at: crate::paths::iso_now(),
            started_at: None,
            finished_at: None,
        };
        {
            let mut inner = self.lock();
            inner.jobs.push(Tracked {
                snapshot: snapshot.clone(),
                output_directory,
                cancel: CancellationToken::new(),
            });
            inner.queue.push_back(id.clone());
        }
        emit(app, vec![snapshot.clone()]);
        self.pump(app);
        self.lock()
            .get_mut(&id)
            .map_or(snapshot, |job| job.snapshot.clone())
    }

    fn pump(&self, app: &Arc<Core>) {
        let mut started = Vec::new();
        let mut snapshots = Vec::new();
        {
            let mut inner = self.lock();
            while !inner.quitting && inner.running.len() < MAX_PARALLEL_JOBS {
                let Some(id) = inner.queue.pop_front() else {
                    break;
                };
                if inner
                    .get_mut(&id)
                    .is_none_or(|job| job.snapshot.status != "queued")
                {
                    continue;
                }
                inner.running.insert(id.clone());
                if let Some(snapshot) = Self::patch(&mut inner, &id, |s| {
                    s.status = "pending".into();
                    s.step = "Starting…".into();
                    s.started_at = Some(crate::paths::iso_now());
                }) {
                    snapshots.push(snapshot);
                }
                logger::info(
                    "jobs.start",
                    json!({ "jobId": id, "running": inner.running.len(), "queued": inner.queue.len() }),
                );
                started.push(id);
            }
        }
        emit(app, snapshots);
        for id in started {
            let app = app.clone();
            app.runtime.clone().spawn(async move {
                run_tracked_job(app.clone(), id.clone()).await;
                if app.jobs.lock().running.remove(&id) {
                    app.jobs.pump(&app);
                }
            });
        }
    }

    fn start_info(&self, id: &str) -> Option<(ClipJobRequest, PathBuf, CancellationToken)> {
        let mut inner = self.lock();
        let job = inner.get_mut(id)?;
        Some((
            job.snapshot.request.clone(),
            job.output_directory.clone(),
            job.cancel.clone(),
        ))
    }

    fn progress(&self, app: &Core, id: &str, progress: &Progress) {
        let snapshot = {
            let mut inner = self.lock();
            if inner
                .get_mut(id)
                .is_none_or(|job| !is_active_job_status(&job.snapshot.status))
            {
                return;
            }
            Self::patch(&mut inner, id, |s| {
                s.status = stage_status(progress.stage).into();
                if progress.percent.is_finite() {
                    s.percent = progress.percent.clamp(0.0, 100.0);
                }
                s.step = safe_text(&progress.step);
                s.clips_done = progress.clips_done;
                s.clips_total = progress.clips_total;
            })
        };
        emit(app, snapshot.into_iter().collect());
    }

    /// Finish an active job. Returns false when it already ended (e.g. cancelled).
    fn finish(&self, app: &Core, id: &str, outcome: Outcome) -> bool {
        let snapshot = {
            let mut inner = self.lock();
            if inner
                .get_mut(id)
                .is_none_or(|job| !is_active_job_status(&job.snapshot.status))
            {
                return false;
            }
            Self::finish_locked(&mut inner, id, outcome)
        };
        emit(app, snapshot.into_iter().collect());
        true
    }

    /// Cancel a queued or running job. Returns false when there is nothing to cancel.
    pub fn cancel(&self, app: &Core, id: &str) -> bool {
        let snapshot = {
            let mut inner = self.lock();
            let Some(job) = inner.get_mut(id) else {
                return false;
            };
            if !is_active_job_status(&job.snapshot.status) {
                return false;
            }
            let was_queued = job.snapshot.status == "queued";
            let output_directory = job.output_directory.clone();
            // A running job's slot frees when its engine task actually returns.
            job.cancel.cancel();
            if was_queued {
                inner.queue.retain(|queued| queued != id);
            }
            if finish_run_record(
                &output_directory,
                id,
                "cancelled",
                None,
                FailureDetails::default(),
            )
            .is_err()
            {
                logger::warn("job.history.writeFailed", json!({ "jobId": id }));
            }
            logger::info("job.cancel", json!({ "jobId": id, "queued": was_queued }));
            Self::finish_locked(&mut inner, id, Outcome::Cancelled)
        };
        emit(app, snapshot.into_iter().collect());
        true
    }

    /// Forget a finished job for this session; its run folder stays in the library.
    pub fn dismiss(&self, id: &str) -> bool {
        let mut inner = self.lock();
        let before = inner.jobs.len();
        inner
            .jobs
            .retain(|job| job.snapshot.id != id || is_active_job_status(&job.snapshot.status));
        inner.jobs.len() != before
    }

    /// Delete a job and its run folder. An active job is cancelled first, and the folder
    /// is only removed once its engine task has returned (child processes killed, work
    /// folder cleaned), so nothing keeps writing into a deleted run. `library` is the
    /// output folder for runs this session does not track.
    pub async fn delete(&self, app: &Core, id: &str, library: PathBuf) -> Result<(), String> {
        self.cancel(app, id);
        for _ in 0..300 {
            if !self.lock().running.contains(id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let output_directory = {
            let mut inner = self.lock();
            if inner.running.contains(id) {
                logger::warn("job.delete.stillStopping", json!({ "jobId": id }));
                return Err(
                    "This job is still stopping. Try deleting it again in a moment.".into(),
                );
            }
            let tracked = inner
                .jobs
                .iter()
                .position(|job| job.snapshot.id == id)
                .map(|index| inner.jobs.remove(index).output_directory);
            tracked.unwrap_or(library)
        };
        let deleted = crate::run_history::delete_run_folder(&output_directory, id);
        logger::info("job.delete", json!({ "jobId": id, "ok": deleted.is_ok() }));
        deleted
    }

    /// Every job this session, newest first.
    pub fn list(&self) -> Vec<JobSnapshot> {
        self.lock()
            .jobs
            .iter()
            .rev()
            .map(|job| job.snapshot.clone())
            .collect()
    }

    /// Queued and running job IDs, so run history can tell live runs from interrupted ones.
    pub fn live_ids(&self) -> HashSet<String> {
        self.lock()
            .jobs
            .iter()
            .filter(|job| is_active_job_status(&job.snapshot.status))
            .map(|job| job.snapshot.id.clone())
            .collect()
    }

    /// On quit: queued jobs will never start, so record them as cancelled rather than
    /// interrupted, and cancel running ones. Returns how many engine tasks are still running.
    pub fn stop_all_for_quit(&self) -> usize {
        let mut inner = self.lock();
        if inner.quitting {
            return inner.running.len();
        }
        inner.quitting = true;
        let queued: Vec<String> = inner.queue.drain(..).collect();
        for job in &inner.jobs {
            if queued.contains(&job.snapshot.id) {
                let _ = finish_run_record(
                    &job.output_directory,
                    &job.snapshot.id,
                    "cancelled",
                    None,
                    FailureDetails::default(),
                );
            }
            if inner.running.contains(&job.snapshot.id) {
                job.cancel.cancel();
            }
        }
        inner.running.len()
    }

    pub fn running_count(&self) -> usize {
        self.lock().running.len()
    }
}

/// Private per-job scratch space under the app data folder.
fn work_root(user_data: &Path) -> std::io::Result<PathBuf> {
    crate::paths::private_dir(&user_data.join("work"))
}

fn valid_job_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Remove temporary media left by a run that was terminated before cleanup.
pub fn clean_stale_workspaces(user_data: &Path, active: &HashSet<String>) {
    let Ok(root) = work_root(user_data) else {
        logger::warn("job.work.staleCleanupFailed", json!({}));
        return;
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if valid_job_id(&name)
            && !active.contains(&name)
            && entry.file_type().is_ok_and(|t| t.is_dir())
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Development builds keep failure detail in a private per-job log; packaged builds
/// never write it, since raw engine errors can contain provider URLs and local paths.
fn write_dev_engine_log(logs: &Path, id: &str, detail: &str) {
    if !cfg!(debug_assertions) || !valid_job_id(id) {
        return;
    }
    let dir = logs.join("engine");
    if crate::paths::private_dir(&dir).is_ok() {
        let path = dir.join(format!("{id}.log"));
        let _ = std::fs::remove_file(&path);
        if let Ok(mut file) = crate::paths::open_private_new(&path) {
            use std::io::Write;
            let _ = file.write_all(detail.as_bytes());
        }
    }
}

async fn run_tracked_job(app: Arc<Core>, id: String) {
    let state = &*app;
    let Some((request, output_directory, cancel)) = state.jobs.start_info(&id) else {
        return;
    };
    let started = Instant::now();

    let report_error = |message: String, hint: Option<String>| {
        let message = safe_text(&message);
        let hint = hint.map(|h| safe_text(&h));
        if finish_run_record(
            &output_directory,
            &id,
            "failed",
            Some(&message),
            FailureDetails::default(),
        )
        .is_err()
        {
            logger::warn("job.history.writeFailed", json!({ "jobId": id }));
        }
        state
            .jobs
            .finish(&app, &id, Outcome::Failed { message, hint });
    };

    // Provider and Whisper model are read when the job starts, so a queued job uses
    // the Settings in effect when it actually runs. A queued job keeps the output
    // folder chosen when its run record was created.
    let settings = match state.settings.load() {
        Ok(settings) => settings,
        Err(message) => return report_error(message, Some("Open Settings and retry.".into())),
    };
    let paths = &state.paths;
    let whisper_model_path = state.whisper.downloaded(&settings.whisper_model);
    let cli_path = crate::ai_providers::resolve_cli_path(&paths.home, &settings.ai_provider);
    let Some(whisper_model_path) = whisper_model_path else {
        return report_error(
            "The Whisper transcription model is not downloaded.".into(),
            Some("Open Settings to download the model or choose an AI provider.".into()),
        );
    };
    let manual = request.manual_clips.is_some();
    if !manual && settings.ai_provider.starts_with("cli:") && cli_path.is_none() {
        return report_error(
            "The selected AI CLI is not installed.".into(),
            Some("Open Settings to download the model or choose an AI provider.".into()),
        );
    }
    let work_dir = match work_root(&paths.user_data)
        .and_then(|root| crate::paths::private_dir(&root.join(&id)))
    {
        Ok(dir) => dir,
        Err(_) => {
            return report_error(
                "Clipward could not create a private temporary work folder.".into(),
                None,
            )
        }
    };
    let ytdlp = resolve_binary(paths, Tool::YtDlp);
    let run_dir = output_directory.join(&id);
    let config = EngineConfig {
        ffmpeg: resolve_binary(paths, Tool::Ffmpeg),
        ffprobe: resolve_binary(paths, Tool::Ffprobe),
        ytdlp: tool_available(&ytdlp).then_some(ytdlp),
        assets_dir: paths.assets_dir(),
        whisper_model_path,
        ai: settings.engine_ai(cli_path),
        custom_vocabulary: vocabulary_terms(&settings.custom_vocabulary),
        work_dir: work_dir.clone(),
        output_dir: run_dir.clone(),
    };
    logger::info(
        "job.start",
        json!({
            "jobId": id,
            "sourceType": if request.video_url.starts_with("http") { "remote" } else { "local" },
            "maxClips": request.max_clips,
            "autoClipCount": request.auto_clip_count,
            "aspectRatio": request.aspect_ratio,
            "captionPreset": request.caption_preset,
            "provider": if manual { "none" } else if settings.ai_provider.starts_with("cli:") { "cli" } else { settings.ai_provider.as_str() },
            "manualClips": request.manual_clips.as_ref().map(Vec::len),
        }),
    );

    // Coalesce progress: stage and clip-count changes go out at once, other updates
    // at most every 100ms.
    let throttle = Mutex::new((None::<Instant>, None::<Stage>, 0u32));
    let progress_app = app.clone();
    let progress_id = id.clone();
    let progress: ProgressFn = Arc::new(move |update: Progress| {
        {
            let Ok(mut last) = throttle.lock() else {
                return;
            };
            let (at, stage, clips) = *last;
            let changed = stage != Some(update.stage) || clips != update.clips_done;
            if !changed && at.is_some_and(|at| at.elapsed() < Duration::from_millis(100)) {
                return;
            }
            *last = (Some(Instant::now()), Some(update.stage), update.clips_done);
        }
        progress_app
            .jobs
            .progress(&progress_app, &progress_id, &update);
    });

    // A separate task, so a panic inside the engine fails this job instead of the app.
    let engine = tokio::spawn(clip_engine::pipeline::run_job(
        request,
        config,
        progress,
        cancel.clone(),
    ));
    let result = engine.await;
    let duration_ms = started.elapsed().as_millis() as u64;

    match result {
        _ if cancel.is_cancelled() => {
            logger::info(
                "job.close",
                json!({ "jobId": id, "durationMs": duration_ms, "cancelled": true }),
            );
        }
        Ok(Ok(value)) => match parse_job_output(&value) {
            Some(mut output) => {
                output["job_id"] = Value::from(id.clone());
                // History reads the saved result; keep one even if the engine did not.
                if !OUTPUT_FILES.iter().any(|name| run_dir.join(name).exists()) {
                    if let Ok(bytes) = serde_json::to_vec(&output) {
                        let _ = crate::paths::write_atomic(&run_dir.join("result.json"), &bytes);
                    }
                }
                if finish_run_record(
                    &output_directory,
                    &id,
                    "completed",
                    None,
                    FailureDetails::default(),
                )
                .is_err()
                {
                    logger::warn("job.history.writeFailed", json!({ "jobId": id }));
                }
                logger::info(
                    "job.close",
                    json!({ "jobId": id, "durationMs": duration_ms, "completed": true }),
                );
                // The engine's result has no editor flags; history adds them when it reads the run.
                crate::file_manager::mark_editable(&run_dir, &mut output);
                state.jobs.finish(&app, &id, Outcome::Completed(output));
            }
            None => {
                logger::error("job.engine.invalidResult", json!({ "jobId": id }));
                report_error(
                    "The clipping engine returned an unsupported result.".into(),
                    None,
                );
            }
        },
        Ok(Err(EngineError::Cancelled)) => {
            report_error("The clipping engine stopped.".into(), None);
        }
        Ok(Err(error)) => {
            write_dev_engine_log(&paths.logs, &id, &format!("{error:?}\n"));
            let (message, hint) = clip_engine::pipeline::describe_failure(&error);
            logger::error(
                "job.failed",
                json!({ "jobId": id, "durationMs": duration_ms }),
            );
            report_error(message, Some(hint).filter(|h| !h.is_empty()));
        }
        Err(join_error) => {
            write_dev_engine_log(
                &paths.logs,
                &id,
                &format!("engine task ended abnormally: {join_error:?}\n"),
            );
            logger::error(
                "job.engine.panicked",
                json!({ "jobId": id, "durationMs": duration_ms }),
            );
            report_error(
                "The clipping engine stopped unexpectedly.".into(),
                Some(
                    "Retry the run. If it keeps failing, report the steps that reproduce it."
                        .into(),
                ),
            );
        }
    }

    let _ = std::fs::remove_dir_all(&work_dir);
    crate::sources::apply_retention(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_text_hides_paths_and_urls() {
        assert_eq!(safe_text("Rendering clip 2"), "Rendering clip 2");
        assert_ne!(
            safe_text("Reading /Users/me/secret.mp4"),
            "Reading /Users/me/secret.mp4"
        );
        assert_ne!(
            safe_text("GET https://api.example.com"),
            "GET https://api.example.com"
        );
        assert_ne!(safe_text("api_key=abc"), "api_key=abc");
    }
}
