//! Pick who plans the clips: a coding CLI the user is already logged in to (billed
//! to that plan), their own API key, or a model on their own server (Local AI).
//! Ports ProviderSetup.tsx, LocalAiSetup.tsx, ApiKeyInput.tsx and
//! use-api-key-drafts.ts. Transcription never uses this; it runs offline.
//!
//! API keys stay drafts until saved: 600 ms after typing stops, on blur, before a
//! test or model detection, and when the view closes. `compact` (the Create
//! page's setup card) lists only installed CLIs and leaves out the model field.
use crate::models::Settings;
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, Btn, Hue, TypeScale, Truncate, u};
use crate::{shell, state};
use clipward_core::ai_providers::{LocalModelsResult, ProviderTestResult};
use clipward_core::clip_engine::llm::ModelChoice;
use clipward_core::provider_catalog::{
    self as catalog, API_PROVIDERS, LOCAL_DEFAULT_BASE_URL, LOCAL_EXAMPLE_MODEL, LOCAL_ID, LOCAL_KEY,
};
use clipward_core::settings::PublicSettings;
use clipward_core::shared::AI_CLIS;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{Sizable, Size};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::time::Duration;

use super::widgets::{Choice, error_text, eyebrow, field, option_tile, refresh_button, tile_columns};

type ModelSelect = SelectState<SearchableVec<Choice>>;

/// A key field's local draft: never persisted until edited (use-api-key-drafts.ts).
struct KeyDraft {
    name: &'static str,
    input: Entity<InputState>,
    dirty: bool,
    visible: bool,
}

pub struct ProviderSetup {
    compact: bool,
    settings: Entity<Settings>,
    keys: Vec<KeyDraft>,
    key_error: Option<SharedString>,
    /// Waits 600 ms after typing in a key field, then saves it.
    key_debounce: Option<Task<()>>,
    error: Option<SharedString>,
    test: Option<ProviderTestResult>,
    testing: bool,
    test_task: Option<Task<()>>,
    detecting: bool,
    detect_task: Option<Task<()>>,
    models: Vec<ModelChoice>,
    models_error: Option<SharedString>,
    loading_models: bool,
    /// The (provider, has key) the model list was fetched for.
    models_for: Option<(String, bool)>,
    models_task: Option<Task<()>>,
    model_input: Entity<InputState>,
    model_select: Entity<ModelSelect>,
    // Local AI
    url_input: Entity<InputState>,
    local_model_input: Entity<InputState>,
    detected: Option<LocalModelsResult>,
    detecting_local: bool,
    local_error: Option<SharedString>,
    local_task: Option<Task<()>>,
    was_local: bool,
    /// The settings the fields were last brought in step with (Debug text).
    synced: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ProviderSetup {
    pub fn new(compact: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::global(cx);
        let mut subscriptions = vec![cx.observe_in(&settings, window, |this, _, window, cx| {
            this.sync(window, cx);
        })];
        let model_input = cx.new(|cx| InputState::new(window, cx));
        let url_input = cx.new(|cx| InputState::new(window, cx).placeholder(LOCAL_DEFAULT_BASE_URL));
        let local_model_input = cx.new(|cx| InputState::new(window, cx).placeholder(LOCAL_EXAMPLE_MODEL));
        let names = API_PROVIDERS.iter().map(|p| p.key).chain([LOCAL_KEY]);
        let keys: Vec<KeyDraft> = names
            .map(|name| KeyDraft {
                name,
                input: cx.new(|cx| InputState::new(window, cx).masked(true)),
                dirty: false,
                visible: false,
            })
            .collect();
        for (ix, key) in keys.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(&key.input, window, move |this, _, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::Change => {
                        this.keys[ix].dirty = true;
                        this.key_debounce = Some(cx.spawn_in(window, async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(600)).await;
                            let _ = this.update_in(cx, |this, window, cx| this.persist_keys(window, cx));
                        }));
                        cx.notify();
                    }
                    InputEvent::Blur => {
                        this.persist_keys(window, cx);
                    }
                    _ => {}
                }
            }));
        }
        subscriptions.push(cx.subscribe_in(&model_input, window, |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Blur) {
                let draft = input.read(cx).value().trim().to_string();
                if draft != this.saved(cx).ai_model {
                    this.save_model(draft, cx);
                }
            }
        }));
        subscriptions.push(cx.subscribe_in(&url_input, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Blur) {
                this.save_url(cx);
            }
        }));
        subscriptions.push(cx.subscribe_in(&local_model_input, window, |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Blur) {
                let draft = input.read(cx).value().to_string();
                this.save_local_model(&draft, cx);
            }
        }));
        let model_select = cx.new(|cx| {
            SelectState::new(SearchableVec::new(Vec::<Choice>::new()), None, window, cx).searchable(true)
        });
        subscriptions.push(cx.subscribe_in(
            &model_select,
            window,
            |this, _, event: &SelectEvent<SearchableVec<Choice>>, _, cx| {
                let SelectEvent::Confirm(Some(value)) = event else { return };
                this.save_model(value.to_string(), cx);
            },
        ));
        // Navigation may close the page before the debounce fires.
        cx.on_release(|this, cx| {
            let pending: Vec<(&'static str, String)> = this
                .keys
                .iter()
                .filter(|key| key.dirty)
                .map(|key| (key.name, key.input.read(cx).value().trim().to_string()))
                .filter(|(_, value)| !value.is_empty())
                .collect();
            this.settings.update(cx, |settings, cx| {
                for (name, value) in pending {
                    settings.replace_api_key(name, &value, cx);
                }
            });
        })
        .detach();

        let mut this = Self {
            compact,
            settings,
            keys,
            key_error: None,
            key_debounce: None,
            error: None,
            test: None,
            testing: false,
            test_task: None,
            detecting: false,
            detect_task: None,
            models: Vec::new(),
            models_error: None,
            loading_models: false,
            models_for: None,
            models_task: None,
            model_input,
            model_select,
            url_input,
            local_model_input,
            detected: None,
            detecting_local: false,
            local_error: None,
            local_task: None,
            was_local: false,
            synced: None,
            _subscriptions: subscriptions,
        };
        this.sync(window, cx);
        this
    }

    fn saved(&self, cx: &App) -> PublicSettings {
        self.settings.read(cx).value.clone().expect("settings are loaded before pages open")
    }

    fn save_error(&self, cx: &App, fallback: &str) -> SharedString {
        self.settings.read(cx).save_error.clone().unwrap_or_else(|| fallback.into()).into()
    }

    fn configured(s: &PublicSettings, key: &str) -> bool {
        match key {
            "openrouterApiKey" => s.openrouter_configured,
            "anthropicApiKey" => s.anthropic_configured,
            "openaiApiKey" => s.openai_configured,
            _ => s.local_configured,
        }
    }

    /// Bring the fields in step with saved settings (not while the user edits
    /// them), refetch models when the provider or its key changes, and list the
    /// local server's models when Local AI is picked.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.saved(cx);
        // Whisper progress and tool checks notify too; leave open fields alone then.
        let snapshot = format!("{s:?}");
        if self.synced.as_ref() == Some(&snapshot) {
            cx.notify();
            return;
        }
        self.synced = Some(snapshot);
        for key in &self.keys {
            let placeholder = if Self::configured(&s, key.name) {
                "Saved securely. Paste a new key to replace."
            } else if key.name == LOCAL_KEY {
                "Only if your server asks for one"
            } else {
                API_PROVIDERS.iter().find(|p| p.key == key.name).map_or("", |p| p.placeholder)
            };
            key.input.update(cx, |input, cx| input.set_placeholder(placeholder, window, cx));
        }
        let api = catalog::api_provider(&s.ai_provider);
        let placeholder = api.map_or("CLI default", |api| api.default_model);
        self.model_input.update(cx, |input, cx| input.set_placeholder(placeholder, window, cx));
        for (input, value) in [
            (&self.model_input, s.ai_model.clone()),
            (&self.url_input, s.local_base_url.clone()),
            (&self.local_model_input, s.local_model.clone()),
        ] {
            if !input.read(cx).focus_handle(cx).is_focused(window) && input.read(cx).value() != value {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
        }
        let selected = SharedString::from(s.ai_model.clone());
        self.model_select.update(cx, |select, cx| {
            if select.selected_value() != Some(&selected) {
                select.set_selected_value(&selected, window, cx);
            }
        });

        let cli = s.ai_provider.starts_with("cli:");
        let has_key = api.is_some_and(|api| Self::configured(&s, api.key));
        let wanted = (s.ai_provider.clone(), has_key);
        if self.models_for.as_ref() != Some(&wanted) {
            self.models_for = Some(wanted);
            self.load_models(!self.compact && (cli || has_key), window, cx);
        }
        let local = s.ai_provider == LOCAL_ID;
        if local && !self.was_local {
            // LocalAiSetup lists the server's models once when it appears.
            self.detected = None;
            self.local_error = None;
            self.detect_local(window, cx);
        }
        self.was_local = local;
        cx.notify();
    }

    fn load_models(&mut self, fetch: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.models.clear();
        self.models_error = None;
        self.loading_models = fetch;
        self.models_task = None;
        if !fetch {
            return;
        }
        let core = state::core(cx);
        let task = state::run(cx, async move { core.ai_models().await });
        self.models_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.loading_models = false;
                match result {
                    Ok(models) => this.models = models,
                    Err(message) => this.models_error = Some(message.into()),
                }
                this.refresh_model_options(window, cx);
                cx.notify();
            });
        }));
    }

    fn refresh_model_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.saved(cx);
        let default_detail = catalog::api_provider(&s.ai_provider).map_or("CLI default", |api| api.default_model);
        let mut options = vec![Choice { detail: Some(default_detail.into()), ..Choice::new("", "Default") }];
        options.extend(self.models.iter().map(|model| Choice {
            detail: (model.label != model.id).then(|| model.id.clone().into()),
            ..Choice::new(model.id.clone(), model.label.clone())
        }));
        let selected = SharedString::from(s.ai_model);
        self.model_select.update(cx, |select, cx| {
            select.set_items(SearchableVec::new(options), window, cx);
            select.set_selected_value(&selected, window, cx);
        });
    }

    fn choose(&mut self, provider: String, cx: &mut Context<Self>) {
        self.test = None;
        // A model name only means something to the provider it was written for.
        let ok = self.settings.update(cx, |s, cx| {
            s.save(
                |e| {
                    e.ai_provider = provider;
                    e.ai_model = String::new();
                },
                cx,
            )
        });
        self.error = (!ok).then(|| self.save_error(cx, "Could not save the provider"));
        cx.notify();
    }

    fn save_model(&mut self, model: String, cx: &mut Context<Self>) {
        let ok = self.settings.update(cx, |s, cx| s.save(|e| e.ai_model = model, cx));
        self.error = (!ok).then(|| self.save_error(cx, "Invalid model name"));
        cx.notify();
    }

    /// Save every edited, non-empty key; a saved field empties.
    fn persist_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.key_debounce = None;
        let mut saved_any = false;
        for ix in 0..self.keys.len() {
            let value = self.keys[ix].input.read(cx).value().trim().to_string();
            if !self.keys[ix].dirty || value.is_empty() {
                continue;
            }
            let name = self.keys[ix].name;
            if !self.settings.update(cx, |s, cx| s.replace_api_key(name, &value, cx)) {
                self.key_error = Some(self.save_error(cx, "Could not save your API keys"));
                cx.notify();
                return false;
            }
            self.keys[ix].input.update(cx, |input, cx| input.set_value("", window, cx));
            // Emptying the field is not an edit.
            self.keys[ix].dirty = false;
            saved_any = true;
        }
        if saved_any {
            self.key_error = None;
            cx.notify();
        }
        true
    }

    fn remove_key(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.persist_keys(window, cx) {
            return;
        }
        let name = self.keys[ix].name;
        if self.settings.update(cx, |s, cx| s.replace_api_key(name, "", cx)) {
            self.keys[ix].input.update(cx, |input, cx| input.set_value("", window, cx));
            self.keys[ix].dirty = false;
            self.key_error = None;
        } else {
            self.key_error = Some(self.save_error(cx, "Could not remove the API key"));
        }
        cx.notify();
    }

    fn toggle_visible(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let key = &mut self.keys[ix];
        key.visible = !key.visible;
        let masked = !key.visible;
        key.input.update(cx, |input, cx| input.set_masked(masked, window, cx));
        cx.notify();
    }

    fn run_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.testing = true;
        self.test = None;
        cx.notify();
        if !self.persist_keys(window, cx) {
            self.testing = false;
            let message = self.key_error.clone().unwrap_or_default().to_string();
            self.test = Some(ProviderTestResult { ok: false, message });
            return;
        }
        let core = state::core(cx);
        let task = state::run(cx, async move { core.ai_test().await });
        self.test_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.testing = false;
                this.test = Some(result.unwrap_or_else(|message| ProviderTestResult { ok: false, message }));
                cx.notify();
            });
        }));
    }

    fn redetect(&mut self, cx: &mut Context<Self>) {
        self.detecting = true;
        self.settings.update(cx, |s, cx| s.detect_clis(cx));
        // Detection is quick; the icon still makes one whole turn.
        self.detect_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(700)).await;
            let _ = this.update(cx, |this, cx| {
                this.detecting = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    // ---- Local AI ----------------------------------------------------------

    /// Save the server URL if it changed; false when it was refused.
    fn save_url(&mut self, cx: &mut Context<Self>) -> bool {
        let draft = self.url_input.read(cx).value().trim().to_string();
        if draft == self.saved(cx).local_base_url {
            return true;
        }
        let ok = self.settings.update(cx, |s, cx| s.save(|e| e.local_base_url = Some(draft), cx));
        if ok {
            self.local_error = None;
            self.test = None;
        } else {
            self.local_error = Some(self.save_error(cx, "Invalid server URL"));
        }
        cx.notify();
        ok
    }

    fn save_local_model(&mut self, value: &str, cx: &mut Context<Self>) {
        let model = value.trim().to_string();
        if model == self.saved(cx).local_model {
            return;
        }
        if self.settings.update(cx, |s, cx| s.save(|e| e.local_model = Some(model), cx)) {
            self.local_error = None;
            self.test = None;
        } else {
            self.local_error = Some(self.save_error(cx, "Invalid model name"));
        }
        cx.notify();
    }

    fn detect_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.save_url(cx) {
            return;
        }
        self.persist_keys(window, cx);
        self.detecting_local = true;
        let core = state::core(cx);
        let task = state::run(cx, async move { core.ai_local_models().await });
        self.local_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.detected = Some(result.unwrap_or_else(|message| LocalModelsResult {
                    ok: false,
                    models: Vec::new(),
                    message,
                }));
                this.detecting_local = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    // ---- Rendering -----------------------------------------------------------

    /// ApiKeyInput.tsx.
    #[allow(clippy::too_many_arguments)]
    fn key_field(
        &self,
        ix: usize,
        label: String,
        configured: bool,
        url: Option<&'static str>,
        description: &'static str,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = &self.keys[ix];
        let has_value = !key.input.read(cx).value().trim().is_empty();
        let t = *t;
        let label = div()
            .flex()
            .items_center()
            .gap(u(8.))
            .child(label)
            .when(has_value || configured, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(4.))
                        .h(u(18.))
                        .px(u(6.))
                        .rounded(u(4.))
                        .border_1()
                        .border_color(t.success.opacity(0.25))
                        .bg(t.success.opacity(0.1))
                        .text_meta()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.success)
                        .child(ui::icon("check").size(u(10.)).text_color(t.success))
                        .child("Added"),
                )
            });
        let link = |id: &'static str, text: &'static str, danger: bool| {
            div()
                .id((id, ix))
                .flex()
                .items_center()
                .gap(u(2.))
                .rounded(u(6.))
                .px(u(8.))
                .py(u(4.))
                .text_caption()
                .text_color(t.ink_secondary)
                .cursor_pointer()
                .hover(move |s| {
                    if danger {
                        s.bg(t.danger.opacity(0.1)).text_color(t.danger)
                    } else {
                        s.bg(t.tint(0.1)).text_color(t.ink)
                    }
                })
                .child(text)
        };
        let aside = div()
            .flex()
            .items_center()
            .gap(u(4.))
            .when(configured, |d| {
                d.child(
                    link("remove-key", "Remove key", true)
                        .on_click(cx.listener(move |this, _, window, cx| this.remove_key(ix, window, cx))),
                )
            })
            .when_some(url, |d, url| {
                d.child(
                    link("get-key", "Get a key", false)
                        .child(ui::icon("arrow-up-right").size(u(12.)))
                        .on_click(move |_, _, cx| {
                            let _ = shell::open_url(url, cx);
                        }),
                )
            })
            .into_any_element();
        let input = Input::new(&key.input)
            .with_size(Size::Small)
            .text_caption()
            .when(key.visible && has_value, |i| i.font_family(crate::MONO_FONT))
            .prefix(ui::icon("key-round").size(u(14.)).text_color(t.ink_secondary))
            .suffix(
                Btn::new(("toggle-key", ix))
                    .ghost()
                    .small()
                    .icon(if key.visible { "eye-off" } else { "eye" })
                    .tooltip(if key.visible { "Hide key" } else { "Show key" })
                    .on_click(cx.listener(move |this, _, window, cx| this.toggle_visible(ix, window, cx))),
            );
        field(label, Some(aside), input, Some(description.into_any_element()), &t).into_any_element()
    }

    /// LocalAiSetup.tsx.
    fn local_setup(&self, s: &PublicSettings, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let t = *t;
        let code = |text: String| div().font_family(crate::MONO_FONT).text_color(t.ink).child(text);
        let intro = div()
            .max_w(u(560.))
            .flex()
            .flex_wrap()
            .text_caption()
            .text_color(t.ink_secondary)
            .child("Plans clips on your own machine with Ollama, LM Studio, llama.cpp or vLLM. For Ollama, install it, run\u{a0}")
            .child(code(format!("ollama pull {LOCAL_EXAMPLE_MODEL}")))
            .child("\u{a0}and keep it open; the URL below is its default (LM Studio uses\u{a0}")
            .child(code("http://localhost:1234/v1".into()))
            .child("). Planning requests go only to this server, and nothing is billed.");
        let models = self.detected.as_ref().map(|d| d.models.clone()).unwrap_or_default();
        let hint: AnyElement = match &self.detected {
            Some(d) if !d.ok => div().text_color(t.danger).child(d.message.clone()).into_any_element(),
            Some(d) if models.is_empty() => d.message.clone().into_any_element(),
            _ => "Pick a detected model or type its name.".into_any_element(),
        };
        let chips = div().flex().flex_wrap().gap(u(6.)).pt(u(4.)).children(models.iter().enumerate().map(|(i, model)| {
            let selected = *model == s.local_model;
            let pick = model.clone();
            option_tile(("local-model", i), selected, &t)
                .rounded(u(6.))
                .max_w_full()
                .px(u(8.))
                .py(u(2.))
                .font_family(crate::MONO_FONT)
                .text_caption()
                .text_color(t.ink)
                .one_line()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.local_model_input.update(cx, |input, cx| input.set_value(pick.clone(), window, cx));
                    this.save_local_model(&pick, cx);
                }))
                .child(model.clone())
        }));
        let detect = Btn::new("detect-models")
            .ghost()
            .small()
            .icon("search")
            .label("Detect models")
            .loading(self.detecting_local)
            .on_click(cx.listener(|this, _, window, cx| this.detect_local(window, cx)))
            .into_any_element();
        let weak = cx.weak_entity();
        let images = ui::switch(
            "local-images",
            s.local_images,
            false,
            move |on, _, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if this.settings.update(cx, |s, cx| s.save(|e| e.local_images = Some(on), cx)) {
                        this.test = None;
                    } else {
                        this.local_error = Some(this.save_error(cx, "Could not save the setting"));
                    }
                    cx.notify();
                });
            },
            &t,
        );
        let key_ix = self.keys.len() - 1;
        div()
            .mt(u(12.))
            .flex()
            .flex_col()
            .gap(u(12.))
            .child(intro)
            .child(field(
                "Server URL",
                None,
                Input::new(&self.url_input).with_size(Size::Small).font_family(crate::MONO_FONT).text_caption(),
                Some("The server's OpenAI-compatible address, usually ending in /v1.".into_any_element()),
                &t,
            ))
            .child(field(
                "Model",
                Some(detect),
                div()
                    .flex()
                    .flex_col()
                    .gap(u(6.))
                    .child(Input::new(&self.local_model_input).with_size(Size::Small).font_family(crate::MONO_FONT).text_caption())
                    .when(!models.is_empty(), |d| d.child(chips)),
                Some(hint),
                &t,
            ))
            .child(ui::setting_row(
                "Model accepts images",
                Some("Turn on for vision models (for example qwen2.5vl or gemma3) so planning can look at video frames. Leave off for text-only models.".into()),
                images,
                false,
                &t,
            ))
            .child(self.key_field(
                key_ix,
                "API key (optional)".into(),
                s.local_configured,
                None,
                "Encrypted with your system keychain. Only sent to this server.",
                &t,
                cx,
            ))
            .when_some(self.local_error.clone(), |d, message| d.child(error_text(message, &t)))
            .into_any_element()
    }
}

impl Render for ProviderSetup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let s = self.saved(cx);
        let columns = tile_columns(window);
        let clis = self.settings.read(cx).clis.clone().unwrap_or_default();
        let visible: Vec<_> = clis.into_iter().filter(|cli| !self.compact || cli.path.is_some()).collect();
        let api = catalog::api_provider(&s.ai_provider);
        let local = s.ai_provider == LOCAL_ID;
        let has_key = api.is_some_and(|api| Self::configured(&s, api.key));

        let cli_section = div()
            .child(
                div()
                    .mb(u(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(eyebrow("Use a CLI you're logged in to", &t))
                    .child(
                        refresh_button("detect-clis", "Detect again", self.detecting, true, &t)
                            .when(!self.detecting, |b| b.on_click(cx.listener(|this, _, _, cx| this.redetect(cx)))),
                    ),
            )
            .child(if visible.is_empty() {
                let names: Vec<&str> = AI_CLIS.iter().map(|(_, label, _)| *label).collect();
                div()
                    .text_caption()
                    .text_color(t.ink_secondary)
                    .child(format!(
                        "No supported CLI found. Install and log in to one of: {}. Or use an API key or a local model below.",
                        names.join(", ")
                    ))
                    .into_any_element()
            } else {
                div()
                    .grid()
                    .grid_cols(columns)
                    .gap(u(8.))
                    .children(visible.iter().enumerate().map(|(i, cli)| {
                        let id = format!("cli:{}", cli.id);
                        let selected = s.ai_provider == id;
                        let installed = cli.path.is_some();
                        option_tile(("cli", i), selected, &t)
                            .flex()
                            .items_start()
                            .gap(u(10.))
                            .px(u(12.))
                            .py(u(10.))
                            .when(!installed, |tile| tile.opacity(0.45))
                            .when(installed, |tile| {
                                tile.cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| this.choose(id.clone(), cx)))
                            })
                            .child(
                                div().mt(u(2.)).child(
                                    ui::icon("square-terminal")
                                        .size(u(16.))
                                        .text_color(if selected { t.accent_fg } else { t.ink_tertiary }),
                                ),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .child(div().text_label().text_color(t.ink).child(cli.label.clone()))
                                    .child(
                                        div()
                                            .text_meta()
                                            .text_color(t.ink_secondary)
                                            .one_line()
                                            .child(if installed { catalog::cli_hint(&cli.id) } else { "Not installed" }),
                                    ),
                            )
                    }))
                    .into_any_element()
            });

        let api_section = div()
            .child(eyebrow("Or bring your own API key", &t).mb(u(8.)))
            .child(div().grid().grid_cols(3).gap(u(8.)).children(API_PROVIDERS.iter().enumerate().map(|(i, provider)| {
                let selected = s.ai_provider == provider.id;
                let id = provider.id.to_string();
                option_tile(("api", i), selected, &t)
                    .flex()
                    .items_center()
                    .gap(u(10.))
                    .px(u(12.))
                    .py(u(10.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(id.clone(), cx)))
                    .child(
                        ui::icon("key-round")
                            .size(u(16.))
                            .text_color(if selected { t.accent_fg } else { t.ink_tertiary }),
                    )
                    .child(div().flex_1().min_w_0().one_line().text_label().text_color(t.ink).child(provider.label))
                    .when(Self::configured(&s, provider.key), |tile| tile.child(ui::badge("Key", Hue::Success, &t)))
            })))
            .when_some(api, |d, api| {
                let ix = API_PROVIDERS.iter().position(|p| p.id == api.id).unwrap_or(0);
                d.child(div().mt(u(12.)).child(self.key_field(
                    ix,
                    format!("{} API key", api.label),
                    Self::configured(&s, api.key),
                    Some(api.url),
                    "Encrypted with your system keychain. Only sent to this provider.",
                    &t,
                    cx,
                )))
            });

        let local_section = div()
            .child(eyebrow("Or run a model on your own computer", &t).mb(u(8.)))
            .child(
                div().grid().grid_cols(if columns == 3 { 3 } else { 1 }).gap(u(8.)).child(
                    option_tile("local", local, &t)
                        .when(columns == 3, |tile| tile.col_span(2))
                        .flex()
                        .items_start()
                        .gap(u(10.))
                        .px(u(12.))
                        .py(u(10.))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !local {
                                this.choose(LOCAL_ID.into(), cx);
                            }
                        }))
                        .child(
                            div()
                                .mt(u(2.))
                                .child(ui::icon("cpu").size(u(16.)).text_color(if local { t.accent_fg } else { t.ink_tertiary })),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .child(div().text_label().text_color(t.ink).child("Local AI (Ollama, LM Studio…)"))
                                .child(
                                    div()
                                        .text_meta()
                                        .text_color(t.ink_secondary)
                                        .one_line()
                                        .child("Fully offline, free, on your hardware"),
                                ),
                        ),
                ),
            )
            .when(local, |d| d.child(self.local_setup(&s, &t, cx)));

        let model_field = (!self.compact && !local).then(|| {
            let hint = if self.loading_models {
                None
            } else if let Some(error) = &self.models_error {
                Some(error.to_string())
            } else if let Some(api) = api {
                Some(if has_key {
                    format!("Default uses {}.", api.default_model)
                } else {
                    "Add a key to pick from its models.".into()
                })
            } else if !self.models.is_empty() {
                Some("Default uses the CLI’s own model.".into())
            } else {
                Some("This CLI can’t list its models. Empty uses its default.".into())
            };
            let control = if self.loading_models || !self.models.is_empty() {
                Select::new(&self.model_select)
                    .with_size(Size::Small)
                    .search_placeholder("Filter models")
                    .text_caption()
                    .when(self.loading_models, |select| select.placeholder("Loading models…").disabled(true))
                    .into_any_element()
            } else {
                Input::new(&self.model_input)
                    .with_size(Size::Small)
                    .font_family(crate::MONO_FONT)
                    .text_caption()
                    .into_any_element()
            };
            field("Model (optional)", None, control, hint.map(|h| h.into_any_element()), &t)
        });

        let test = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(u(12.))
            .child(
                Btn::new("test-connection")
                    .icon("zap")
                    .label("Test connection")
                    .loading(self.testing)
                    .on_click(cx.listener(|this, _, window, cx| this.run_test(window, cx))),
            )
            .when_some(self.test.clone(), |d, test| {
                let color = if test.ok { t.success } else { t.danger };
                d.child(
                    div()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .gap(u(6.))
                        .text_caption()
                        .text_color(color)
                        .child(ui::icon(if test.ok { "circle-check" } else { "circle-x" }).size(u(14.)).text_color(color))
                        .child(div().min_w_0().one_line().child(test.message)),
                )
            });

        div()
            .flex()
            .flex_col()
            .gap(u(16.))
            .child(cli_section)
            .child(api_section)
            .child(local_section)
            .children(model_field)
            .child(test)
            .when_some(self.error.clone().or(self.key_error.clone()), |d, message| d.child(error_text(message, &t)))
    }
}
