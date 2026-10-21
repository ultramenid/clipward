//! App-wide state the views share: settings with setup readiness, the
//! session's jobs, and app updates. Ports `use-settings-store.ts` and `use-job-store.ts`.
//!
//! Settings calls are synchronous (a cached struct and a small JSON write), so
//! they run on the UI thread in order; the TS store's save queue isn't needed.
use crate::state;
use clipward_core::ToolStatus;
use clipward_core::ai_providers::DetectedCli;
use clipward_core::appearance::Appearance;
use clipward_core::jobs::JobSnapshot;
use clipward_core::settings::{EditableSettings, PublicSettings};
use clipward_core::job_list::JobList;
use clipward_core::readiness;
use clipward_core::updater::{self, Update};
use clipward_core::whisper::WhisperModelStatus;
use gpui_kit::{App, AppContext, Context, Entity, EventEmitter, Global, Task};

pub use clipward_core::readiness::{LOCAL_PROVIDER, SetupState, setup_blocked_reason};

pub struct Settings {
    pub value: Option<PublicSettings>,
    pub load_error: Option<String>,
    /// The last failed save, shown by the Settings page.
    pub save_error: Option<String>,
    pub tool_status: Option<ToolStatus>,
    pub checking_tools: bool,
    pub tool_error: Option<String>,
    /// Supported CLIs and where each is installed; None until detected.
    pub clis: Option<Vec<DetectedCli>>,
    pub whisper_models: Vec<WhisperModelStatus>,
    /// A catalog refresh from the Hugging Face repo is running.
    pub checking_whisper: bool,
    tool_check: Option<Task<()>>,
}

/// Emitted after every successful settings change, with the new value.
pub struct SettingsChanged;

impl EventEmitter<SettingsChanged> for Settings {}

impl Settings {
    fn new() -> Self {
        Self {
            value: None,
            load_error: None,
            save_error: None,
            tool_status: None,
            checking_tools: false,
            tool_error: None,
            clis: None,
            whisper_models: Vec::new(),
            checking_whisper: false,
            tool_check: None,
        }
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalModels>().settings.clone()
    }

    pub fn appearance(&self) -> Appearance {
        self.value
            .as_ref()
            .map(|s| s.appearance.clone())
            .unwrap_or_default()
    }

    /// Read settings, installed CLIs and Whisper models (boot and Retry).
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let core = state::core(cx);
        match core.settings_load() {
            Ok(settings) => {
                self.value = Some(settings);
                self.load_error = None;
                self.clis = Some(core.ai_detect());
                self.whisper_models = core.whisper_list();
                // New or changed models in the repo show without a click; offline keeps the cache.
                self.refresh_whisper_models(cx).detach();
                cx.emit(SettingsChanged);
            }
            Err(message) => self.load_error = Some(message),
        }
        cx.notify();
    }

    fn take(&mut self, result: Result<PublicSettings, String>, cx: &mut Context<Self>) -> bool {
        match result {
            Ok(saved) => {
                self.value = Some(saved);
                self.save_error = None;
                cx.emit(SettingsChanged);
                cx.notify();
                true
            }
            Err(message) => {
                self.save_error = Some(message);
                cx.notify();
                false
            }
        }
    }

    /// Save the editable fields after `edit` changes them.
    pub fn save(&mut self, edit: impl FnOnce(&mut EditableSettings), cx: &mut Context<Self>) -> bool {
        let Some(current) = self.value.as_ref() else {
            return false;
        };
        let mut update = editable(current);
        edit(&mut update);
        let result = state::core(cx).settings_save(update);
        self.take(result, cx)
    }

    pub fn replace_api_key(&mut self, key: &str, value: &str, cx: &mut Context<Self>) -> bool {
        let result = state::core(cx).settings_replace_api_key(key, value);
        self.take(result, cx)
    }

    pub fn set_window_glass(&mut self, enabled: bool, cx: &mut Context<Self>) -> bool {
        let result = state::core(cx).set_window_glass(enabled);
        self.take(result, cx)
    }

    pub fn set_source_retention(&mut self, days: Option<u32>, cx: &mut Context<Self>) -> bool {
        let result = state::core(cx).set_source_retention(days);
        self.take(result, cx)
    }

    pub fn set_appearance(&mut self, appearance: Appearance, cx: &mut Context<Self>) -> bool {
        let result = state::core(cx).set_appearance(appearance);
        self.take(result, cx)
    }

    pub fn detect_clis(&mut self, cx: &mut Context<Self>) {
        self.clis = Some(state::core(cx).ai_detect());
        cx.notify();
    }

    pub fn reload_whisper_models(&mut self, cx: &mut Context<Self>) {
        self.whisper_models = state::core(cx).whisper_list();
        cx.notify();
    }

    /// Re-read the model list from the Hugging Face repo.
    pub fn refresh_whisper_models(&mut self, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        self.checking_whisper = true;
        cx.notify();
        let core = state::core(cx);
        let refresh = state::run(cx, async move { core.whisper_refresh().await });
        cx.spawn(async move |this, cx| {
            let result = refresh.await;
            let _ = this.update(cx, |this, cx| {
                this.checking_whisper = false;
                // A download in flight keeps its progress; the list itself comes fresh.
                this.whisper_models = state::core(cx).whisper_list();
                cx.notify();
            });
            result.map(|_| ())
        })
    }

    pub fn set_whisper_status(&mut self, status: WhisperModelStatus, cx: &mut Context<Self>) {
        for model in &mut self.whisper_models {
            if model.id == status.id {
                *model = status.clone();
            }
        }
        cx.notify();
    }

    /// Run the system check; a newer check replaces an older one still running.
    pub fn check_tools(&mut self, cx: &mut Context<Self>) {
        self.checking_tools = true;
        self.tool_error = None;
        cx.notify();
        let core = state::core(cx);
        let check = state::run(cx, async move { Ok(core.system_check_tools().await) });
        self.tool_check = Some(cx.spawn(async move |this, cx| {
            let result = check.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(status) => this.tool_status = Some(status),
                    Err(_) => {
                        this.tool_status = None;
                        this.tool_error =
                            Some("Could not check required tools. Try again in Settings.".into());
                    }
                }
                this.checking_tools = false;
                cx.notify();
            });
        }));
    }

    /// Whether the selected provider is usable, from settings alone (no network).
    pub fn provider_ready(&self) -> bool {
        self.value
            .as_ref()
            .is_some_and(|s| readiness::provider_ready(s, self.clis.as_deref()))
    }

    pub fn setup(&self) -> SetupState {
        match self.value.as_ref() {
            Some(s) => readiness::setup_state(
                s,
                self.clis.as_deref(),
                &self.whisper_models,
                self.tool_status.as_ref(),
                self.tool_error.is_some(),
                self.checking_tools,
            ),
            None => SetupState::default(),
        }
    }
}

/// The fields settings:save takes, from the current settings.
pub fn editable(s: &PublicSettings) -> EditableSettings {
    EditableSettings {
        output_directory: s.output_directory.clone(),
        custom_vocabulary: s.custom_vocabulary.clone(),
        ai_provider: s.ai_provider.clone(),
        ai_model: s.ai_model.clone(),
        ai_effort: Some(s.ai_effort.clone()),
        whisper_model: s.whisper_model.clone(),
        local_base_url: Some(s.local_base_url.clone()),
        local_model: Some(s.local_model.clone()),
        local_images: Some(s.local_images),
    }
}

/// The session's clipping jobs, kept in step with the core's job queue. Reads go
/// through `JobList` (`jobs.jobs`, `jobs.focused`, `jobs.active()`).
#[derive(Default)]
pub struct Jobs(JobList);

impl std::ops::Deref for Jobs {
    type Target = JobList;
    fn deref(&self) -> &JobList {
        &self.0
    }
}

impl Jobs {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalModels>().jobs.clone()
    }

    /// Apply a snapshot from the queue; older revisions are ignored.
    pub fn upsert(&mut self, job: JobSnapshot, cx: &mut Context<Self>) {
        if self.0.upsert(job) {
            cx.notify();
        }
    }

    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.0.remove(id);
        cx.notify();
    }

    pub fn focus(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.0.focused = id;
        cx.notify();
    }
}

const UPDATE_RECHECK: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// Where an app update stands (`clipward_core::updater`).
#[derive(Clone, Debug, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    Latest,
    Available(Update),
    Downloading { update: Update, received: u64, total: u64 },
    /// Installed; the app is about to restart.
    Installing(Update),
    Failed { message: String, update: Option<Update> },
}

/// Checks GitHub for a newer release at launch and installs it on request.
#[derive(Default)]
pub struct Updater {
    pub state: UpdateState,
    task: Option<Task<()>>,
}

impl Updater {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalModels>().updater.clone()
    }

    /// Whether this copy can update itself (an installed app, not a dev build).
    pub fn installable() -> bool {
        updater::install_target().is_some()
    }

    pub fn available(&self) -> Option<&Update> {
        match &self.state {
            UpdateState::Available(update) | UpdateState::Failed { update: Some(update), .. } => Some(update),
            _ => None,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self.state, UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Installing(_))
    }

    pub fn check(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.state = UpdateState::Checking;
        cx.notify();
        let check = state::run(cx, updater::check(env!("CARGO_PKG_VERSION")));
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = check.await;
            let _ = this.update(cx, |this, cx| {
                this.state = match result {
                    Ok(Some(update)) => UpdateState::Available(update),
                    Ok(None) => UpdateState::Latest,
                    Err(message) => UpdateState::Failed { message, update: None },
                };
                cx.notify();
            });
        }));
    }

    /// Download, verify and swap in the update, then restart into it.
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let Some(update) = self.available().cloned() else { return };
        self.state = UpdateState::Downloading { update: update.clone(), received: 0, total: update.size };
        cx.notify();
        let dir = state::core(cx).paths.user_data.join("updates");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, u64)>();
        let job = {
            let update = update.clone();
            state::run(cx, async move {
                let package = updater::download(&update, &dir, move |got, total| {
                    let _ = tx.send((got, total));
                })
                .await?;
                tokio::task::spawn_blocking(move || updater::install(&package))
                    .await
                    .map_err(|_| "The update couldn't be installed.".to_string())?
            })
        };
        self.task = Some(cx.spawn(async move |this, cx| {
            let progress = this.clone();
            let feed = cx.spawn(async move |cx| {
                while let Some((received, total)) = rx.recv().await {
                    let _ = progress.update(cx, |this, cx| {
                        if let UpdateState::Downloading { update, .. } = &this.state {
                            this.state = UpdateState::Downloading { update: update.clone(), received, total };
                            cx.notify();
                        }
                    });
                }
            });
            let result = job.await;
            drop(feed);
            let _ = this.update(cx, |this, cx| match result {
                Ok(launch) => {
                    if updater::relaunch(&launch).is_ok() {
                        this.state = UpdateState::Installing(update);
                        cx.quit();
                    } else {
                        // Installed, just not restarted.
                        this.state = UpdateState::Failed {
                            message: format!("Updated to {}. Quit and reopen Clipward to use it.", update.version),
                            update: None,
                        };
                    }
                    cx.notify();
                }
                Err(message) => {
                    this.state = UpdateState::Failed { message, update: Some(update) };
                    cx.notify();
                }
            });
        }));
    }
}

struct GlobalModels {
    settings: Entity<Settings>,
    jobs: Entity<Jobs>,
    updater: Entity<Updater>,
}

impl Global for GlobalModels {}

/// Create the shared models and feed job updates from the core into `Jobs`.
pub fn init(job_updates: tokio::sync::mpsc::UnboundedReceiver<JobSnapshot>, cx: &mut App) {
    let settings = cx.new(|_| Settings::new());
    let jobs = cx.new(|_| Jobs::default());
    let feed = jobs.downgrade();
    let mut updates = job_updates;
    cx.spawn(async move |cx| {
        while let Some(job) = updates.recv().await {
            if feed.update(cx, |jobs, cx| jobs.upsert(job, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
    let updater = cx.new(|cx| {
        let mut updater = Updater::default();
        updater::cleanup(&state::core(cx).paths.user_data.join("updates"));
        if Updater::installable() {
            updater.check(cx);
            // An app left open for days still hears about new versions.
            cx.spawn(async |this, cx| loop {
                cx.background_executor().timer(UPDATE_RECHECK).await;
                let Ok(()) = this.update(cx, |u, cx| {
                    if !matches!(u.state, UpdateState::Available(_)) {
                        u.check(cx)
                    }
                }) else {
                    break;
                };
            })
            .detach();
        }
        updater
    });
    cx.set_global(GlobalModels { settings, jobs, updater });
}
