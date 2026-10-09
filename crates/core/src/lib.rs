//! Clipward's app logic, shared by the desktop shells: settings, the job queue, the
//! clip library, the editor's previews and the checks on every path the UI names.
//!
//! A shell builds one [`Core`] at startup and calls the methods in `api.rs`; dialogs,
//! menus and windows stay in the shell. Clipping runs in-process through
//! `clip_engine::pipeline::run_job` (see `jobs.rs`).
pub mod ai_providers;
mod api;
pub mod appearance;
pub mod color;
pub mod edit_history;
pub mod editor;
pub mod file_manager;
pub mod format;
pub mod inspector;
pub mod job_form;
pub mod job_list;
pub mod job_output;
pub mod job_view;
pub mod jobs;
pub mod jobs_page;
pub mod library;
pub mod manual_clips;
pub mod logger;
pub mod network_policy;
pub mod overlays;
pub mod provider_catalog;
pub mod paths;
pub mod preview;
pub mod readiness;
pub mod run_history;
pub mod security;
pub mod settings;
pub mod shared;
pub mod sources;
pub mod timeline;
pub mod timeline_view;
pub mod tools;
pub mod validation;
pub mod video_source;
pub mod whisper;

pub use api::{ImportedAudio, JobStartResult, ToolStatus};
pub use clip_engine;
pub use paths::AppPaths;

use jobs::JobSnapshot;
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// What the platform's native window glass can do; the shells draw it.
pub mod window_glass {
    /// Whether this platform draws native glass at all.
    pub const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "windows"));
    /// Whether the blur radius can be changed (macOS WindowServer blur).
    pub const BLUR_SUPPORTED: bool = cfg!(target_os = "macos");
}

pub struct Core {
    pub paths: AppPaths,
    pub settings: settings::SettingsStore,
    pub media: security::MediaRegistry,
    pub whisper: whisper::WhisperModels,
    pub jobs: jobs::JobManager,
    pub previews: preview::Previews,
    /// Output folders the user picked with the folder dialog this session; settings
    /// accept no other new output folder.
    pub selected_output_dirs: Mutex<HashSet<String>>,
    /// Runs the job queue's tasks, wherever its methods are called from.
    pub(crate) runtime: tokio::runtime::Handle,
    /// Called with every job snapshot change (the shell's `jobs:update`).
    pub(crate) on_job: Box<dyn Fn(&JobSnapshot) + Send + Sync>,
}

impl Core {
    /// Start the app's logic: logging, PATH for per-user tools, and a clean slate for
    /// stale work folders and previews. `on_job` receives every job update.
    pub fn start(
        paths: AppPaths,
        runtime: tokio::runtime::Handle,
        version: &str,
        on_job: impl Fn(&JobSnapshot) + Send + Sync + 'static,
    ) -> Arc<Self> {
        logger::init(&paths.logs);
        // The engine and the CLIs it launches find per-user installs (Homebrew, ~/.local/bin).
        tools::augment_process_path(&paths.home);
        jobs::clean_stale_workspaces(&paths.user_data, &HashSet::new());
        preview::clear(&paths.user_data);
        logger::info(
            "app.boot",
            json!({
                "version": version,
                "platform": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "isPackaged": paths.packaged,
                "userData": paths.user_data.to_string_lossy(),
                "logFile": logger::log_file_path().to_string_lossy(),
            }),
        );
        let core = Arc::new(Self {
            settings: settings::SettingsStore::new(
                &paths.user_data,
                paths.legacy_user_data.as_deref(),
                &paths.home,
            ),
            whisper: whisper::WhisperModels::new(&paths.user_data),
            media: security::MediaRegistry::default(),
            jobs: jobs::JobManager::default(),
            previews: preview::Previews::new(),
            selected_output_dirs: Mutex::new(HashSet::new()),
            paths,
            runtime,
            on_job: Box::new(on_job),
        });
        let sweep = core.clone();
        core.runtime
            .spawn_blocking(move || sources::apply_retention(&sweep));
        core
    }

    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.runtime
    }
}
