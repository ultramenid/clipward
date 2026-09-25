//! Tauri commands: the renderer-facing API.
//! src/renderer/lib/bridge.ts maps the old `window.clipward` API onto these.
//!
//! Only the main window may call them: build.rs declares each command and
//! capabilities/default.json grants them to that window alone. Every path the
//! renderer names is checked here before it is read, opened or copied.
use crate::ai_providers::{self, DetectedCli, LocalModelsResult, ProviderTestResult};
use crate::file_manager::{self, ExportClip, ExportResult, JobHistoryEntry};
use crate::jobs::JobSnapshot;
use crate::logger;
use crate::network_policy::assert_public_web_url;
use crate::run_history::{create_run_record, finish_run_record, FailureDetails};
use crate::security::{
    assert_absolute_path, is_trusted_external_url, is_web_url, is_within_directory,
};
use crate::settings::{public_settings, EditableSettings, PublicSettings};
use crate::tools::{check_runs, resolve_binary, supports_caption_filter, tool_available, Tool};
use crate::whisper::WhisperModelStatus;
use crate::AppState;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

type CmdResult<T> = Result<T, String>;

/// Defense in depth next to the capability: commands answer the main window only.
fn assert_main(window: &WebviewWindow) -> CmdResult<()> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("Unauthorized application request".into())
    }
}

fn output_directory(state: &AppState) -> CmdResult<PathBuf> {
    Ok(PathBuf::from(state.settings.load()?.output_directory))
}

async fn pick_folder(window: &WebviewWindow, title: &str) -> Option<PathBuf> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(window)
        .set_title(title)
        .set_can_create_directories(true)
        .pick_folder(move |path| {
            let _ = tx.send(path);
        });
    rx.await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
}

// ---- settings ---------------------------------------------------------------

#[tauri::command]
pub fn settings_load(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<PublicSettings> {
    assert_main(&window)?;
    Ok(public_settings(&state.settings.load()?))
}

#[tauri::command]
pub fn settings_save(
    window: WebviewWindow,
    state: State<'_, AppState>,
    settings: EditableSettings,
) -> CmdResult<PublicSettings> {
    assert_main(&window)?;
    let current = state.settings.load()?;
    let selected = state
        .selected_output_dirs
        .lock()
        .map_err(|_| "Settings are unavailable".to_string())?;
    if settings.output_directory != current.output_directory
        && !selected.contains(&settings.output_directory)
    {
        return Err("Choose the output folder with the folder picker".into());
    }
    drop(selected);
    state.settings.save_public(settings)
}

#[tauri::command]
pub fn settings_replace_api_key(
    window: WebviewWindow,
    state: State<'_, AppState>,
    key: String,
    value: String,
) -> CmdResult<PublicSettings> {
    assert_main(&window)?;
    state.settings.replace_api_key(&key, &value)
}

/// Switch native window glass on or off now and remember the choice.
#[tauri::command]
pub fn window_set_glass(
    window: WebviewWindow,
    state: State<'_, AppState>,
    enabled: bool,
) -> CmdResult<PublicSettings> {
    assert_main(&window)?;
    if !crate::window_glass::SUPPORTED {
        return Err("Window transparency is available on macOS and Windows".into());
    }
    let saved = state.settings.set_window_glass(enabled)?;
    crate::window_glass::apply(&window, enabled);
    Ok(saved)
}

/// Save the appearance settings and apply the parts the shell owns: the
/// webview zoom (interface scale) and the macOS blur radius. The renderer
/// applies the rest (theme, colours, glass strengths) itself.
#[tauri::command]
pub fn settings_set_appearance(
    window: WebviewWindow,
    state: State<'_, AppState>,
    appearance: crate::settings::Appearance,
) -> CmdResult<PublicSettings> {
    assert_main(&window)?;
    let saved = state.settings.set_appearance(appearance)?;
    let _ = window.set_zoom(saved.appearance.ui_scale);
    crate::window_glass::set_blur_radius(&window, saved.appearance.blur_radius);
    Ok(saved)
}

#[tauri::command]
pub async fn settings_select_output_dir(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<Option<String>> {
    assert_main(&window)?;
    let Some(dir) = pick_folder(&window, "Choose Output Folder").await else {
        return Ok(None);
    };
    let dir = dir.to_string_lossy().into_owned();
    if let Ok(mut selected) = state.selected_output_dirs.lock() {
        selected.insert(dir.clone());
    }
    Ok(Some(dir))
}

// ---- AI providers & Whisper ---------------------------------------------------

#[tauri::command]
pub fn ai_detect(window: WebviewWindow, state: State<'_, AppState>) -> CmdResult<Vec<DetectedCli>> {
    assert_main(&window)?;
    Ok(ai_providers::detect_clis(&state.paths.home))
}

#[tauri::command]
pub async fn ai_test(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<ProviderTestResult> {
    assert_main(&window)?;
    let settings = state.settings.load()?;
    Ok(ai_providers::test_provider(&state.paths.home, &settings).await)
}

/// Models the saved local AI server offers (Settings → AI provider → Local AI).
#[tauri::command]
pub async fn ai_local_models(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<LocalModelsResult> {
    assert_main(&window)?;
    let settings = state.settings.load()?;
    Ok(ai_providers::detect_local_models(&settings).await)
}

#[tauri::command]
pub fn whisper_list(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<Vec<WhisperModelStatus>> {
    assert_main(&window)?;
    Ok(state.whisper.list())
}

#[tauri::command]
pub async fn whisper_download(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<Vec<WhisperModelStatus>> {
    assert_main(&window)?;
    state
        .whisper
        .download(&id, |status| {
            let _ = app.emit_to("main", "whisper:progress", status);
        })
        .await
}

#[tauri::command]
pub fn whisper_cancel(
    window: WebviewWindow,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<()> {
    assert_main(&window)?;
    state.whisper.cancel(&id);
    Ok(())
}

#[tauri::command]
pub fn whisper_delete(
    window: WebviewWindow,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<Vec<WhisperModelStatus>> {
    assert_main(&window)?;
    state.whisper.delete(&id)
}

// ---- jobs ---------------------------------------------------------------------

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct JobStartResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queued: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn start_error(message: impl Into<String>) -> CmdResult<JobStartResult> {
    Ok(JobStartResult {
        error: Some(message.into()),
        ..Default::default()
    })
}

#[tauri::command]
pub async fn job_start(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    config: Value,
) -> CmdResult<JobStartResult> {
    assert_main(&window)?;
    let settings = match state.settings.load() {
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
        state
            .media
            .assert_media_path(&request.video_url, &library)
            .map(|_| ())
    };
    if let Err(message) = checked {
        return start_error(message);
    }
    if let Some(banner) = &request.banner_channel_url {
        if let Err(message) = assert_public_web_url(banner).await {
            return start_error(message);
        }
    }

    let paths = &state.paths;
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
    if state.whisper.downloaded(&settings.whisper_model).is_none() {
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
    let job = state.jobs.enqueue(&app, job_id.clone(), request, library);
    Ok(JobStartResult {
        job_id: Some(job_id),
        queued: Some(job.status == "queued"),
        error: None,
    })
}

#[tauri::command]
pub fn job_cancel(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    job_id: String,
) -> CmdResult<bool> {
    assert_main(&window)?;
    logger::info("job.cancel.request", json!({ "jobId": job_id }));
    Ok(state.jobs.cancel(&app, &job_id))
}

#[tauri::command]
pub fn jobs_list(window: WebviewWindow, state: State<'_, AppState>) -> CmdResult<Vec<JobSnapshot>> {
    assert_main(&window)?;
    Ok(state.jobs.list())
}

#[tauri::command]
pub fn jobs_dismiss(
    window: WebviewWindow,
    state: State<'_, AppState>,
    job_id: String,
) -> CmdResult<bool> {
    assert_main(&window)?;
    Ok(state.jobs.dismiss(&job_id))
}

// ---- library --------------------------------------------------------------------

#[tauri::command]
pub async fn history_list(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<Vec<JobHistoryEntry>> {
    assert_main(&window)?;
    let library = output_directory(&state)?;
    let live = state.jobs.live_ids();
    tauri::async_runtime::spawn_blocking(move || file_manager::get_job_history(&library, &live))
        .await
        .map_err(|_| "Could not read the clip library".to_string())?
}

#[tauri::command]
pub fn history_get_job(
    window: WebviewWindow,
    state: State<'_, AppState>,
    output_dir: String,
) -> CmdResult<Option<Value>> {
    assert_main(&window)?;
    let dir = assert_absolute_path(&output_dir)?;
    let library = output_directory(&state)?;
    if !is_within_directory(&dir, &library) {
        return Err("Job is outside the library".into());
    }
    Ok(file_manager::get_job_output(&dir, &library))
}

#[tauri::command]
pub async fn thumbnails_generate(
    window: WebviewWindow,
    state: State<'_, AppState>,
    video_path: String,
    seek_seconds: Option<f64>,
) -> CmdResult<Option<String>> {
    assert_main(&window)?;
    let library = output_directory(&state)?;
    let video = state.media.assert_media_path(&video_path, &library)?;
    if seek_seconds.is_some_and(|s| !s.is_finite() || !(0.0..=6.0 * 60.0 * 60.0).contains(&s)) {
        return Err("Invalid thumbnail time".into());
    }
    let Some(thumbnail) =
        file_manager::generate_thumbnail(&state.paths, &video, seek_seconds).await
    else {
        return Ok(None);
    };
    // Thumbnails live outside the library; authorize this one so it can be shown.
    Ok(Some(
        state
            .media
            .authorize(&thumbnail)?
            .to_string_lossy()
            .into_owned(),
    ))
}

/// Development-only: scripted end-to-end runs must never open the developer's browser.
fn external_links_blocked() -> bool {
    cfg!(debug_assertions) && std::env::var("CLIPWARD_E2E").as_deref() == Ok("1")
}

pub fn open_trusted_url(app: &AppHandle, url: &str) -> CmdResult<()> {
    if !is_trusted_external_url(url) {
        return Err("This external link is not supported".into());
    }
    if external_links_blocked() {
        logger::info("e2e.openExternal.blocked", json!({}));
        return Ok(());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|_| "Could not open the link".to_string())
}

#[tauri::command]
pub fn shell_open_path(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> CmdResult<bool> {
    assert_main(&window)?;
    if is_web_url(&path) {
        open_trusted_url(&app, &path)?;
        return Ok(true);
    }
    let path = assert_absolute_path(&path)?;
    if !path.exists() {
        return Ok(false);
    }
    // Check and open the same canonical name: an alias can hide a .app suffix.
    let canonical = std::fs::canonicalize(&path).map_err(|_| "Invalid path".to_string())?;
    let canonical_str = canonical.to_string_lossy().into_owned();
    let library = output_directory(&state)?;
    if !is_within_directory(&canonical, &library) {
        state.media.assert_media_path(&canonical_str, &library)?;
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
        state.media.assert_media_path(&canonical_str, &library)?;
    }
    Ok(app.opener().open_path(canonical_str, None::<&str>).is_ok())
}

#[tauri::command]
pub fn shell_show_item_in_folder(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> CmdResult<bool> {
    assert_main(&window)?;
    let path = assert_absolute_path(&path)?;
    let library = output_directory(&state)?;
    if !is_within_directory(&path, &library) {
        state
            .media
            .assert_media_path(&path.to_string_lossy(), &library)?;
    }
    if !path.exists() {
        return Ok(false);
    }
    Ok(app.opener().reveal_item_in_dir(&path).is_ok())
}

#[tauri::command]
pub async fn dialog_select_video(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<Option<String>> {
    assert_main(&window)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("Choose a Video")
        .add_filter(
            "Video Files",
            &["mp4", "m4v", "mkv", "webm", "avi", "mov", "flv"],
        )
        .add_filter("All Files", &["*"])
        .pick_file(move |path| {
            let _ = tx.send(path);
        });
    let Some(path) = rx
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
    else {
        return Ok(None);
    };
    Ok(Some(
        state.media.authorize(&path)?.to_string_lossy().into_owned(),
    ))
}

#[tauri::command]
pub async fn clips_bulk_export(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    clips: Vec<ExportClip>,
) -> CmdResult<ExportResult> {
    assert_main(&window)?;
    if clips.len() > 500 {
        return Err("Invalid export selection".into());
    }
    let library = output_directory(&state)?;
    for clip in &clips {
        if clip.name.encode_utf16().count() > 500 {
            return Err("Invalid export selection".into());
        }
        state.media.assert_media_path(&clip.path, &library)?;
    }
    if clips.is_empty() {
        return Ok(ExportResult {
            success: false,
            count: 0,
            failed_count: 0,
            dest_dir: None,
        });
    }
    let Some(dest) = pick_folder(&window, "Export Clips To…").await else {
        return Ok(ExportResult {
            success: false,
            count: 0,
            failed_count: 0,
            dest_dir: None,
        });
    };
    tauri::async_runtime::spawn_blocking(move || {
        let state = tauri::Manager::state::<AppState>(&app);
        file_manager::export_clips(&state.media, &library, &clips, &dest)
    })
    .await
    .map_err(|_| "Could not export the clips".to_string())
}

// ---- system & diagnostics -------------------------------------------------------

#[tauri::command]
pub fn system_is_packaged(window: WebviewWindow, state: State<'_, AppState>) -> CmdResult<bool> {
    assert_main(&window)?;
    Ok(state.paths.packaged)
}

/// ToolStatus in src/shared/api.ts: external tools plus the bundled engine assets
/// (`engine`: fonts and models; `smart_framing`: the face detection model).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    smart_framing: bool,
    smart_framing_error: Option<String>,
    ffmpeg: bool,
    ffmpeg_captions: bool,
    ffprobe: bool,
    ytdlp: bool,
    engine: bool,
    engine_path: String,
}

#[tauri::command]
pub async fn system_check_tools(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<ToolStatus> {
    assert_main(&window)?;
    let paths = &state.paths;
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
    Ok(status)
}

#[tauri::command]
pub fn diagnostics_get_log_path(window: WebviewWindow) -> CmdResult<String> {
    assert_main(&window)?;
    Ok(logger::log_file_path().to_string_lossy().into_owned())
}

#[tauri::command]
pub fn diagnostics_open_log_folder(window: WebviewWindow, app: AppHandle) -> CmdResult<bool> {
    assert_main(&window)?;
    Ok(show_log_file(&app))
}

pub fn show_log_file(app: &AppHandle) -> bool {
    let log = logger::log_file_path();
    Path::new(&log).exists() && app.opener().reveal_item_in_dir(&log).is_ok()
}

// ---- updates ----------------------------------------------------------------------
// TODO(updater): wire tauri-plugin-updater once release signing keys and an update
// endpoint exist (it needs a pubkey in tauri.conf.json and signed artifacts). Until
// then checks find nothing and download/install report that updates are unavailable.

#[tauri::command]
pub fn update_check(window: WebviewWindow) -> CmdResult<()> {
    assert_main(&window)
}

#[tauri::command]
pub fn update_download(window: WebviewWindow) -> CmdResult<()> {
    assert_main(&window)?;
    Err("Automatic updates are not available in this build. Download the latest release from GitHub.".into())
}

#[tauri::command]
pub fn update_install(window: WebviewWindow) -> CmdResult<()> {
    assert_main(&window)?;
    Err("Automatic updates are not available in this build. Download the latest release from GitHub.".into())
}
