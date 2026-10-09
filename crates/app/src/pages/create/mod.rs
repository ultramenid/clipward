//! Create: the setup card (until the AI provider, or for manual clips just Whisper,
//! is ready) and the clipping wizard. Runs go to the core's queue and are followed
//! on the Jobs page, so this page is always ready for the next video. Ports
//! ClipPage.tsx, JobForm.tsx and use-draft-store.ts; the step bodies are in
//! steps.rs, the source picker in source.rs, manual clips in manual.rs, caption
//! looks in captions.rs and the setup card in setup.rs.
mod captions;
mod manual;
mod setup;
mod source;
mod steps;

use crate::models::{self, Jobs, Settings};
use crate::state;
use crate::theme::ActiveTokens;
use crate::ui::{self, u};
use crate::workspace::{self, Page};
use clipward_core::job_form::{self, ClipDraft, ClipMode, FormCheck, StartedJob, WizardStep};
use gpui_kit::component::input::{Enter, InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;

actions!(create, [Generate]);

const CONTEXT: &str = "CreatePage";

/// The Create form, kept app-wide so the chosen video and options survive
/// navigating away (to Settings to add a key) and a failed run.
pub struct Draft {
    pub value: ClipDraft,
}

struct GlobalDraft(Entity<Draft>);

impl Global for GlobalDraft {}

impl Draft {
    /// The app's draft, created on first use.
    pub fn global(cx: &mut App) -> Entity<Draft> {
        if let Some(global) = cx.try_global::<GlobalDraft>() {
            return global.0.clone();
        }
        // ⌘↵ / Ctrl+↵ generates from any step.
        cx.bind_keys([KeyBinding::new("secondary-enter", Generate, Some(CONTEXT))]);
        crate::captions::load_fonts(cx);
        let draft = cx.new(|_| Draft {
            value: ClipDraft::default(),
        });
        cx.set_global(GlobalDraft(draft.clone()));
        draft
    }
}

/// Change the draft from a click or toggle handler.
fn edit(draft: &Entity<Draft>, cx: &mut App, change: impl FnOnce(&mut ClipDraft)) {
    draft.update(cx, |draft, cx| {
        change(&mut draft.value);
        cx.notify();
    });
}

/// The wizard's text fields, kept in step with the draft.
struct Inputs {
    link: Entity<InputState>,
    trim_start: Entity<InputState>,
    trim_end: Entity<InputState>,
    prompt: Entity<TextareaState>,
    max_clips: Entity<InputState>,
    banner: Entity<InputState>,
}

/// Arrow-key focus for the radio groups (Format, Framing, Speed, Caption style).
struct RadioFocus {
    format: FocusHandle,
    framing: FocusHandle,
    speed: FocusHandle,
    caption: FocusHandle,
}

pub struct CreatePage {
    draft: Entity<Draft>,
    settings: Entity<Settings>,
    jobs: Entity<Jobs>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// A start is in flight; a second Generate is ignored until it returns.
    starting: bool,
    start_error: Option<String>,
    inputs: Inputs,
    radios: RadioFocus,
    source: source::SourceState,
    manual: manual::ManualState,
    logo_error: Option<String>,
    /// Width of the caption picker's grid, measured after layout (auto-fill columns).
    caption_width: Rc<Cell<f32>>,
    setup: setup::SetupCard,
    _subscriptions: Vec<Subscription>,
}

impl CreatePage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft = Draft::global(cx);
        let settings = Settings::global(cx);
        let jobs = Jobs::global(cx);
        let value = draft.read(cx).value.clone();
        let input = |placeholder: &str, text: &str, window: &mut Window, cx: &mut Context<Self>| {
            let placeholder = SharedString::from(placeholder.to_string());
            let text = SharedString::from(text.to_string());
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(text)
            })
        };
        let inputs = Inputs {
            link: input(
                "Paste a YouTube, Twitch or other video link",
                "",
                window,
                cx,
            ),
            trim_start: input("Start 0:00", &value.trim_start, window, cx),
            trim_end: input("End", &value.trim_end, window, cx),
            prompt: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(8)
                    .placeholder(steps::PROMPT_EXAMPLE)
                    .default_value(value.requirements.clone())
            }),
            max_clips: input("", &value.max_clips.to_string(), window, cx),
            banner: input("youtube.com/@yourchannel", &value.banner_url, window, cx),
        };
        let focus_handle = |cx: &mut Context<Self>| cx.focus_handle().tab_stop(true);
        let radios = RadioFocus {
            format: focus_handle(cx),
            framing: focus_handle(cx),
            speed: focus_handle(cx),
            caption: focus_handle(cx),
        };
        let subscriptions = vec![
            cx.observe(&draft, |_, _, cx| cx.notify()),
            cx.observe(&settings, |_, _, cx| cx.notify()),
            cx.observe(&jobs, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &inputs.link,
                window,
                |this, state, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.source.link_error = None;
                        cx.notify();
                    }
                    InputEvent::PressEnter {
                        secondary: false, ..
                    } => {
                        let text = state.read(cx).value().to_string();
                        this.submit_link(&text, window, cx);
                    }
                    _ => {}
                },
            ),
            cx.subscribe(&inputs.trim_start, |this, state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = state.read(cx).value().to_string();
                    edit(&this.draft, cx, |d| d.trim_start = text);
                }
            }),
            cx.subscribe(&inputs.trim_end, |this, state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = state.read(cx).value().to_string();
                    edit(&this.draft, cx, |d| d.trim_end = text);
                }
            }),
            cx.subscribe(&inputs.prompt, |this, state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = state.read(cx).value().to_string();
                    edit(&this.draft, cx, |d| d.requirements = text);
                }
            }),
            cx.subscribe_in(
                &inputs.max_clips,
                window,
                |this, state, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        let typed = state.read(cx).value().to_string();
                        let count = job_form::parse_max_clips(&typed);
                        edit(&this.draft, cx, |d| d.max_clips = count);
                        // A controlled field: it always shows the count it holds.
                        if typed != count.to_string() {
                            state.update(cx, |state, cx| {
                                state.set_value(count.to_string(), window, cx)
                            });
                        }
                    }
                },
            ),
            cx.subscribe(&inputs.banner, |this, state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = state.read(cx).value().to_string();
                    edit(&this.draft, cx, |d| d.banner_url = text);
                }
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            setup: setup::SetupCard::default(),
            draft,
            settings,
            jobs,
            focus,
            scroll: ScrollHandle::new(),
            starting: false,
            start_error: None,
            inputs,
            radios,
            source: source::SourceState::default(),
            manual: manual::ManualState::default(),
            logo_error: None,
            caption_width: Rc::new(Cell::new(0.)),
            _subscriptions: subscriptions,
        }
    }

    fn manual_mode(&self, cx: &App) -> bool {
        self.draft.read(cx).value.clip_mode == ClipMode::Manual
    }

    /// Why a job can't start yet (setup), or None.
    fn blocked_reason(&self, cx: &App) -> Option<&'static str> {
        models::setup_blocked_reason(self.settings.read(cx).setup(), self.manual_mode(cx))
    }

    fn check(&self, cx: &App) -> FormCheck {
        job_form::check_form(
            &self.draft.read(cx).value,
            self.blocked_reason(cx).is_some(),
            self.starting,
        )
    }

    fn go_to(&mut self, step: WizardStep, cx: &mut Context<Self>) {
        edit(&self.draft, cx, |d| d.step = step);
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    /// A click handler that runs `f` on this page.
    fn on_click(
        &self,
        cx: &Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |_, window, cx| {
            let _ = this.update(cx, |this, cx| f(this, window, cx));
        }
    }

    /// Queue the run; ignored while a start is in flight or the form can't submit.
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.starting {
            return;
        }
        let check = self.check(cx);
        if !check.can_submit {
            return;
        }
        let request = job_form::build_job_request(&self.draft.read(cx).value, &check.trim);
        let source = request["videoUrl"].as_str().unwrap_or_default().to_string();
        self.starting = true;
        self.start_error = None;
        cx.notify();
        let core = state::core(cx);
        let start = state::run(cx, async move { Ok(core.job_start(request).await) });
        let draft = self.draft.downgrade();
        cx.spawn(async move |this, cx| {
            let error = match start.await {
                Ok(result) => match (result.error, result.job_id) {
                    (Some(error), _) => Some(error),
                    (None, Some(job_id)) => {
                        let started = StartedJob {
                            job_id,
                            source,
                            queued: result.queued.unwrap_or(false),
                        };
                        let _ = draft.update(cx, |draft, cx| {
                            draft.value.started = Some(started);
                            cx.notify();
                        });
                        None
                    }
                    (None, None) => None,
                },
                Err(_) => Some("Could not start the job. Please try again.".to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                this.starting = false;
                this.start_error = error;
                this.scroll.set_offset(point(px(0.), px(0.)));
                cx.notify();
            });
        })
        .detach();
    }

    /// Back to the first step for the next video, keeping every other choice.
    fn start_another(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        edit(&self.draft, cx, |d| d.start_another());
        for state in [
            &self.inputs.trim_start,
            &self.inputs.trim_end,
            &self.inputs.link,
        ] {
            state.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.manual.reset();
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    fn view_job(&mut self, job_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.jobs
            .update(cx, |jobs, cx| jobs.focus(Some(job_id), cx));
        workspace::go(Page::Jobs, window, cx);
    }
}

impl Render for CreatePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let draft = self.draft.read(cx).value.clone();
        self.sync_preview(&draft.source, cx);
        self.sync_manual_rows(&draft.manual_clips, window, cx);
        let blocked = self.blocked_reason(cx);
        let check = self.check(cx);
        let started = draft.started.clone();

        let form = match &started {
            Some(started) => self.render_started(started, cx).into_any_element(),
            None => div()
                .flex()
                .flex_col()
                .gap(u(12.))
                .child(self.render_stepper(&check, window, cx))
                .child(self.render_step_panel(&draft, &check, window, cx))
                .into_any_element(),
        };
        let page = ui::page(ui::PAGE_NARROW)
            .when(started.is_none(), |page| page.pb(u(12.)))
            .child(ui::page_header(
                "Create clips",
                Some(
                    "Add a long video and tell the AI what you want. Clipward cuts captioned clips that follow it."
                        .into(),
                ),
                None,
                &t,
            ))
            .child(self.setup.render(draft.clip_mode == ClipMode::Manual, &t, window, cx))
            .when_some(self.start_error.clone(), |page, error| {
                let this = cx.entity().downgrade();
                page.child(div().mt(u(12.)).child(ui::callout(
                    ui::CalloutTone::Danger,
                    None,
                    Some(div().child(error).into_any_element()),
                    None,
                    Some(Box::new(move |_, _, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.start_error = None;
                            cx.notify();
                        });
                    })),
                    &t,
                )))
            })
            .child(div().mt(u(16.)).child(form));

        div()
            .id("create-page")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Generate, _, cx| this.submit(cx)))
            // In a text field ⌘↵ arrives as the field's own Enter action.
            .capture_action(cx.listener(|this, enter: &Enter, _, cx| {
                if enter.secondary {
                    cx.stop_propagation();
                    this.submit(cx);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            // The scroll area takes its content's height up to the window, so the
            // footer below sits right under the card or pins to the bottom edge.
            .child(
                div()
                    .id("create-scroll")
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(page),
            )
            .when(started.is_none(), |root| {
                root.child(
                    div().flex_shrink_0().w_full().child(
                        div()
                            .mx_auto()
                            .w_full()
                            .max_w(u(ui::PAGE_NARROW))
                            .px(u(32.))
                            .child(self.render_footer(&check, blocked, cx)),
                    ),
                )
            })
    }
}
