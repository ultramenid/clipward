//! Settings (SettingsPage.tsx): a section nav with scroll spy, readiness tiles,
//! and plain-language sections: AI, Speech to text (with custom words), Files,
//! Look, and Help & about. Rarely needed controls fold under "More
//! options". Changes save as they are made.
mod appearance;
mod color_picker;
pub mod provider_setup;
pub mod whisper_models;
mod widgets;

pub use provider_setup::ProviderSetup;
pub use whisper_models::WhisperModels;

use crate::models::{Jobs, Settings, SettingsChanged, UpdateState, Updater};
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, Btn, Hue, Tone, TypeScale, Truncate, u};
use crate::{shell, state};
use clipward_core::ToolStatus;
use clipward_core::appearance::Appearance;
use clipward_core::provider_catalog::{
    self as catalog, APP_NAME, CLIPWARD_URL, LICENSE_NAME, RETENTION_OPTIONS,
};
use clipward_core::shared::{ISSUES_URL, REPO_URL};
use clipward_core::sources::SourceUsage;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::slider::SliderState;
use gpui_kit::component::{Sizable, Size};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use widgets::{Choice, error_text, refresh_button, tile_columns};

const SECTIONS: [(&str, &str); 5] = [
    ("AI", "sparkles"),
    ("Speech to text", "audio-lines"),
    ("Files", "folder-open"),
    ("Look", "app-window"),
    ("Help & about", "info"),
];
const AI: usize = 0;
const TRANSCRIPTION: usize = 1;
const SYSTEM: usize = 4;

/// How far below the top of the scroll area a section counts as reached (CSS px).
const ACTIVE_OFFSET: f32 = 48.;
/// `scroll-mt-6` and the nav's `top-6`.
const STICKY_TOP: f32 = 24.;
const NAV_WIDTH: f32 = 176.;
/// Tailwind's `lg`: the section nav shows from this window width up.
const NAV_MIN_WIDTH: f32 = 1024.;

/// Where things were laid out last frame, in content coordinates (CSS px are
/// not used here: plain pixels, scroll offset 0 at the top).
#[derive(Default)]
struct Spy {
    /// Top of each section panel.
    tops: Vec<Pixels>,
    /// The nav/content row: its top and height, and the nav's height.
    row_top: Pixels,
    row_height: Pixels,
    nav_height: Pixels,
    active: usize,
    /// A jumped-to section stays selected until the user scrolls on their own.
    pinned: bool,
}

type RetentionSelect = SelectState<SearchableVec<Choice>>;

pub struct SettingsPage {
    settings: Entity<Settings>,
    provider: Entity<ProviderSetup>,
    whisper: Entity<WhisperModels>,
    scroll: ScrollHandle,
    spy: Rc<RefCell<Spy>>,
    scroll_animation: Option<Task<()>>,
    // SaveIndicator
    save_error: Option<SharedString>,
    saved_visible: bool,
    saved_shown: bool,
    saved_timer: Option<Task<()>>,
    vocabulary: Entity<TextareaState>,
    output: Entity<InputState>,
    output_error: Option<SharedString>,
    usage: Option<SourceUsage>,
    usage_for: Option<(String, Option<u32>)>,
    sources_busy: bool,
    sources_error: Option<SharedString>,
    retention: Entity<RetentionSelect>,
    tools_expanded: bool,
    // Appearance (appearance.rs)
    /// Colors and the rail, folded by default.
    appearance_more: bool,
    appearance_draft: Option<Appearance>,
    appearance_saved_at: Option<Instant>,
    appearance_flush: Option<Task<()>>,
    sliders: Vec<(appearance::SliderKind, Entity<SliderState>)>,
    accent_open: bool,
    swatch_bounds: Rc<RefCell<Bounds<Pixels>>>,
    accent_focus: FocusHandle,
    picker: Entity<color_picker::ColorPicker>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::global(cx);
        let saved = settings.read(cx).value.clone().expect("settings are loaded before pages open");
        let provider = cx.new(|cx| ProviderSetup::new(false, window, cx));
        let whisper = cx.new(|cx| WhisperModels::new(false, cx));
        let vocabulary = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(4)
                .placeholder("Acme Energy\nMrBeast\nWhop")
                .default_value(saved.custom_vocabulary.clone())
        });
        let output = cx.new(|cx| InputState::new(window, cx).default_value(saved.output_directory.clone()));
        let options: Vec<Choice> = RETENTION_OPTIONS.iter().map(|(value, label)| Choice::new(*value, *label)).collect();
        let selected = options
            .iter()
            .position(|o| o.value == catalog::retention_value(saved.source_retention_days))
            .map(|row| gpui_kit::component::IndexPath::default().row(row));
        let retention = cx.new(|cx| SelectState::new(SearchableVec::new(options), selected, window, cx));
        let picker = cx.new(|cx| {
            color_picker::ColorPicker::new(
                saved.appearance.accent_color.as_deref().unwrap_or(clipward_core::appearance::ACCENT_COLOR_PRESETS[0]),
                window,
                cx,
            )
        });
        let sliders = appearance::SliderKind::ALL
            .iter()
            .map(|kind| (*kind, cx.new(|_| kind.state(&saved.appearance))))
            .collect::<Vec<_>>();

        let mut subscriptions = vec![
            cx.observe(&settings, |_, _, cx| cx.notify()),
            cx.observe(&Updater::global(cx), |_, _, cx| cx.notify()),
            cx.observe(&Jobs::global(cx), |_, _, cx| cx.notify()),
            cx.subscribe_in(&settings, window, |this, _, _: &SettingsChanged, window, cx| {
                this.sync(window, cx);
            }),
            cx.subscribe_in(&vocabulary, window, |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Blur) {
                    let draft = input.read(cx).value().to_string();
                    if draft != this.saved(cx).custom_vocabulary {
                        this.commit(|e| e.custom_vocabulary = draft, cx);
                    }
                }
            }),
            cx.subscribe(&retention, |this, _, event: &SelectEvent<SearchableVec<Choice>>, cx| {
                let SelectEvent::Confirm(Some(value)) = event else { return };
                this.set_retention(catalog::retention_days(value), cx);
            }),
            cx.subscribe_in(&picker, window, |this, _, event: &color_picker::ColorChanged, _, cx| {
                let mut next = this.appearance(cx);
                next.accent_color = Some(event.0.clone());
                this.change_appearance(next, true, cx);
            }),
        ];
        for (kind, slider) in &sliders {
            let kind = *kind;
            subscriptions.push(cx.subscribe(slider, move |this, _, event, cx| this.on_slider(kind, event, cx)));
        }

        let mut page = Self {
            settings,
            provider,
            whisper,
            scroll: ScrollHandle::new(),
            spy: Rc::default(),
            scroll_animation: None,
            save_error: None,
            saved_visible: false,
            saved_shown: false,
            saved_timer: None,
            vocabulary,
            output,
            output_error: None,
            usage: None,
            usage_for: None,
            sources_busy: false,
            sources_error: None,
            retention,
            tools_expanded: false,
            appearance_more: false,
            appearance_draft: None,
            appearance_saved_at: None,
            appearance_flush: None,
            sliders,
            accent_open: false,
            swatch_bounds: Rc::default(),
            accent_focus: cx.focus_handle(),
            picker,
            _subscriptions: subscriptions,
        };
        page.sync(window, cx);
        page
    }

    fn saved(&self, cx: &App) -> clipward_core::settings::PublicSettings {
        self.settings.read(cx).value.clone().expect("settings are loaded before pages open")
    }

    /// Keep the fields in step with settings changed elsewhere (zoom keys, Reset).
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.saved(cx);
        if !self.vocabulary.read(cx).focus_handle(cx).is_focused(window)
            && self.vocabulary.read(cx).value() != s.custom_vocabulary
        {
            let text = s.custom_vocabulary.clone();
            self.vocabulary.update(cx, |input, cx| input.set_value(text, window, cx));
        }
        if self.output.read(cx).value() != s.output_directory {
            let dir = s.output_directory.clone();
            self.output.update(cx, |input, cx| input.set_value(dir, window, cx));
        }
        let retention = SharedString::from(catalog::retention_value(s.source_retention_days));
        self.retention.update(cx, |select, cx| select.set_selected_value(&retention, window, cx));
        let usage_for = (s.output_directory.clone(), s.source_retention_days);
        if self.usage_for.as_ref() != Some(&usage_for) {
            self.usage_for = Some(usage_for);
            self.refresh_usage(cx);
        }
        self.sync_appearance(window, cx);
    }

    /// Show "Saved" for 2.2 s, or the error.
    fn saved_result(&mut self, ok: bool, fallback: &str, cx: &mut Context<Self>) {
        if !ok {
            let message = self.settings.read(cx).save_error.clone().unwrap_or_else(|| fallback.into());
            self.save_error = Some(message.into());
            cx.notify();
            return;
        }
        self.save_error = None;
        self.saved_visible = true;
        self.saved_shown = true;
        self.saved_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(2200)).await;
            let _ = this.update(cx, |this, cx| {
                this.saved_visible = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn commit(&mut self, edit: impl FnOnce(&mut clipward_core::settings::EditableSettings), cx: &mut Context<Self>) {
        let ok = self.settings.update(cx, |s, cx| s.save(edit, cx));
        self.saved_result(ok, "Could not save settings", cx);
    }

    // ---- Scroll spy ------------------------------------------------------------

    fn rem_scale(window: &Window) -> f32 {
        f32::from(window.rem_size()) / 16.0
    }

    fn jump(&mut self, section: usize, window: &mut Window, cx: &mut Context<Self>) {
        let target = {
            let mut spy = self.spy.borrow_mut();
            spy.pinned = true;
            spy.active = section;
            let Some(top) = spy.tops.get(section).copied() else { return };
            let max = self.scroll.max_offset().y;
            (top - px(STICKY_TOP * Self::rem_scale(window))).clamp(px(0.), max.max(px(0.)))
        };
        let from = -self.scroll.offset().y;
        let scroll = self.scroll.clone();
        // A short eased scroll, as `scrollIntoView({ behavior: 'smooth' })`.
        self.scroll_animation = Some(cx.spawn(async move |this, cx| {
            const FRAMES: u32 = 14;
            for frame in 1..=FRAMES {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let p = frame as f32 / FRAMES as f32;
                let eased = 1.0 - (1.0 - p).powi(3);
                scroll.set_offset(point(px(0.), -(from + (target - from) * eased)));
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// Record where the sections landed and pick the active one.
    fn spy_sections(&self, cx: &mut Context<Self>) -> impl Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static {
        let spy = self.spy.clone();
        let scroll = self.scroll.clone();
        let page = cx.weak_entity();
        move |bounds, window, cx| {
            let viewport = scroll.bounds();
            let offset = scroll.offset().y;
            let origin = viewport.top() + offset;
            let mut spy = spy.borrow_mut();
            // The first child is the readiness panel.
            spy.tops = bounds.iter().skip(1).map(|b| b.top() - origin).collect();
            if spy.pinned {
                return;
            }
            let scroll_top = f32::from(-offset);
            let tops: Vec<f32> = spy.tops.iter().map(|top| f32::from(*top) - scroll_top).collect();
            let active = catalog::active_section(
                &tops,
                scroll_top,
                f32::from(scroll.max_offset().y),
                f32::from(viewport.size.height),
                ACTIVE_OFFSET * Self::rem_scale(window),
            );
            if active != spy.active {
                spy.active = active;
                let page = page.clone();
                cx.defer(move |cx| {
                    let _ = page.update(cx, |_, cx| cx.notify());
                });
            }
        }
    }

    fn spy_row(&self, cx: &mut Context<Self>) -> impl Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static {
        let spy = self.spy.clone();
        let scroll = self.scroll.clone();
        let page = cx.weak_entity();
        move |bounds, _, cx| {
            let origin = scroll.bounds().top() + scroll.offset().y;
            let mut spy = spy.borrow_mut();
            if let [nav, content] = bounds.as_slice() {
                let layout = (content.top() - origin, content.size.height, nav.size.height);
                if layout != (spy.row_top, spy.row_height, spy.nav_height) {
                    (spy.row_top, spy.row_height, spy.nav_height) = layout;
                    // The nav's sticky offset depends on this layout: draw again.
                    let page = page.clone();
                    cx.defer(move |cx| {
                        let _ = page.update(cx, |_, cx| cx.notify());
                    });
                }
            }
        }
    }

    /// `position: sticky; top: 24px` for the nav: how far down its column it sits.
    fn nav_offset(&self, window: &Window) -> Pixels {
        let spy = self.spy.borrow();
        // A wheel event moves the offset past the end until prepaint clamps it: clamp
        // here too, or the nav jumps while scrolling against the bottom.
        let scroll_top = (-self.scroll.offset().y).clamp(px(0.), self.scroll.max_offset().y);
        let wanted = scroll_top + px(STICKY_TOP * Self::rem_scale(window)) - spy.row_top;
        let room = (spy.row_height - spy.nav_height).max(px(0.));
        wanted.clamp(px(0.), room)
    }

    // ---- Output and source videos -------------------------------------------------

    fn choose_output(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let Some(dir) = shell::pick_folder("Choose Output Folder").await else { return };
            let _ = this.update(cx, |this, cx| {
                let dir = state::core(cx).allow_output_dir(&dir);
                this.output_error = None;
                this.commit(|e| e.output_directory = dir, cx);
            });
        })
        .detach();
    }

    fn open_output(&mut self, cx: &mut Context<Self>) {
        let dir = self.saved(cx).output_directory;
        self.output_error = match shell::open_path(&dir, cx) {
            Ok(true) => None,
            Ok(false) => Some("Could not open the output folder".into()),
            Err(message) => Some(message.into()),
        };
        cx.notify();
    }

    fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        let core = state::core(cx);
        let task = state::run(cx, async move { core.sources_usage().await });
        cx.spawn(async move |this, cx| {
            let usage = task.await.ok();
            let _ = this.update(cx, |this, cx| {
                this.usage = usage;
                cx.notify();
            });
        })
        .detach();
    }

    fn set_retention(&mut self, days: Option<u32>, cx: &mut Context<Self>) {
        if days == self.saved(cx).source_retention_days {
            return;
        }
        self.sources_error = None;
        if !self.settings.update(cx, |s, cx| s.set_source_retention(days, cx)) {
            let message = self.settings.read(cx).save_error.clone();
            self.sources_error = Some(message.unwrap_or_else(|| "Could not save this setting".into()).into());
        }
        self.refresh_usage(cx);
        cx.notify();
    }

    fn delete_sources(&mut self, cx: &mut Context<Self>) {
        self.sources_busy = true;
        self.sources_error = None;
        cx.notify();
        let core = state::core(cx);
        let task = state::run(cx, async move { core.sources_delete_all().await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.sources_busy = false;
                if let Err(message) = result {
                    this.sources_error = Some(message.into());
                }
                this.refresh_usage(cx);
            });
        })
        .detach();
    }

    // ---- Rendering -------------------------------------------------------------------

    fn save_indicator(&self, t: &Tokens) -> AnyElement {
        if let Some(error) = &self.save_error {
            return ui::badge(error.clone(), Hue::Danger, t).into_any_element();
        }
        if self.appearance_draft.is_some() {
            return icon_badge(ui::spinner(12., t.ink_secondary), "Saving…", Hue::Neutral, t);
        }
        if !self.saved_shown {
            return div().into_any_element();
        }
        let badge = icon_badge(
            ui::icon("check").size(u(12.)).text_color(t.success).into_any_element(),
            "Saved",
            Hue::Success,
            t,
        );
        let (id, from, to) = if self.saved_visible { ("saved-in", 0.0, 1.0) } else { ("saved-out", 1.0, 0.0) };
        div()
            .child(badge)
            .with_animation(id, Animation::new(Duration::from_millis(300)), move |d, delta| {
                d.opacity(from + (to - from) * delta)
            })
            .into_any_element()
    }

    fn readiness(&self, columns: u16, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let settings = self.settings.read(cx);
        let setup = settings.setup();
        let s = self.saved(cx);
        let (tools_ok, tools_detail) = tools_summary(settings.tool_status.as_ref(), settings.checking_tools);
        let checks: [(&str, Option<bool>, String, usize, Tone); 3] = [
            (
                "AI",
                Some(setup.provider_ready),
                if setup.provider_ready {
                    catalog::provider_label(&s.ai_provider)
                } else {
                    "Choose an AI app or add a key".into()
                },
                AI,
                Tone::Warning,
            ),
            (
                "Speech to text",
                Some(setup.whisper_ready),
                if setup.whisper_ready {
                    catalog::whisper_label(&s.whisper_model).to_string()
                } else {
                    "Download a speech model".into()
                },
                TRANSCRIPTION,
                Tone::Warning,
            ),
            ("App tools", tools_ok, tools_detail, SYSTEM, Tone::Danger),
        ];
        let blocking = checks.iter().filter(|c| c.1 == Some(false)).count();
        let t = *t;
        ui::panel(&t)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .child(if blocking > 0 {
                        ui::icon_tile("sparkles", Hue::Warning, 32., &t)
                    } else {
                        ui::icon_tile("check", Hue::Success, 32., &t)
                    })
                    .child(
                        div()
                            .child(div().text_heading().text_color(t.ink).child(if blocking > 0 {
                                format!(
                                    "{blocking} thing{} to set up before clipping",
                                    if blocking == 1 { "" } else { "s" }
                                )
                            } else {
                                "Ready to clip".into()
                            }))
                            .child(div().mt(u(2.)).text_caption().text_color(t.ink_secondary).child(format!(
                                "{APP_NAME} works on this computer. Speech is turned into text here; your AI picks the best moments."
                            ))),
                    ),
            )
            .child(div().mt(u(12.)).grid().grid_cols(columns).gap(u(8.)).children(checks.into_iter().enumerate().map(
                |(i, (label, ok, detail, section, tone))| {
                    let dot = match ok {
                        None => Tone::Idle,
                        Some(true) => Tone::Success,
                        Some(false) => tone,
                    };
                    let detail_color = match ok {
                        Some(false) => tone.color(&t),
                        _ => t.ink_secondary,
                    };
                    widgets::option_tile(("check", i), false, &t)
                        .flex()
                        .items_start()
                        .gap(u(10.))
                        .px(u(12.))
                        .py(u(8.))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| this.jump(section, window, cx)))
                        .child(div().mt(u(6.)).child(ui::status_dot(dot, false, &t)))
                        .child(
                            div()
                                .min_w_0()
                                .child(div().text_label().text_color(t.ink).child(label))
                                .child(div().text_meta().one_line().text_color(detail_color).child(detail)),
                        )
                },
            )))
            .into_any_element()
    }

    fn nav(&self, offset: Pixels, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let settings = self.settings.read(cx);
        let setup = settings.setup();
        let (tools_ok, _) = tools_summary(settings.tool_status.as_ref(), settings.checking_tools);
        let tones: [Option<Tone>; SECTIONS.len()] = [
            Some(if setup.provider_ready { Tone::Success } else { Tone::Warning }),
            Some(if setup.whisper_ready { Tone::Success } else { Tone::Warning }),
            None,
            None,
            tools_ok.map(|ok| if ok { Tone::Success } else { Tone::Danger }),
        ];
        let active = self.spy.borrow().active;
        let t = *t;
        div()
            .w(u(NAV_WIDTH))
            .flex_shrink_0()
            .mt(offset)
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(
                div()
                    .px(u(8.))
                    .pb(u(4.))
                    .text_caption()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.ink_tertiary)
                    .child("Sections"),
            )
            .children(SECTIONS.iter().enumerate().map(|(i, (label, icon))| {
                let current = active == i;
                div()
                    .id(("section", i))
                    .flex()
                    .w_full()
                    .items_center()
                    .gap(u(8.))
                    .rounded(u(6.))
                    .px(u(8.))
                    .py(u(6.))
                    .text_label()
                    .cursor_pointer()
                    .map(|row| {
                        if current {
                            row.bg(t.selection).text_color(t.ink)
                        } else {
                            row.text_color(t.ink_secondary).hover(move |s| s.bg(t.tint(0.05)).text_color(t.ink))
                        }
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.jump(i, window, cx)))
                    .child(ui::icon(icon).size(u(16.)).opacity(0.7))
                    .child(div().flex_1().min_w_0().one_line().child(*label))
                    .children(tones[i].map(|tone| ui::status_dot(tone, false, &t)))
            }))
            .into_any_element()
    }

    fn vocabulary_panel(&self, saved: &str, t: &Tokens) -> AnyElement {
        let terms = catalog::vocabulary_terms(saved);
        ui::panel(t)
            .child(ui::panel_header(
                Some(ui::icon_tile("book-a", Hue::Neutral, 24., t)),
                "Words to spell exactly",
                Some("Names, brands and terms to get right in captions, one per line.".into()),
                (terms > 0).then(|| {
                    ui::badge(format!("{terms} term{}", if terms == 1 { "" } else { "s" }), Hue::Neutral, t).into_any_element()
                }),
                t,
            ))
            .child(
                div()
                    .mt(u(16.))
                    .child(Textarea::new(&self.vocabulary).text_body())
                    .child(div().mt(u(8.)).text_caption().text_color(t.ink_secondary).child(
                        "Up to five words each. Applies to videos you add from now on.",
                    )),
            )
            .into_any_element()
    }
}

impl SettingsPage {
    fn output_panel(&self, s: &clipward_core::settings::PublicSettings, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let t = *t;
        let usage_text = match &self.usage {
            Some(usage) => format!(
                "{} for {} run{}",
                clipward_core::format::format_bytes(usage.bytes),
                usage.runs,
                if usage.runs == 1 { "" } else { "s" }
            ),
            None => "Measuring…".into(),
        };
        let no_sources = self.usage.as_ref().is_none_or(|usage| usage.runs == 0);
        ui::panel(&t)
            .child(ui::panel_header(
                Some(ui::icon_tile("folder-open", Hue::Neutral, 24., &t)),
                "Where clips are saved",
                Some("Each video gets its own folder inside this one.".into()),
                None,
                &t,
            ))
            .child(
                div()
                    .mt(u(16.))
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .child(
                        div().flex_1().min_w_0().id("output-dir").tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Use Change to select a folder").build(window, cx)
                        }).child(
                            Input::new(&self.output)
                                .with_size(Size::Small)
                                .readonly(true)
                                .font_family(crate::MONO_FONT)
                                .text_caption()
                                .prefix(ui::icon("folder-open").size(u(14.)).text_color(t.ink_secondary)),
                        ),
                    )
                    .child(Btn::new("change-output").label("Change…").on_click(cx.listener(|this, _, _, cx| this.choose_output(cx))))
                    .child(
                        Btn::new("open-output")
                            .ghost()
                            .label("Open")
                            .disabled(s.output_directory.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.open_output(cx))),
                    ),
            )
            .when_some(self.output_error.clone(), |d, message| d.child(error_text(message, &t).mt(u(8.))))
            .child(
                div()
                    .mt(u(12.))
                    .flex()
                    .flex_col()
                    .gap(u(8.))
                    .child(ui::setting_row(
                        "Keep original videos",
                        Some("Needed to edit clips later. Your own files are never moved or deleted.".into()),
                        div().w(u(224.)).flex_shrink_0().child(
                            Select::new(&self.retention)
                                .with_size(Size::Small)
                                .accessibility_label("Keep original videos")
                                .text_caption()
                                .disabled(self.sources_busy),
                        ),
                        false,
                        &t,
                    ))
                    .child(ui::setting_row(
                        "Space used by originals",
                        Some(usage_text.into()),
                        Btn::new("delete-sources")
                            .small()
                            .label("Delete now")
                            .disabled(self.sources_busy || no_sources)
                            .on_click(cx.listener(|this, _, _, cx| this.delete_sources(cx))),
                        false,
                        &t,
                    ))
                    .when_some(self.sources_error.clone(), |d, message| d.child(error_text(message, &t))),
            )
            .into_any_element()
    }

    fn system_panel(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let t = *t;
        let settings = self.settings.read(cx);
        let checking = settings.checking_tools;
        let rows = tool_rows(settings.tool_status.as_ref());
        let checked = settings.tool_status.is_some();
        let missing = rows.iter().filter(|r| !r.optional && r.ok == Some(false)).count();
        let found = rows.iter().filter(|r| r.ok == Some(true)).count();
        let show_rows = !checked || missing > 0 || self.tools_expanded;
        let tool_error = settings.tool_error.clone();
        ui::panel(&t)
            .child(ui::panel_header(
                Some(ui::icon_tile("cpu", if checked && missing > 0 { Hue::Danger } else { Hue::Neutral }, 24., &t)),
                "App tools",
                Some("What Clipward uses to download, transcribe and cut video.".into()),
                Some(
                    refresh_button("recheck", "Check again", checking, false, &t)
                        .when(!checking, |b| {
                            b.on_click(cx.listener(|this, _, _, cx| this.settings.update(cx, |s, cx| s.check_tools(cx))))
                        })
                        .into_any_element(),
                ),
                &t,
            ))
            .child(
                div()
                    .mt(u(16.))
                    .when(checked, |d| {
                        d.child(
                            div()
                                .mb(u(8.))
                                .flex()
                                .items_center()
                                .gap(u(8.))
                                .text_caption()
                                .child(ui::status_dot(if missing > 0 { Tone::Danger } else { Tone::Success }, false, &t))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_color(if missing > 0 { t.danger } else { t.ink_secondary })
                                        .child(if missing > 0 {
                                            format!("{missing} required tool{} missing", if missing == 1 { "" } else { "s" })
                                        } else {
                                            format!("Everything Clipward needs is ready ({found} tools)")
                                        }),
                                )
                                .when(missing == 0, |row| {
                                    row.child(
                                        Btn::new("tool-details")
                                            .ghost()
                                            .small()
                                            .label(if self.tools_expanded { "Hide details" } else { "Show details" })
                                            .trailing_icon(if self.tools_expanded { "chevron-up" } else { "chevron-down" })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.tools_expanded = !this.tools_expanded;
                                                cx.notify();
                                            })),
                                    )
                                }),
                        )
                    })
                    .when(show_rows, |d| {
                        d.child(
                            div()
                                .rounded(u(8.))
                                .border_1()
                                .border_color(t.tint(0.1))
                                .bg(t.tint(0.03))
                                .overflow_hidden()
                                .children(rows.into_iter().enumerate().map(|(i, row)| {
                                    let tone = match row.ok {
                                        None => Tone::Idle,
                                        Some(true) => Tone::Success,
                                        Some(false) if row.optional => Tone::Warning,
                                        Some(false) => Tone::Danger,
                                    };
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(u(12.))
                                        .px(u(12.))
                                        .py(u(8.))
                                        .when(i > 0, |r| r.border_t_1().border_color(t.stroke))
                                        .child(ui::status_dot(tone, false, &t))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .child(div().text_body().text_color(t.ink).child(row.name))
                                                .when_some(row.detail, |d, detail| {
                                                    d.child(
                                                        div()
                                                            .mt(u(2.))
                                                            .font_family(crate::MONO_FONT)
                                                            .text_meta()
                                                            .text_color(t.ink_tertiary)
                                                            .child(detail),
                                                    )
                                                }),
                                        )
                                        .child(match row.ok {
                                            None => div()
                                                .flex_shrink_0()
                                                .text_caption()
                                                .text_color(t.ink_tertiary)
                                                .child(if checking { "Checking…" } else { "Not checked" })
                                                .into_any_element(),
                                            Some(ok) => ui::badge(
                                                if ok { "Found" } else if row.optional { "Unavailable" } else { "Missing" },
                                                if ok { Hue::Neutral } else if row.optional { Hue::Warning } else { Hue::Danger },
                                                &t,
                                            )
                                            .into_any_element(),
                                        })
                                })),
                        )
                    }),
            )
            .when_some(tool_error, |d, message| {
                d.child(div().mt(u(12.)).child(ui::callout(
                    ui::CalloutTone::Danger,
                    None,
                    Some(message.into_any_element()),
                    None,
                    None,
                    &t,
                )))
            })
            .into_any_element()
    }

    /// Updates: check, then download, install and restart in one click.
    fn updates_panel(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let t = *t;
        let updater = Updater::global(cx);
        let state = updater.read(cx).state.clone();
        let installable = Updater::installable();
        // A restart would stop running and queued jobs.
        let jobs_busy = !Jobs::global(cx).read(cx).active().is_empty();
        let check = {
            let updater = updater.clone();
            Btn::new("check-updates")
                .small()
                .icon("refresh-cw")
                .label("Check for updates")
                .loading(matches!(state, UpdateState::Checking))
                .on_click(move |_, _, cx| updater.update(cx, |u, cx| u.check(cx)))
        };
        let caption = |text: String, color: Hsla| div().text_caption().text_color(color).child(text);
        let (line, action): (AnyElement, Option<AnyElement>) = match &state {
            UpdateState::Idle | UpdateState::Checking => (
                caption(format!("You have version {}.", env!("CARGO_PKG_VERSION")), t.ink_secondary).into_any_element(),
                Some(check.into_any_element()),
            ),
            UpdateState::Latest => (
                caption("You have the latest version.".into(), t.ink_secondary).into_any_element(),
                Some(check.into_any_element()),
            ),
            UpdateState::Available(update) | UpdateState::Failed { update: Some(update), .. } => {
                let page = update.page_url.clone();
                let failed = match &state {
                    UpdateState::Failed { message, .. } => Some(message.clone()),
                    _ => None,
                };
                let line = div()
                    .flex()
                    .flex_col()
                    .gap(u(2.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(u(6.))
                            .text_caption()
                            .text_color(t.ink)
                            .child(format!("Version {} is available.", update.version))
                            .child(
                                div()
                                    .id("whats-new")
                                    .text_color(t.accent_fg)
                                    .underline()
                                    .cursor_pointer()
                                    .on_click({
                                        let page = page.clone();
                                        move |_, _, cx| {
                                            let _ = shell::open_url(&page, cx);
                                        }
                                    })
                                    .child("What's new"),
                            ),
                    )
                    .when_some(failed, |d, message| d.child(caption(message, t.danger)))
                    .when(installable && jobs_busy, |d| {
                        d.child(caption("Finish or cancel your jobs first; updating restarts Clipward.".into(), t.ink_secondary))
                    });
                let action = if installable {
                    let updater = updater.clone();
                    Btn::new("install-update")
                        .small()
                        .primary()
                        .icon("download")
                        .label("Update and restart")
                        .disabled(jobs_busy)
                        .on_click(move |_, _, cx| updater.update(cx, |u, cx| u.install(cx)))
                } else {
                    // A development build: get the package by hand.
                    Btn::new("install-update").small().primary().icon("download").label("Download").on_click(
                        move |_, _, cx| {
                            let _ = shell::open_url(&page, cx);
                        },
                    )
                };
                (line.into_any_element(), Some(action.into_any_element()))
            }
            UpdateState::Downloading { update, received, total } => {
                let percent = if *total > 0 { *received as f64 / *total as f64 * 100.0 } else { 0.0 };
                (
                    div()
                        .flex()
                        .flex_col()
                        .gap(u(6.))
                        .child(caption(
                            format!(
                                "Downloading version {}… {} of {}",
                                update.version,
                                clipward_core::format::format_bytes(*received),
                                clipward_core::format::format_bytes(*total)
                            ),
                            t.ink_secondary,
                        ))
                        .child(ui::progress_bar(percent, 4., &t))
                        .into_any_element(),
                    None,
                )
            }
            UpdateState::Installing(update) => (
                caption(format!("Installed version {}. Restarting…", update.version), t.ink_secondary).into_any_element(),
                None,
            ),
            UpdateState::Failed { message, update: None } => {
                (caption(message.clone(), t.danger).into_any_element(), Some(check.into_any_element()))
            }
        };
        ui::panel(&t)
            .child(ui::panel_header(
                Some(ui::icon_tile("download", Hue::Neutral, 24., &t)),
                "Updates",
                Some("New versions install in one click. Your clips and settings stay.".into()),
                None,
                &t,
            ))
            .child(
                div()
                    .mt(u(12.))
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .child(div().flex_1().min_w_0().child(line))
                    .children(action),
            )
            .into_any_element()
    }

    fn about_panel(&self, t: &Tokens) -> AnyElement {
        let t = *t;
        let link = |url: &'static str| move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            let _ = shell::open_url(url, cx);
        };
        ui::panel(&t)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(u(16.))
                    .child(img(logo()).size(u(40.)).flex_shrink_0())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(u(10.))
                                    .child(div().text_heading().text_color(t.ink).child(APP_NAME))
                                    .child(
                                        ui::badge(concat!("v", env!("CARGO_PKG_VERSION")), Hue::Neutral, &t)
                                            .font_family(crate::MONO_FONT),
                                    ),
                            )
                            .child(
                                div()
                                    .mt(u(4.))
                                    .flex()
                                    .flex_wrap()
                                    .text_caption()
                                    .text_color(t.ink_secondary)
                                    .child(format!("Released under the {LICENSE_NAME} license.\u{a0}"))
                                    .child(
                                        div()
                                            .id("clipward-link")
                                            .text_color(t.accent_fg)
                                            .underline()
                                            .cursor_pointer()
                                            .on_click(link(CLIPWARD_URL))
                                            .child("clipward.ai"),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(u(6.))
                            .child(Btn::new("github").small().icon("github").label("GitHub").on_click(link(REPO_URL)))
                            .child(
                                Btn::new("report-issue")
                                    .small()
                                    .ghost()
                                    .label("Report an issue")
                                    .trailing_icon("arrow-up-right")
                                    .on_click(link(ISSUES_URL)),
                            )
                            .child(
                                Btn::new("show-logs")
                                    .small()
                                    .ghost()
                                    .icon("scroll-text")
                                    .label("Show logs")
                                    .on_click(|_, _, cx| {
                                        shell::show_log_file(cx);
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let s = self.saved(cx);
        let scale = Self::rem_scale(window);
        let show_nav = f32::from(window.viewport_size().width) / scale >= NAV_MIN_WIDTH;
        let columns = tile_columns(window);
        let nav_offset = self.nav_offset(window);

        let content = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(u(16.))
            .on_children_prepainted(self.spy_sections(cx))
            .child(self.readiness(columns, &t, cx))
            // One child per entry in SECTIONS after the readiness panel: the scroll spy counts them.
            .child(
                ui::panel(&t)
                    .child(ui::panel_header(
                        Some(ui::icon_tile("sparkles", Hue::Accent, 24., &t)),
                        "AI that picks your clips",
                        Some("It reads what's said in the video and picks the best moments. Use an AI app you're signed in to, or add a key.".into()),
                        None,
                        &t,
                    ))
                    .child(div().mt(u(16.)).child(self.provider.clone())),
            )
            .child(
                section()
                    .child(
                        ui::panel(&t)
                            .child(ui::panel_header(
                                Some(ui::icon_tile("audio-lines", Hue::Neutral, 24., &t)),
                                "Speech to text",
                                Some("Turns what's said into text, right on this computer; audio never leaves it. Bigger models are more accurate but slower.".into()),
                                None,
                                &t,
                            ))
                            .child(div().mt(u(16.)).child(self.whisper.clone())),
                    )
                    .child(self.vocabulary_panel(&s.custom_vocabulary, &t)),
            )
            .child(self.output_panel(&s, &t, cx))
            .child(self.appearance_panel(window, &t, cx))
            .child(
                section()
                    .child(self.updates_panel(&t, cx))
                    .child(self.system_panel(&t, cx))
                    .child(self.about_panel(&t)),
            );

        div()
            .id("settings-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                this.spy.borrow_mut().pinned = false;
                this.scroll_animation = None;
                cx.notify();
            }))
            .child(
                ui::page(1080.)
                    .child(ui::page_header(
                        "Settings",
                        Some("Changes save automatically.".into()),
                        Some(self.save_indicator(&t)),
                        &t,
                    ))
                    .child(
                        div()
                            .mt(u(20.))
                            .flex()
                            .items_start()
                            .gap(u(24.))
                            .when(show_nav, |row| {
                                row.on_children_prepainted(self.spy_row(cx)).child(self.nav(nav_offset, &t, cx))
                            })
                            .child(content),
                    ),
            )
    }
}

/// Panels that share one entry in the section nav.
fn section() -> Div {
    div().flex().flex_col().gap(u(16.))
}

/// A Badge with a leading icon.
fn icon_badge(icon: AnyElement, text: &'static str, hue: Hue, t: &Tokens) -> AnyElement {
    let (fg, bg, border) = match hue {
        Hue::Success => (t.success, t.success.opacity(0.1), t.success.opacity(0.25)),
        _ => (t.ink_secondary, t.tint(0.05), t.tint(0.1)),
    };
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(u(4.))
        .min_h(u(18.))
        .px(u(6.))
        .rounded(u(4.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_meta()
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(icon)
        .child(text)
        .into_any_element()
}

struct ToolRow {
    name: &'static str,
    ok: Option<bool>,
    detail: Option<String>,
    optional: bool,
}

fn tool_rows(status: Option<&ToolStatus>) -> Vec<ToolRow> {
    let ok = |f: fn(&ToolStatus) -> bool| status.map(f);
    vec![
        ToolRow {
            name: "Smart framing",
            ok: ok(|s| s.smart_framing),
            detail: Some(match status {
                Some(s) if !s.smart_framing => s.smart_framing_error.clone().unwrap_or_default(),
                _ => "Face detection model for vertical framing".into(),
            })
            .filter(|d| !d.is_empty()),
            optional: false,
        },
        ToolRow { name: "FFmpeg", ok: ok(|s| s.ffmpeg), detail: None, optional: false },
        ToolRow {
            name: "FFmpeg captions",
            ok: ok(|s| s.ffmpeg_captions),
            detail: Some("Required when captions are enabled; FFmpeg must include the ass filter".into()),
            optional: true,
        },
        ToolRow { name: "FFprobe", ok: ok(|s| s.ffprobe), detail: None, optional: false },
        ToolRow {
            name: "yt-dlp",
            ok: ok(|s| s.ytdlp),
            detail: Some("Downloads from YouTube, Twitch, Kick, X and other sites".into()),
            optional: false,
        },
        ToolRow {
            name: "Clipward clipping engine",
            ok: ok(|s| s.engine),
            detail: status.map(|s| s.engine_path.clone()).filter(|p| !p.is_empty()),
            optional: false,
        },
    ]
}

/// The readiness "Tools" tile: whether every required tool is there (None until
/// checked), and its line of detail.
fn tools_summary(status: Option<&ToolStatus>, checking: bool) -> (Option<bool>, String) {
    let rows = tool_rows(status);
    if status.is_none() {
        return (None, if checking { "Checking…" } else { "Not checked" }.into());
    }
    let missing = rows.iter().filter(|r| !r.optional && r.ok == Some(false)).count();
    if missing > 0 {
        (Some(false), format!("{missing} missing"))
    } else {
        (Some(true), "All installed".into())
    }
}

/// The app icon: the PNG inside resources/clipward-icon-small.svg (the SVG
/// renderer doesn't draw embedded images), loaded once.
fn logo() -> Arc<Image> {
    static LOGO: std::sync::OnceLock<Arc<Image>> = std::sync::OnceLock::new();
    LOGO.get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Png, include_bytes!("clipward-icon.png").to_vec())))
        .clone()
}
