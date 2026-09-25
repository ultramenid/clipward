//! Clipward app shell: window, menu, commands and the job queue.
//!
//! The renderer talks to this shell through the commands in `commands.rs` and
//! listens for `jobs:update` and `whisper:progress` events. Clipping runs
//! in-process through `clip_engine::pipeline::run_job` (see `jobs.rs`).
mod ai_providers;
mod appearance;
mod commands;
mod file_manager;
mod job_output;
mod jobs;
mod logger;
mod media_protocol;
mod menu;
mod network_policy;
mod paths;
mod run_history;
mod security;
mod settings;
mod shared;
mod tools;
mod validation;
mod whisper;
mod window_glass;

use paths::AppPaths;
use serde_json::json;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, RunEvent, WebviewWindowBuilder};

pub struct AppState {
    pub paths: AppPaths,
    pub settings: settings::SettingsStore,
    pub media: security::MediaRegistry,
    pub whisper: whisper::WhisperModels,
    pub jobs: jobs::JobManager,
    /// Output folders the user picked with the folder dialog this session; settings
    /// accept no other new output folder.
    pub selected_output_dirs: Mutex<HashSet<String>>,
}

static EXITING: AtomicBool = AtomicBool::new(false);

/// Development-only: scripted end-to-end runs keep the window hidden.
fn hidden_for_tests() -> bool {
    cfg!(debug_assertions) && std::env::var("CLIPWARD_E2E").as_deref() == Ok("1")
}

/// Whether a navigation stays inside the app's own bundled (or dev-server) pages.
fn is_app_url(app: &AppHandle, url: &url::Url) -> bool {
    if url.scheme() == "tauri"
        || (matches!(url.scheme(), "http" | "https") && url.host_str() == Some("tauri.localhost"))
    {
        return true;
    }
    if cfg!(debug_assertions) {
        if let Some(dev) = app.config().build.dev_url.as_ref() {
            return url.origin() == dev.origin();
        }
    }
    false
}

fn open_external_if_trusted(app: &AppHandle, url: &url::Url) {
    if security::is_trusted_external_url(url.as_str()) {
        let _ = commands::open_trusted_url(app, url.as_str());
    }
}

fn create_main_window(app: &AppHandle) -> tauri::Result<()> {
    let Some(config) = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
    else {
        return Err(tauri::Error::WindowNotFound);
    };
    // Glass needs a transparent window from the start; the config keeps
    // `transparent: false` so Linux stays opaque (see window_glass.rs).
    let (glass, appearance) = app
        .state::<AppState>()
        .settings
        .load()
        .map(|settings| (settings.window_glass, settings.appearance))
        .unwrap_or_else(|_| (true, appearance::Appearance::default()));
    let nav_app = app.clone();
    let popup_app = app.clone();
    let builder = WebviewWindowBuilder::from_config(app, &config)?
        .background_color(window_glass::initial_background(glass));
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let builder = builder.transparent(true);
    let window = builder
        // Never navigate away from the app; trusted links open in the browser.
        .on_navigation(move |url| {
            if is_app_url(&nav_app, url) {
                return true;
            }
            open_external_if_trusted(&nav_app, url);
            false
        })
        .on_new_window(move |url, _features| {
            open_external_if_trusted(&popup_app, &url);
            NewWindowResponse::Deny
        })
        .visible(config.visible && !hidden_for_tests())
        .build()?;
    // Interface scale is the webview zoom; set before the page paints.
    let _ = window.set_zoom(appearance.ui_scale);
    window_glass::set_blur_radius(&window, appearance.blur_radius);
    if glass {
        window_glass::apply(&window, true);
    }
    Ok(())
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let paths = AppPaths::resolve(&handle);
    logger::init(&paths.logs);
    // The engine and the CLIs it launches find per-user installs (Homebrew, ~/.local/bin).
    tools::augment_process_path(&paths.home);
    if cfg!(debug_assertions) {
        use tracing_subscriber::EnvFilter;
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .try_init();
    }

    jobs::clean_stale_workspaces(&paths.user_data, &HashSet::new());
    logger::info(
        "app.boot",
        json!({
            "version": app.package_info().version.to_string(),
            "tauri": tauri::VERSION,
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "isPackaged": paths.packaged,
            "userData": paths.user_data.to_string_lossy(),
            "logFile": logger::log_file_path().to_string_lossy(),
        }),
    );

    app.manage(AppState {
        settings: settings::SettingsStore::new(
            &paths.user_data,
            paths.legacy_user_data.as_deref(),
            &paths.home,
        ),
        whisper: whisper::WhisperModels::new(&paths.user_data),
        media: security::MediaRegistry::default(),
        jobs: jobs::JobManager::default(),
        selected_output_dirs: Mutex::new(HashSet::new()),
        paths,
    });

    create_main_window(&handle)?;
    Ok(())
}

pub fn run() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        logger::error(
            "main.panic",
            json!({ "at": location.replace(['/', '\\'], ".") }),
        );
    }));

    let app = tauri::Builder::default()
        // Must be registered first: a second launch focuses the running window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol("local-file", |ctx, request, responder| {
            if ctx.webview_label() != "main" {
                responder.respond(
                    tauri::http::Response::builder()
                        .status(403)
                        .body(Vec::new())
                        .unwrap_or_default(),
                );
                return;
            }
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(media_protocol::handle(&app, &request));
            });
        })
        .menu(menu::build)
        .on_menu_event(menu::handle_event)
        .setup(setup)
        .invoke_handler(tauri::generate_handler![
            commands::settings_load,
            commands::settings_save,
            commands::settings_replace_api_key,
            commands::window_set_glass,
            commands::settings_set_appearance,
            commands::settings_select_output_dir,
            commands::ai_detect,
            commands::ai_test,
            commands::whisper_list,
            commands::whisper_download,
            commands::whisper_cancel,
            commands::whisper_delete,
            commands::job_start,
            commands::job_cancel,
            commands::jobs_list,
            commands::jobs_dismiss,
            commands::history_list,
            commands::history_get_job,
            commands::thumbnails_generate,
            commands::shell_open_path,
            commands::shell_show_item_in_folder,
            commands::dialog_select_video,
            commands::clips_bulk_export,
            commands::system_is_packaged,
            commands::system_check_tools,
            commands::diagnostics_get_log_path,
            commands::diagnostics_open_log_folder,
            commands::update_check,
            commands::update_download,
            commands::update_install,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Clipward");

    app.run(|app, event| {
        if let RunEvent::ExitRequested { api, .. } = event {
            // Queued runs are recorded as cancelled; running ones are cancelled and
            // get a few seconds to stop their media processes and clean up.
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            let running = state.jobs.stop_all_for_quit();
            if running > 0 && !EXITING.swap(true, Ordering::SeqCst) {
                api.prevent_exit();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    for _ in 0..50 {
                        if app.state::<AppState>().jobs.running_count() == 0 {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    app.exit(0);
                });
            }
        }
    });
}
