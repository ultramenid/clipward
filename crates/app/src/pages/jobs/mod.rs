//! Clipping jobs (JobsPage.tsx): what is running or queued right now (live, from
//! the job queue) and every earlier run in the output folder. Opening a job shows
//! its progress, its clips, or what went wrong (views.rs).
//!
//! Redraws stay local (measured.rs): the Previous list (previous.rs), the
//! progress ring and stepper (progress.rs) and the arcade are cached entities, so
//! the page's own render, which the active rows' one-second timers and the
//! arcade's frames trigger, only rebuilds the small parts around them.
mod arcade;
mod measured;
mod previous;
mod progress;
mod views;

use crate::models::Jobs;
use crate::state;
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, Btn, CalloutTone, Truncate, TypeScale, u};
use crate::workspace::{self, Page};
use arcade::Arcade;
#[cfg(feature = "page-library")]
use crate::pages::library::clip_list::ClipList;
use clipward_core::file_manager::JobHistoryEntry;
use clipward_core::format::{format_timecode, source_label};
use clipward_core::job_view::{JobOutput, parse_job_output};
use clipward_core::jobs::JobSnapshot;
use clipward_core::jobs_page as logic;
use clipward_core::shared::{MAX_PARALLEL_JOBS, is_active_job_status};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use previous::{PreviousData, PreviousPanel};
use progress::{ProgressCore, Stepper};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// A finished run opened from the Previous list: its folder and saved results.
struct OpenRun {
    output_dir: String,
    output: JobOutput,
}

/// The live views of the job whose progress is on show.
struct Progress {
    job_id: String,
    core: Entity<ProgressCore>,
    stepper: Entity<Stepper>,
    arcade: Entity<Arcade>,
}

pub struct JobsPage {
    jobs: Entity<Jobs>,
    /// None until the first history read.
    entries: Option<Rc<Vec<JobHistoryEntry>>>,
    error: Option<SharedString>,
    refreshing: bool,
    /// The refresh icon turns whole turns: it stops at this moment.
    spin_until: Option<Instant>,
    /// Only the newest history read lands.
    request_id: u64,
    deleting: HashSet<String>,
    active_count: usize,
    scroll: ScrollHandle,
    previous: Entity<PreviousPanel>,
    progress: Option<Progress>,
    open_run: Option<OpenRun>,
    /// The clip list on show, keyed by its run folder.
    #[cfg(feature = "page-library")]
    clips: Option<(String, Entity<ClipList>, Subscription)>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

fn message(error: String, fallback: &str) -> SharedString {
    if error.trim().is_empty() { fallback.into() } else { error.into() }
}

/// The window width in CSS pixels, for the layout's breakpoints.
fn css_width(window: &Window) -> f32 {
    f32::from(window.viewport_size().width) * 16.0 / f32::from(window.rem_size())
}

fn tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |window, cx| Tooltip::new(text.clone()).build(window, cx)
}

/// Rows separated by hairlines (`divide-y divide-stroke`).
fn divided(rows: Vec<AnyElement>, t: &Tokens) -> Div {
    div().flex().flex_col().children(
        rows.into_iter()
            .enumerate()
            .map(|(i, row)| div().when(i > 0, |d| d.border_t_1().border_color(t.stroke)).child(row)),
    )
}

/// A panel's eyebrow and count (SectionTitle).
fn section_title(label: &'static str, count: usize, t: &Tokens) -> Div {
    div()
        .flex()
        .items_center()
        .gap(u(8.))
        .child(div().text_caption().font_weight(FontWeight::SEMIBOLD).text_color(t.ink_tertiary).child(label))
        .child(div().text_meta().text_color(t.ink_tertiary).child(count.to_string()))
}

/// The trash button on a row (DeleteButton).
fn delete_button(page: &WeakEntity<JobsPage>, id: &str, title: String, active: bool, deleting: bool) -> AnyElement {
    let page = page.clone();
    let job_id = id.to_string();
    DangerButton {
        id: SharedString::from(format!("delete-{id}")),
        icon: "trash",
        tooltip: if deleting { "Deleting…" } else { "Delete job" },
        disabled: false,
        loading: deleting,
        on_click: Box::new(move |_, window, cx| {
            let (job_id, title) = (job_id.clone(), title.clone());
            let _ = page.update(cx, |page, cx| page.request_delete(job_id, title, active, window, cx));
        }),
    }
    .into_any_element()
}

/// Ghost icon button with the danger hover the row actions use (Button with
/// `hover:bg-danger/10 hover:text-danger`). ui::Btn has no hover override.
#[derive(IntoElement)]
struct DangerButton {
    id: SharedString,
    icon: &'static str,
    tooltip: &'static str,
    disabled: bool,
    loading: bool,
    on_click: Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
}

impl RenderOnce for DangerButton {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *cx.tokens();
        let inactive = self.disabled || self.loading;
        let fg = if inactive { t.tint(0.3) } else { t.ink_secondary };
        let (danger, danger_bg) = (t.danger, t.danger.opacity(0.1));
        let tooltip_text = self.tooltip;
        let on_click = self.on_click;
        div()
            .id(self.id)
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(u(24.))
            .rounded(u(6.))
            .text_color(fg)
            .when(!inactive, |b| b.cursor_pointer().hover(move |s| s.bg(danger_bg).text_color(danger)))
            .child(if self.loading {
                ui::spinner(14., fg)
            } else {
                ui::icon(self.icon).size(u(14.)).into_any_element()
            })
            .tooltip(tooltip(tooltip_text))
            .when(!inactive, |b| b.on_click(move |event, window, cx| on_click(event, window, cx)))
    }
}

impl JobsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let jobs = Jobs::global(cx);
        let page = cx.weak_entity();
        let previous = cx.new(|cx| PreviousPanel::new(page, window, cx));
        let subscriptions = vec![cx.observe(&jobs, |this: &mut Self, jobs, cx| {
            // A job finishing moves it from Active to Previous; refresh the list then.
            let count = jobs.read(cx).active().len();
            if count != this.active_count {
                this.active_count = count;
                this.load(false, cx);
            }
            this.sync_previous(cx);
            cx.notify();
        })];
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(10)).await;
                if this.update(cx, |this, cx| this.load(false, cx)).is_err() {
                    break;
                }
            }
        });
        // The active rows' elapsed and waiting timers tick each second, while the list shows them.
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let live = this.update(cx, |this, cx| {
                    let jobs = this.jobs.read(cx);
                    if jobs.focused.is_none() && this.open_run.is_none() && !jobs.active().is_empty() {
                        cx.notify();
                    }
                });
                if live.is_err() {
                    break;
                }
            }
        });
        let active_count = jobs.read(cx).active().len();
        #[cfg(feature = "snapshot")]
        snapshot_jobs(&jobs, cx);
        let mut page = Self {
            jobs,
            entries: None,
            error: None,
            refreshing: false,
            spin_until: None,
            request_id: 0,
            deleting: HashSet::new(),
            active_count,
            scroll: ScrollHandle::new(),
            previous,
            progress: None,
            open_run: None,
            #[cfg(feature = "page-library")]
            clips: None,
            _tasks: vec![poll, clock],
            _subscriptions: subscriptions,
        };
        page.load(false, cx);
        page.sync_previous(cx);
        page
    }

    /// Hand the Previous panel what it shows; it redraws only when that changed.
    fn sync_previous(&mut self, cx: &mut Context<Self>) {
        let jobs = self.jobs.read(cx);
        let data = PreviousData {
            entries: self.entries.clone(),
            live: jobs.active().iter().map(|job| job.id.clone()).collect(),
            session: jobs.jobs.keys().cloned().collect(),
            deleting: self.deleting.clone(),
        };
        self.previous.update(cx, |panel, cx| panel.set(data, cx));
    }

    fn load(&mut self, manual: bool, cx: &mut Context<Self>) {
        self.request_id += 1;
        let request = self.request_id;
        if manual {
            self.refreshing = true;
            self.spin_until = Some(Instant::now());
            cx.notify();
        }
        let core = state::core(cx);
        let read = state::run(cx, async move { core.history_list().await });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| {
                if request != this.request_id {
                    return;
                }
                match result {
                    Ok(entries) => {
                        this.entries = Some(Rc::new(entries));
                        this.error = None;
                    }
                    Err(error) => {
                        this.entries.get_or_insert_with(Default::default);
                        this.error = Some(message(error, "Could not load previous jobs."));
                    }
                }
                if this.refreshing {
                    this.refreshing = false;
                    this.stop_spin(cx);
                }
                this.sync_previous(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Let the refresh icon finish its turn, so it comes to rest upright.
    fn stop_spin(&mut self, cx: &mut Context<Self>) {
        let Some(started) = self.spin_until else { return };
        let turn = 700u128;
        let turns = started.elapsed().as_millis().div_ceil(turn).max(1);
        let until = started + Duration::from_millis((turns * turn) as u64);
        self.spin_until = Some(until);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(until.saturating_duration_since(Instant::now())).await;
            let _ = this.update(cx, |this, cx| {
                if !this.refreshing && this.spin_until == Some(until) {
                    this.spin_until = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn scroll_to_top(&self) {
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    fn focus_job(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.jobs.update(cx, |jobs, cx| jobs.focus(id, cx));
        self.scroll_to_top();
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self.open_run = None;
        self.focus_job(None, cx);
        cx.notify();
    }

    fn cancel(&mut self, id: &str, cx: &mut Context<Self>) {
        if !state::core(cx).job_cancel(id) {
            self.error = Some("This job already finished.".into());
            cx.notify();
        }
    }

    fn remove(&mut self, job_id: String, cx: &mut Context<Self>) {
        self.error = None;
        self.deleting.insert(job_id.clone());
        self.sync_previous(cx);
        cx.notify();
        let core = state::core(cx);
        let id = job_id.clone();
        let delete = state::run(cx, async move { core.job_delete(&id).await });
        cx.spawn(async move |this, cx| {
            let result = delete.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.jobs.update(cx, |jobs, cx| jobs.remove(&job_id, cx));
                        if let Some(entries) = &mut this.entries {
                            Rc::make_mut(entries).retain(|entry| entry.job_id != job_id);
                        }
                    }
                    Err(error) => this.error = Some(message(error, "Could not delete this job.")),
                }
                this.deleting.remove(&job_id);
                this.sync_previous(cx);
                this.load(false, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// ConfirmDialog: Cancel or Delete; Escape and the backdrop cancel.
    fn request_delete(&mut self, job_id: String, title: String, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        let body: SharedString = logic::delete_body(&title, active).into();
        let page = cx.entity().downgrade();
        let width = u(420.).to_pixels(window.rem_size());
        window.open_dialog(cx, move |dialog, _, cx| {
            let t = *cx.tokens();
            let (page, job_id) = (page.clone(), job_id.clone());
            dialog
                .w(width)
                .close_button(false)
                .p_0()
                .gap_0()
                .rounded(u(16.))
                .bg(if t.dark { t.bg } else { t.elevated })
                .border_color(t.tint(0.1))
                .child(
                    div()
                        .p(u(16.))
                        .child(div().text_heading().text_color(t.ink).child("Delete this job?"))
                        .child(div().mt(u(6.)).text_body().text_color(t.ink_secondary).child(body.clone())),
                )
                .footer(
                    div()
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(u(6.))
                        .border_t_1()
                        .border_color(t.stroke)
                        .px(u(16.))
                        .py(u(12.))
                        .child(Btn::new("confirm-cancel").label("Cancel").on_click(|_, window, cx| {
                            window.close_dialog(cx);
                        }))
                        .child(Btn::new("confirm-delete").label("Delete").danger().on_click(
                            move |_, window, cx| {
                                window.close_dialog(cx);
                                let job_id = job_id.clone();
                                let _ = page.update(cx, |page, cx| page.remove(job_id, cx));
                            },
                        )),
                )
        });
    }

    fn run_again(&mut self, job: &JobSnapshot, cx: &mut Context<Self>) {
        self.error = None;
        let core = state::core(cx);
        let config = serde_json::to_value(&job.request);
        let start = state::run(cx, async move {
            let config = config.map_err(|_| String::new())?;
            Ok(core.job_start(config).await)
        });
        cx.spawn(async move |this, cx| {
            let result = start.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(started) => match (started.error, started.job_id) {
                        (Some(error), _) => this.error = Some(error.into()),
                        (None, Some(id)) => this.focus_job(Some(id), cx),
                        (None, None) => {}
                    },
                    Err(error) => this.error = Some(message(error, "Could not start this job again.")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_entry(&mut self, job_id: String, output_dir: String, cx: &mut Context<Self>) {
        // This session's jobs open their live view; older runs load from disk.
        if self.jobs.read(cx).jobs.contains_key(&job_id) {
            self.focus_job(Some(job_id), cx);
            return;
        }
        self.error = None;
        let core = state::core(cx);
        let dir = output_dir.clone();
        let read = state::run(cx, async move { core.history_get_job(&dir) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(value) => match value.as_ref().and_then(parse_job_output) {
                        Some(output) => {
                            this.open_run = Some(OpenRun { output_dir, output });
                            this.scroll_to_top();
                        }
                        None => this.error = Some("This run’s saved clips are unavailable.".into()),
                    },
                    Err(error) => this.error = Some(message(error, "Could not open this run.")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_folder(&mut self, dir: &str, cx: &mut Context<Self>) {
        match crate::shell::open_path(dir, cx) {
            Ok(true) => {}
            Ok(false) => self.error = Some("This run folder is unavailable.".into()),
            Err(error) => self.error = Some(message(error, "Could not open this run folder.")),
        }
        cx.notify();
    }

    fn error_callout(&self, t: &Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let error = self.error.clone()?;
        let dismiss = cx.listener(|this, _, _, cx| {
            this.error = None;
            cx.notify();
        });
        Some(
            ui::callout(CalloutTone::Danger, None, Some(error.into_any_element()), None, Some(Box::new(dismiss)), t)
                .into_any_element(),
        )
    }

    // ---- the list -------------------------------------------------------------

    fn active_row(&self, job: &JobSnapshot, position: Option<usize>, wide: f32, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let queued = job.status == "queued";
        let elapsed = logic::elapsed_ms(logic::timer_start(job), logic::now_ms());
        let stage = if queued { "Queued" } else { logic::stage_label(&job.status) };
        let detail = if queued { "Starts when a slot frees up" } else { job.step.as_str() };
        let title = source_label(&job.request.video_url);
        let id = job.id.clone();
        let deleting = self.deleting.contains(&job.id);
        let indicator = if queued {
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .size(u(32.))
                .rounded_full()
                .border_1()
                .border_color(t.tint(0.15))
                .text_meta()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.ink_secondary)
                .child(format!("#{}", position.unwrap_or(1)))
        } else {
            progress::ring(job.percent, 32., 3., t, |d| {
                d.child(
                    div()
                        .text_micro()
                        .line_height(u(10.))
                        .text_color(t.accent_fg)
                        .child(format!("{}%", job.percent.round())),
                )
            })
        };
        let page = cx.weak_entity();
        let cancel = {
            let (page, id) = (page.clone(), id.clone());
            DangerButton {
                id: SharedString::from(format!("cancel-{id}")),
                icon: if queued { "x" } else { "ban" },
                tooltip: if queued { "Remove from queue" } else { "Cancel job" },
                disabled: deleting,
                loading: false,
                on_click: Box::new(move |_, _, cx| {
                    let _ = page.update(cx, |page, cx| page.cancel(&id, cx));
                }),
            }
        };
        div()
            .flex()
            .items_center()
            .pr(u(8.))
            .hover(|row| row.bg(t.tint(0.04)))
            .child(
                div()
                    .id(SharedString::from(format!("open-{id}")))
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(u(12.))
                    .py(u(8.))
                    .pl(u(16.))
                    .pr(u(12.))
                    .cursor_pointer()
                    .tooltip(tooltip(job.request.video_url.clone()))
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| this.focus_job(Some(id.clone()), cx)
                    }))
                    .child(indicator)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_label().text_color(t.ink).one_line().child(title.clone()))
                            .child(
                                div()
                                    .flex()
                                    .text_caption()
                                    .text_color(t.ink_secondary)
                                    .one_line()
                                    .child(div().flex_shrink_0().text_color(if queued { t.ink_tertiary } else { t.accent_fg }).child(stage))
                                    .when(!detail.is_empty() && detail != stage, |d| {
                                        d.child(div().min_w_0().one_line().child(format!(" · {detail}")))
                                    }),
                            ),
                    )
                    .when(job.clips_total > 0 && wide >= 640., |d| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .whitespace_nowrap()
                                .text_caption()
                                .text_color(t.ink_secondary)
                                .child(format!("{}/{} clips", job.clips_done, job.clips_total)),
                        )
                    })
                    .child(
                        div()
                            .w(u(112.))
                            .flex_shrink_0()
                            .flex()
                            .justify_end()
                            .gap(u(4.))
                            .whitespace_nowrap()
                            .text_caption()
                            .text_color(t.ink_secondary)
                            .child(div().text_color(t.ink.opacity(0.8)).child(format_timecode(elapsed)))
                            .child(if queued { "waiting" } else { "elapsed" }),
                    ),
            )
            .child(cancel)
            .child(delete_button(&page, &id, title, true, deleting))
            .into_any_element()
    }

    fn render_list(&mut self, window: &mut Window, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let wide = css_width(window);
        let active: Vec<JobSnapshot> = self.jobs.read(cx).active().into_iter().cloned().collect();
        let refs: Vec<&JobSnapshot> = active.iter().collect();
        let positions: Vec<Option<usize>> = active
            .iter()
            .map(|job| (job.status == "queued").then(|| logic::queue_position(&refs, &job.id)).flatten())
            .collect();
        let running = active.iter().filter(|job| job.status != "queued").count();
        let queued = active.len() - running;
        let spinning = self.refreshing || self.spin_until.is_some_and(|until| Instant::now() < until);

        let disabled = self.refreshing;
        let refresh_icon = ui::icon("refresh-cw").size(u(14.)).text_color(if disabled { t.tint(0.3) } else { t.ink_secondary });
        let refresh = div()
            .id("refresh")
            .flex()
            .items_center()
            .justify_center()
            .size(u(28.))
            .rounded(u(6.))
            .when(!disabled, |b| {
                b.cursor_pointer()
                    .hover(|b| b.bg(t.tint(0.1)))
                    .on_click(cx.listener(|this, _, _, cx| this.load(true, cx)))
            })
            .tooltip(tooltip("Refresh jobs"))
            .child(if spinning {
                refresh_icon
                    .with_animation(
                        "refresh-spin",
                        Animation::new(Duration::from_millis(700)).repeat(),
                        |icon, delta| icon.rotate(percentage(delta)),
                    )
                    .into_any_element()
            } else {
                refresh_icon.into_any_element()
            });
        let actions = div()
            .flex()
            .items_center()
            .gap(u(6.))
            .child(refresh)
            .child(
                Btn::new("new-job")
                    .primary()
                    .icon("plus")
                    .label("New job")
                    .on_click(|_, window, cx| workspace::go(Page::Create, window, cx)),
            )
            .into_any_element();

        let active_panel = (!active.is_empty()).then(|| {
            let rows: Vec<AnyElement> = active
                .iter()
                .zip(&positions)
                .map(|(job, position)| self.active_row(job, *position, wide, t, cx))
                .collect();
            ui::card(t)
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap(u(8.))
                        .border_b_1()
                        .border_color(t.stroke)
                        .py(u(8.))
                        .pl(u(16.))
                        .pr(u(12.))
                        .child(section_title("Active", active.len(), t))
                        .child(
                            div()
                                .text_meta()
                                .text_color(t.ink_secondary)
                                .child(logic::active_summary(running, queued, MAX_PARALLEL_JOBS)),
                        ),
                )
                .child(divided(rows, t))
        });
        let measured = self.previous.read(cx).measured.clone();

        ui::page(ui::PAGE_DEFAULT)
            .child(ui::page_header("Jobs", None, Some(actions), t).items_center())
            .child(
                div()
                    .mt(u(16.))
                    .flex()
                    .flex_col()
                    .gap(u(12.))
                    .children(self.error_callout(t, cx))
                    .children(active_panel)
                    .child(measured.view(&self.previous)),
            )
            .into_any_element()
    }
}

impl Render for JobsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let focused = {
            let jobs = self.jobs.read(cx);
            jobs.focused.as_ref().and_then(|id| jobs.jobs.get(id)).cloned()
        };
        if !focused.as_ref().is_some_and(|job| is_active_job_status(&job.status)) {
            self.progress = None;
        }
        // A completed job, or a finished run from the list: its clips (they scroll on their own).
        let clips = match &focused {
            Some(job) if job.status == "completed" => job
                .output
                .as_ref()
                .and_then(parse_job_output)
                .map(|output| (job.output_dir.clone(), output)),
            Some(_) => None,
            None => self.open_run.as_ref().map(|run| (run.output_dir.clone(), run.output.clone())),
        };
        if let Some((output_dir, output)) = clips {
            return div().size_full().child(self.render_clips(output_dir, output, window, &t, cx));
        }
        #[cfg(feature = "page-library")]
        {
            self.clips = None;
        }
        let content = match focused {
            Some(job) => self.render_focused(job, window, &t, cx),
            None => self.render_list(window, &t, cx),
        };
        div().size_full().child(
            div()
                .id("page-scroll")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .child(content),
        )
    }
}

/// Development only: `CLIPWARD_SNAPSHOT_JOB=running|queued|failed|cancelled|list`
/// puts made-up jobs in the session's list (in memory; the queue never sees
/// them), so snapshots can show each state.
#[cfg(feature = "snapshot")]
fn snapshot_jobs(jobs: &Entity<Jobs>, cx: &mut Context<JobsPage>) {
    let Ok(kind) = std::env::var("CLIPWARD_SNAPSHOT_JOB") else { return };
    let now = chrono_like_now();
    let request = |url: &str, speed: f64| {
        serde_json::from_value(serde_json::json!({
            "videoUrl": url, "autoClipCount": true, "aspectRatio": "9:16",
            "layoutStyle": "smart", "layoutVision": false, "pacing": "natural",
            "includeCaptions": true, "captionPreset": "pop", "videoSpeed": speed
        }))
        .expect("a minimal request")
    };
    let job = |id: &str, status: &str, percent: f64, url: &str| JobSnapshot {
        id: id.into(),
        revision: 1,
        request: request(url, 1.5),
        status: status.into(),
        percent,
        step: "Rendering clip 3 of 6".into(),
        clips_done: 2,
        clips_total: 6,
        error: (status == "failed").then(|| "ffmpeg exited with status 1: Invalid data found when processing input".into()),
        error_hint: (status == "failed").then(|| "The download may be incomplete. Try the video again, or pick a local file.".into()),
        failure_code: (status == "failed").then(|| "render_failed".into()),
        failure_stage: None,
        http_status: None,
        output: None,
        output_dir: format!("/tmp/clipward-snapshot/{id}"),
        queued_at: now.clone(),
        started_at: (status != "queued").then(|| now.clone()),
        finished_at: None,
    };
    let list = vec![
        job("snap-run", "rendering", 62.0, "https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
        job("snap-dl", "downloading", 8.0, "/Users/me/Movies/Keynote 2026.mp4"),
        job("snap-queued", "queued", 0.0, "/Users/me/Movies/Interview.mov"),
        job("snap-failed", "failed", 40.0, "/Users/me/Movies/Broken.mp4"),
        job("snap-cancelled", "cancelled", 20.0, "/Users/me/Movies/Stopped.mp4"),
    ];
    let focus = match kind.as_str() {
        "running" => Some("snap-run"),
        "queued" => Some("snap-queued"),
        "failed" => Some("snap-failed"),
        "cancelled" => Some("snap-cancelled"),
        _ => None,
    };
    jobs.update(cx, |jobs, cx| {
        for job in list {
            jobs.upsert(job, cx);
        }
        jobs.focus(focus.map(String::from), cx);
    });
}

#[cfg(feature = "snapshot")]
fn chrono_like_now() -> String {
    // Two minutes ago, so the timers read 02:00-ish.
    let at = std::time::SystemTime::now() - Duration::from_secs(125);
    let secs = at.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since the epoch (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z", rem / 3600, rem % 3600 / 60, rem % 60)
}
