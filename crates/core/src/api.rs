//! The app's operations, one method per UI action. Shells call these after doing the
//! native part themselves (a file dialog, a window effect); every path the UI names
//! is checked here before it is read, opened or copied.
//!
//! Async methods must run on the tokio runtime (`Core::runtime`).
use crate::ai_providers::{self, DetectedCli, LocalModelsResult, ProviderTestResult};
use crate::file_manager::{self, ExportClip, ExportResult, JobHistoryEntry};
use crate::jobs::JobSnapshot;
use crate::logger;
use crate::network_policy::assert_public_web_url;
use crate::run_history::{create_run_record, finish_run_record, FailureDetails};
use crate::security::{assert_absolute_path, is_web_url, is_within_directory};
use crate::settings::{public_settings, Appearance, EditableSettings, PublicSettings};
use crate::tools::{check_runs, resolve_binary, supports_caption_filter, tool_available, Tool};
use crate::whisper::WhisperModelStatus;
use crate::Core;
use clip_engine::render::project::{MediaFile, OverlayPreview, Project};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type CmdResult<T> = Result<T, String>;

#[derive(Serialize, Default, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobStartResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn start_error(message: impl Into<String>) -> JobStartResult {
    JobStartResult {
        error: Some(message.into()),
        ..Default::default()
    }
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImportedAudio {
    pub file: String,
    pub name: String,
    pub duration_ms: i64,
}

/// ToolStatus in src/shared/api.ts: external tools plus the bundled engine assets
/// (`engine`: fonts and models; `smart_framing`: the face detection model).
#[derive(Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub smart_framing: bool,
    pub smart_framing_error: Option<String>,
    pub ffmpeg: bool,
    pub ffmpeg_captions: bool,
    pub ffprobe: bool,
    pub ytdlp: bool,
    pub engine: bool,
    pub engine_path: String,
}

/// Largest YouTube thumbnail accepted, bytes (hqdefault is about 30 KB).
const MAX_THUMBNAIL_BYTES: usize = 2 * 1024 * 1024;

/// YouTube video ids are 11 URL-safe base64 characters.
fn is_youtube_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Largest voiceover recording accepted, bytes.
const MAX_RECORDING_BYTES: usize = 64 * 1024 * 1024;

impl Core {
    pub fn output_directory(&self) -> CmdResult<PathBuf> {
        Ok(PathBuf::from(self.settings.load()?.output_directory))
    }

    // ---- settings -----------------------------------------------------------

    pub fn settings_load(&self) -> CmdResult<PublicSettings> {
        Ok(public_settings(&self.settings.load()?))
    }

    pub fn settings_save(&self, settings: EditableSettings) -> CmdResult<PublicSettings> {
        let current = self.settings.load()?;
        let selected = self
            .selected_output_dirs
            .lock()
            .map_err(|_| "Settings are unavailable".to_string())?;
        if settings.output_directory != current.output_directory
            && !selected.contains(&settings.output_directory)
        {
            return Err("Choose the output folder with the folder picker".into());
        }
        drop(selected);
        self.settings.save_public(settings)
    }

    pub fn settings_replace_api_key(&self, key: &str, value: &str) -> CmdResult<PublicSettings> {
        self.settings.replace_api_key(key, value)
    }

    /// Remember the choice; the shell switches the window's glass itself.
    pub fn set_window_glass(&self, enabled: bool) -> CmdResult<PublicSettings> {
        if !crate::window_glass::SUPPORTED {
            return Err("Window transparency is available on macOS and Windows".into());
        }
        self.settings.set_window_glass(enabled)
    }

    /// Save the appearance; the shell applies scale and blur radius from the result.
    pub fn set_appearance(&self, appearance: Appearance) -> CmdResult<PublicSettings> {
        self.settings.set_appearance(appearance)
    }

    /// How long runs keep their source video for re-editing; None keeps it until the run is deleted.
    pub fn set_source_retention(&self, days: Option<u32>) -> CmdResult<PublicSettings> {
        let saved = self.settings.set_source_retention(days)?;
        crate::sources::apply_retention(self);
        Ok(saved)
    }

    /// Disk used by source videos kept for re-editing.
    pub async fn sources_usage(&self) -> CmdResult<crate::sources::SourceUsage> {
        let library = self.output_directory()?;
        tokio::task::spawn_blocking(move || crate::sources::usage(&library))
            .await
            .map_err(|_| "Could not measure kept source videos".to_string())
    }

    /// Delete every kept source video except those of runs in progress. Returns how many.
    pub async fn sources_delete_all(&self) -> CmdResult<u32> {
        let library = self.output_directory()?;
        let live = self.jobs.live_ids();
        tokio::task::spawn_blocking(move || {
            crate::sources::sweep(&library, std::time::Duration::ZERO, &live)
        })
        .await
        .map_err(|_| "Could not delete kept source videos".to_string())
    }

    /// A folder the user just picked with the folder dialog: settings may now use it.
    pub fn allow_output_dir(&self, dir: &Path) -> String {
        let dir = dir.to_string_lossy().into_owned();
        if let Ok(mut selected) = self.selected_output_dirs.lock() {
            selected.insert(dir.clone());
        }
        dir
    }

    // ---- AI providers & Whisper ---------------------------------------------

    pub fn ai_detect(&self) -> Vec<DetectedCli> {
        ai_providers::detect_clis(&self.paths.home)
    }

    pub async fn ai_test(&self) -> CmdResult<ProviderTestResult> {
        let settings = self.settings.load()?;
        Ok(ai_providers::test_provider(&self.paths.home, &settings).await)
    }

    /// Models the saved local AI server offers (Settings → AI provider → Local AI).
    pub async fn ai_local_models(&self) -> CmdResult<LocalModelsResult> {
        let settings = self.settings.load()?;
        Ok(ai_providers::detect_local_models(&settings).await)
    }

    pub async fn ai_models(&self) -> CmdResult<Vec<clip_engine::llm::ModelChoice>> {
        let settings = self.settings.load()?;
        ai_providers::list_models(&self.paths.home, &settings).await
    }

    pub fn whisper_list(&self) -> Vec<WhisperModelStatus> {
        self.whisper.list()
    }

    pub async fn whisper_download(
        &self,
        id: &str,
        on_progress: impl Fn(&WhisperModelStatus),
    ) -> CmdResult<Vec<WhisperModelStatus>> {
        self.whisper
            .download(id, |status| on_progress(&status))
            .await
    }

    pub fn whisper_cancel(&self, id: &str) {
        self.whisper.cancel(id);
    }

    pub fn whisper_delete(&self, id: &str) -> CmdResult<Vec<WhisperModelStatus>> {
        self.whisper.delete(id)
    }

    // ---- jobs ---------------------------------------------------------------

    /// Validate a job request, check the tools it needs and queue it.
    pub async fn job_start(self: &Arc<Self>, config: Value) -> JobStartResult {
        let settings = match self.settings.load() {
            Ok(settings) => settings,
            Err(message) => return start_error(message),
        };
        let library = PathBuf::from(&settings.output_directory);
        let request = match crate::validation::validate_job_config(config) {
            Ok(request) => request,
            Err(message) => return start_error(message),
        };
        let checked = if is_web_url(&request.video_url) {
            assert_public_web_url(&request.video_url).await
        } else {
            self.media
                .assert_media_path(&request.video_url, &library)
                .map(|_| ())
        };
        if let Err(message) = checked {
            return start_error(message);
        }
        // Logos must be files the user picked this session; the engine gets their canonical paths.
        let mut request = request;
        for logo in &mut request.logos {
            match self.media.assert_media_path(&logo.path, &library) {
                Ok(canonical) => logo.path = canonical.to_string_lossy().into_owned(),
                Err(message) => return start_error(message),
            }
        }

        let paths = &self.paths;
        // Manual clip mode never calls an AI provider; only AI planning needs one ready.
        let provider_problem = request
            .manual_clips
            .is_none()
            .then(|| ai_providers::provider_problem(&paths.home, &settings))
            .flatten();
        if let Some(problem) = provider_problem {
            logger::warn(
                "job.start.providerNotReady",
                json!({ "provider": settings.ai_provider }),
            );
            return start_error(problem);
        }
        if self.whisper.downloaded(&settings.whisper_model).is_none() {
            return start_error(
                "Download a Whisper model in Settings → Transcription first. Transcription runs offline on this computer.",
            );
        }
        let missing = [Tool::Ffmpeg, Tool::Ffprobe]
            .into_iter()
            .find(|tool| !tool_available(&resolve_binary(paths, *tool)));
        if let Some(tool) = missing {
            logger::error(
                "job.start.preflight.failed",
                json!({ "tool": format!("{tool:?}") }),
            );
            return start_error(if paths.packaged {
                "Bundled FFmpeg tools are missing.\n\nReinstall Clipward to repair the clipping tools."
            } else {
                "FFmpeg and FFprobe were not found.\n\nInstall FFmpeg or stage it in engine-bin/, then open Settings → System check."
            });
        }
        if !paths.assets_dir().is_dir() {
            return start_error("The clipping engine is incomplete. Open Settings → System check, then repair the Clipward installation before starting.");
        }
        if request.include_captions && !supports_caption_filter(paths).await {
            return start_error("FFmpeg cannot render captions because its ass filter is missing. Install an FFmpeg build with libass, or turn captions off.");
        }

        if std::fs::create_dir_all(&library).is_err() {
            return start_error(
                "Could not create the clipping run. Check the output folder and retry.",
            );
        }
        let job_id = uuid::Uuid::new_v4().to_string();
        logger::info(
            "job.start.request",
            json!({
                "jobId": job_id,
                "sourceType": if is_web_url(&request.video_url) { "remote" } else { "local" },
                "aspectRatio": request.aspect_ratio,
            }),
        );
        if create_run_record(&library, &job_id, &request.video_url).is_err() {
            let _ = finish_run_record(
                &library,
                &job_id,
                "failed",
                Some("Could not start this run."),
                FailureDetails::default(),
            );
            return start_error(
                "Could not create the clipping run. Check the output folder and retry.",
            );
        }
        // Starts now when a slot is free; otherwise waits its turn in the queue.
        let job = self.jobs.enqueue(self, job_id.clone(), request, library);
        JobStartResult {
            job_id: Some(job_id),
            queued: Some(job.status == "queued"),
            error: None,
        }
    }

    pub fn job_cancel(&self, job_id: &str) -> bool {
        logger::info("job.cancel.request", json!({ "jobId": job_id }));
        self.jobs.cancel(self, job_id)
    }

    pub fn jobs_list(&self) -> Vec<JobSnapshot> {
        self.jobs.list()
    }

    pub fn jobs_dismiss(&self, job_id: &str) -> bool {
        self.jobs.dismiss(job_id)
    }

    pub async fn job_delete(&self, job_id: &str) -> CmdResult<()> {
        logger::info("job.delete.request", json!({ "jobId": job_id }));
        let library = self.output_directory()?;
        self.jobs.delete(self, job_id, library).await?;
        // Preview chunks aren't filed by run; drop them all so none of the deleted clip
        // stays behind (others re-render when next edited).
        crate::preview::clear(&self.paths.user_data);
        Ok(())
    }

    // ---- library ------------------------------------------------------------

    pub async fn history_list(&self) -> CmdResult<Vec<JobHistoryEntry>> {
        let library = self.output_directory()?;
        let live = self.jobs.live_ids();
        tokio::task::spawn_blocking(move || file_manager::get_job_history(&library, &live))
            .await
            .map_err(|_| "Could not read the clip library".to_string())?
    }

    pub fn history_get_job(&self, output_dir: &str) -> CmdResult<Option<Value>> {
        let dir = assert_absolute_path(output_dir)?;
        let library = self.output_directory()?;
        if !is_within_directory(&dir, &library) {
            return Err("Job is outside the library".into());
        }
        Ok(file_manager::get_job_output(&dir, &library))
    }

    /// A clip's edit, its source video's availability and the campaign brief.
    pub async fn project_load(
        &self,
        output_dir: String,
        clip_index: u32,
    ) -> CmdResult<crate::editor::EditorProject> {
        let library = self.output_directory()?;
        let mut opened = tokio::task::spawn_blocking(move || {
            crate::editor::load(&output_dir, clip_index, &library)
        })
        .await
        .map_err(|_| "Could not open this clip".to_string())??;
        // The editor's instant preview plays the source (a user's own file may be outside the library).
        opened.source_path = opened
            .source_path
            .and_then(|path| self.media.authorize(&path).ok());
        opened.source_available = opened.source_path.is_some();
        Ok(opened)
    }

    /// The exact preview of one CHUNK_MS stretch of the edit, rendered by the engine.
    pub async fn project_preview_chunk(
        &self,
        output_dir: &str,
        project: &Project,
        index: u32,
        generation: u64,
    ) -> CmdResult<PathBuf> {
        let library = self.output_directory()?;
        crate::preview::chunk(
            (&self.previews, &self.paths, &self.media),
            &library,
            output_dir,
            project,
            index,
            generation,
        )
        .await
    }

    /// The whole edit's exact preview as one file (its chunks joined), for smooth playback.
    pub async fn project_preview_joined(
        &self,
        output_dir: &str,
        project: &Project,
        generation: u64,
    ) -> CmdResult<PathBuf> {
        let library = self.output_directory()?;
        crate::preview::joined(
            (&self.previews, &self.paths, &self.media),
            &library,
            output_dir,
            project,
            generation,
        )
        .await
    }

    /// Save the edit and render it over the clip; `on_progress` gets 0..1.
    pub async fn project_export(
        &self,
        output_dir: &str,
        clip_index: u32,
        project: &Project,
        on_progress: impl Fn(f64) + Send + Sync + 'static,
    ) -> CmdResult<()> {
        let library = self.output_directory()?;
        crate::preview::export(
            (&self.previews, &self.paths),
            &library,
            output_dir,
            clip_index,
            project,
            on_progress,
        )
        .await
    }

    /// Import a music or sound effect file the user picked into the run.
    pub async fn project_import_audio(
        &self,
        output_dir: &str,
        path: &Path,
    ) -> CmdResult<ImportedAudio> {
        let library = self.output_directory()?;
        let name = path
            .file_stem()
            .map(|n| n.to_string_lossy().chars().take(200).collect())
            .unwrap_or_default();
        let (file, duration_ms) =
            crate::preview::import_audio(&self.paths, &library, output_dir, path).await?;
        Ok(ImportedAudio {
            file,
            name,
            duration_ms,
        })
    }

    /// Import a video or image the user picked into the run.
    pub async fn project_import_media(
        &self,
        output_dir: &str,
        path: &Path,
        fps: &str,
    ) -> CmdResult<MediaFile> {
        let library = self.output_directory()?;
        crate::preview::import_media(&self.paths, &library, output_dir, path, fps).await
    }

    /// Save a voiceover the editor recorded into the run, as a sound.
    pub async fn project_import_recording(
        &self,
        output_dir: &str,
        data: &[u8],
        ext: &str,
    ) -> CmdResult<ImportedAudio> {
        if data.is_empty() || data.len() > MAX_RECORDING_BYTES {
            return Err("The recording is empty or too long".into());
        }
        let library = self.output_directory()?;
        let (file, duration_ms) =
            crate::preview::import_recording(&self.paths, &library, output_dir, data, ext).await?;
        Ok(ImportedAudio {
            file,
            name: "Voiceover".into(),
            duration_ms,
        })
    }

    /// A sound's waveform: loudness peaks, 20 per second (the source's when `file` is None).
    pub async fn project_waveform(
        &self,
        output_dir: &str,
        file: Option<&str>,
    ) -> CmdResult<Vec<u8>> {
        let library = self.output_directory()?;
        crate::preview::waveform(&self.paths, &library, output_dir, file).await
    }

    /// Thumbnails for the timeline: one frame per second of `block` (60 s) of a video (the
    /// source's when `file` is None), side by side in one image. Returns its path.
    pub async fn project_filmstrip(
        &self,
        output_dir: &str,
        file: Option<&str>,
        block: u32,
    ) -> CmdResult<PathBuf> {
        let library = self.output_directory()?;
        crate::preview::filmstrip(&self.paths, &library, output_dir, file, block).await
    }

    /// The title and banner images with their automatic places at `time_ms`, for the editor.
    pub fn project_overlay_previews(
        &self,
        output_dir: &str,
        project: &Project,
        time_ms: i64,
    ) -> CmdResult<Vec<OverlayPreview>> {
        let library = self.output_directory()?;
        crate::preview::overlay_previews(
            (&self.paths, &self.media),
            &library,
            output_dir,
            project,
            time_ms,
        )
    }

    /// Copy a logo image the user picked into the run; returns its file name there.
    pub fn project_import_logo(&self, output_dir: &str, path: &Path) -> CmdResult<String> {
        let library = self.output_directory()?;
        crate::preview::import_logo(&library, output_dir, path)
    }

    /// Save a clip's edit after checking it against its source video; true while the
    /// library's clip file lacks it (not exported yet).
    pub async fn project_save(
        &self,
        output_dir: String,
        clip_index: u32,
        project: Project,
    ) -> CmdResult<bool> {
        let library = self.output_directory()?;
        tokio::task::spawn_blocking(move || {
            crate::editor::save(&output_dir, clip_index, &project, &library)
        })
        .await
        .map_err(|_| "Could not save the edit".to_string())?
    }

    pub async fn thumbnails_generate(
        &self,
        video_path: &str,
        seek_seconds: Option<f64>,
    ) -> CmdResult<Option<PathBuf>> {
        let library = self.output_directory()?;
        let video = self.media.assert_media_path(video_path, &library)?;
        if seek_seconds.is_some_and(|s| !s.is_finite() || !(0.0..=6.0 * 60.0 * 60.0).contains(&s)) {
            return Err("Invalid thumbnail time".into());
        }
        let Some(thumbnail) =
            file_manager::generate_thumbnail(&self.paths, &video, seek_seconds).await
        else {
            return Ok(None);
        };
        // Thumbnails live outside the library; authorize this one so it can be shown.
        Ok(Some(self.media.authorize(&thumbnail)?))
    }

    /// A YouTube video's thumbnail (`i.ytimg.com/vi/<id>/hqdefault.jpg`, as the React
    /// source picker showed), downloaded once into the private thumbnails folder.
    pub async fn youtube_thumbnail(&self, id: &str) -> CmdResult<Option<PathBuf>> {
        if !is_youtube_id(id) {
            return Err("Invalid YouTube video id".into());
        }
        let dir = crate::paths::private_dir(&self.paths.user_data.join("thumbnails"))
            .map_err(|_| "Could not store the thumbnail".to_string())?;
        let path = dir.join(format!("youtube-{id}.jpg"));
        if path.is_file() {
            return Ok(Some(path));
        }
        let url = format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg");
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|_| "Could not fetch the thumbnail".to_string())?;
        let Ok(response) = client.get(url).send().await else {
            return Ok(None);
        };
        if !response.status().is_success() {
            return Ok(None);
        }
        let Ok(bytes) = response.bytes().await else {
            return Ok(None);
        };
        // A JPEG of sane size; anything else is not shown.
        if bytes.len() > MAX_THUMBNAIL_BYTES || !bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Ok(None);
        }
        crate::paths::write_atomic(&path, &bytes)
            .map_err(|_| "Could not store the thumbnail".to_string())?;
        Ok(Some(path))
    }

    // ---- files the user opens or picks ---------------------------------------

    /// The canonical path to open for `path` (a library file or folder, or a file
    /// picked this session); None when it no longer exists.
    pub fn openable_path(&self, path: &str) -> CmdResult<Option<PathBuf>> {
        let path = assert_absolute_path(path)?;
        if !path.exists() {
            return Ok(None);
        }
        // Check and open the same canonical name: an alias can hide a .app suffix.
        let canonical = std::fs::canonicalize(&path).map_err(|_| "Invalid path".to_string())?;
        let canonical_str = canonical.to_string_lossy().into_owned();
        let library = self.output_directory()?;
        if !is_within_directory(&canonical, &library) {
            self.media.assert_media_path(&canonical_str, &library)?;
        }
        if canonical.is_dir() {
            let is_bundle = canonical.components().any(|part| {
                let part = part.as_os_str().to_string_lossy().to_ascii_lowercase();
                part.ends_with(".app") || part.ends_with(".bundle")
            });
            if is_bundle {
                return Err("Application bundles cannot be opened from the library".into());
            }
        } else {
            self.media.assert_media_path(&canonical_str, &library)?;
        }
        Ok(Some(canonical))
    }

    /// The path to reveal in the file manager; None when it no longer exists.
    pub fn revealable_path(&self, path: &str) -> CmdResult<Option<PathBuf>> {
        let path = assert_absolute_path(path)?;
        let library = self.output_directory()?;
        if !is_within_directory(&path, &library) {
            self.media
                .assert_media_path(&path.to_string_lossy(), &library)?;
        }
        Ok(path.exists().then_some(path))
    }

    /// A file the user picked with a dialog: it may be read this session.
    pub fn authorize_picked(&self, path: &Path) -> CmdResult<String> {
        Ok(self.media.authorize(path)?.to_string_lossy().into_owned())
    }

    /// Check an export selection before asking where to put it.
    pub fn check_export(&self, clips: &[ExportClip]) -> CmdResult<()> {
        if clips.len() > 500 {
            return Err("Invalid export selection".into());
        }
        let library = self.output_directory()?;
        for clip in clips {
            if clip.name.encode_utf16().count() > 500 {
                return Err("Invalid export selection".into());
            }
            self.media.assert_media_path(&clip.path, &library)?;
        }
        Ok(())
    }

    /// Copy checked clips (`check_export`) into the folder the user picked.
    pub async fn export_clips(
        self: &Arc<Self>,
        clips: Vec<ExportClip>,
        dest: PathBuf,
    ) -> CmdResult<ExportResult> {
        self.check_export(&clips)?;
        let library = self.output_directory()?;
        let core = self.clone();
        tokio::task::spawn_blocking(move || {
            file_manager::export_clips(&core.media, &library, &clips, &dest)
        })
        .await
        .map_err(|_| "Could not export the clips".to_string())
    }

    // ---- system & diagnostics -------------------------------------------------

    pub async fn system_check_tools(&self) -> ToolStatus {
        let paths = &self.paths;
        let assets = paths.assets_dir();
        let framing_model = assets
            .join("models")
            .join("face_detection_yunet_2023mar.onnx")
            .is_file();
        // ffmpeg/ffprobe only accept `-version`; `--version` exits non-zero.
        let [ffmpeg_bin, ffprobe_bin, ytdlp_bin] =
            [Tool::Ffmpeg, Tool::Ffprobe, Tool::YtDlp].map(|tool| resolve_binary(paths, tool));
        let (ffmpeg, ffmpeg_captions, ffprobe, ytdlp) = tokio::join!(
            check_runs(&ffmpeg_bin, "-version"),
            supports_caption_filter(paths),
            check_runs(&ffprobe_bin, "-version"),
            check_runs(&ytdlp_bin, "--version"),
        );
        let status = ToolStatus {
            smart_framing: framing_model,
            smart_framing_error: (!framing_model)
                .then(|| "The smart framing model is missing from this installation.".into()),
            ffmpeg,
            ffmpeg_captions,
            ffprobe,
            ytdlp,
            engine: assets.join("fonts").is_dir() && assets.join("models").is_dir(),
            engine_path: assets.to_string_lossy().into_owned(),
        };
        logger::info(
            "system.checkTools",
            json!({ "ffmpeg": ffmpeg, "ffmpegCaptions": ffmpeg_captions, "ffprobe": ffprobe, "ytdlp": ytdlp, "engine": status.engine, "framingModel": framing_model }),
        );
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_ids_are_eleven_url_safe_characters() {
        assert!(is_youtube_id("dQw4w9WgXcQ"));
        assert!(is_youtube_id("a-b_c1234XY"));
        assert!(!is_youtube_id("../../etc/x"));
        assert!(!is_youtube_id("short"));
        assert!(!is_youtube_id("dQw4w9WgXcQ1"));
    }
}
