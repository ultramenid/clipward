//! App-wide state the views share: settings with setup readiness, and the
//! session's jobs. Ports `use-settings-store.ts` and `use-job-store.ts`.
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

struct GlobalModels {
    settings: Entity<Settings>,
    jobs: Entity<Jobs>,
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
    cx.set_global(GlobalModels { settings, jobs });
}
