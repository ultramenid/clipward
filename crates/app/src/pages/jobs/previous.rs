//! The Previous panel (JobsList in JobsPage.tsx): every earlier run, with status
//! filters and search, or the empty state before the first job. Its own entity,
//! so the active rows' timers don't redraw a long history every second.
use super::measured::Measured;
use super::{JobsPage, divided, tooltip};
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, Btn, Tone, Truncate, TypeScale, u};
use crate::workspace::{self, Page};
use clipward_core::file_manager::JobHistoryEntry;
use clipward_core::format::{format_date, format_duration, format_relative_date, format_usd};
use clipward_core::jobs_page::{self as logic, FILTERS};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::collections::HashSet;
use std::rc::Rc;

/// What the panel shows from the page's state.
#[derive(Default)]
pub(super) struct PreviousData {
    /// None until the first history read.
    pub entries: Option<Rc<Vec<JobHistoryEntry>>>,
    /// Queued and running jobs: shown above, so left out here.
    pub live: HashSet<String>,
    /// This session's jobs, which open their live view.
    pub session: HashSet<String>,
    pub deleting: HashSet<String>,
}

impl PreviousData {
    fn same(&self, other: &Self) -> bool {
        let entries = match (&self.entries, &other.entries) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b) || logic::same_entries(a, b),
            (None, None) => true,
            _ => false,
        };
        entries && self.live == other.live && self.session == other.session && self.deleting == other.deleting
    }
}

pub(super) struct PreviousPanel {
    page: WeakEntity<JobsPage>,
    data: PreviousData,
    filter: &'static str,
    query: Entity<InputState>,
    pub measured: Measured,
    _subscription: Subscription,
}

/// Previous-run statuses: dot tone and status text colour.
fn status_tone(status: &str, t: &Tokens) -> (Tone, Hsla) {
    match status {
        "completed" => (Tone::Success, t.success),
        "running" => (Tone::Accent, t.accent_fg),
        "failed" => (Tone::Danger, t.danger),
        "cancelled" => (Tone::Idle, t.ink_secondary),
        _ => (Tone::Warning, t.warning),
    }
}

impl PreviousPanel {
    pub fn new(page: WeakEntity<JobsPage>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search jobs"));
        let subscription = cx.subscribe(&query, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            page,
            data: PreviousData::default(),
            filter: "all",
            query,
            measured: Measured::default(),
            _subscription: subscription,
        }
    }

    /// Take the page's latest state; redraws only when something in it changed.
    pub fn set(&mut self, data: PreviousData, cx: &mut Context<Self>) {
        if !self.data.same(&data) {
            self.data = data;
            cx.notify();
        }
    }

    fn row(&self, entry: &JobHistoryEntry, wide: f32, t: &Tokens) -> AnyElement {
        let completed = entry.status == "completed";
        // Failed and cancelled jobs from this session keep their options, so they can run again.
        let openable = completed || self.data.session.contains(&entry.job_id);
        let dated = logic::is_dated(&entry.date);
        let (tone, status_color) = status_tone(&entry.status, t);
        let group: SharedString = format!("row-{}", entry.job_id).into();
        let deleting = self.data.deleting.contains(&entry.job_id);
        let hint = match &entry.error_message {
            Some(error) => format!("{} · {error}", entry.video_title),
            None => entry.video_title.clone(),
        };
        let column = |width: f32, text: String| {
            div()
                .w(u(width))
                .flex_shrink_0()
                .whitespace_nowrap()
                .flex()
                .justify_end()
                .text_caption()
                .text_color(t.ink_secondary)
                .child(text)
        };
        let cells = div()
            .id(SharedString::from(format!("entry-{}", entry.job_id)))
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(u(12.))
            .py(u(8.))
            .pl(u(16.))
            .pr(u(8.))
            .child(ui::status_dot(tone, false, t))
            .child(
                // The title keeps its room; the status stays whole and the error takes what is left.
                div()
                    .id(SharedString::from(format!("title-{}", entry.job_id)))
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_baseline()
                    .gap(u(8.))
                    .tooltip(tooltip(hint))
                    .child(
                        div()
                            .min_w_0()
                            .flex_shrink_1()
                            .text_body()
                            .text_color(t.ink)
                            .one_line()
                            .child(entry.video_title.clone()),
                    )
                    .when(!completed, |d| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .text_caption()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(status_color)
                                .child(logic::status_label(&entry.status)),
                        )
                    })
                    .when_some(entry.error_message.clone().filter(|_| wide >= 1280.), |d, error| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_caption()
                                .text_color(t.ink_secondary)
                                .one_line()
                                .child(format!(" · {error}")),
                        )
                    }),
            )
            .when(wide >= 640., |d| {
                d.child(column(64., if completed { logic::clip_count(entry.clip_count) } else { String::new() }))
            })
            .when(wide >= 768., |d| {
                d.child(column(64., entry.duration_ms.map(|ms| format_duration(ms as f64)).unwrap_or_default()))
                    .child(column(56., entry.total_cost_usd.map(format_usd).unwrap_or_default()))
            })
            .child(
                div()
                    .id(SharedString::from(format!("date-{}", entry.job_id)))
                    .w(u(128.))
                    .flex_shrink_0()
                    .flex()
                    .justify_end()
                    .text_caption()
                    .text_color(t.ink_secondary)
                    .one_line()
                    .when(dated, |d| d.tooltip(tooltip(format_date(&entry.date))))
                    .child(if dated { format_relative_date(&entry.date) } else { "Date unavailable".into() }),
            )
            .when(openable, |cells| {
                let (job_id, dir) = (entry.job_id.clone(), entry.output_dir.clone());
                let page = self.page.clone();
                cells
                    .cursor_pointer()
                    .tooltip(tooltip(if completed { "View clips" } else { "Details" }))
                    .on_click(move |_, _, cx| {
                        let (job_id, dir) = (job_id.clone(), dir.clone());
                        let _ = page.update(cx, |page, cx| page.open_entry(job_id, dir, cx));
                    })
            });
        let (dir, page) = (entry.output_dir.clone(), self.page.clone());
        div()
            .group(group.clone())
            .flex()
            .items_center()
            .pr(u(8.))
            .when(openable, |row| row.hover(|row| row.bg(t.tint(0.04))))
            .child(cells)
            .child(
                div().opacity(0.).group_hover(group.clone(), |d| d.opacity(1.)).child(
                    Btn::new(SharedString::from(format!("folder-{}", entry.job_id)))
                        .ghost()
                        .small()
                        .icon("folder-open")
                        .tooltip("Open folder")
                        .on_click(move |_, _, cx| {
                            let _ = page.update(cx, |page, cx| page.open_folder(&dir, cx));
                        }),
                ),
            )
            .child(
                div()
                    .when(!deleting, |d| d.opacity(0.).group_hover(group, |d| d.opacity(1.)))
                    .child(super::delete_button(&self.page, &entry.job_id, entry.video_title.clone(), false, deleting)),
            )
            .into_any_element()
    }

    fn content(&mut self, window: &mut Window, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let wide = super::css_width(window);
        let live: HashSet<&str> = self.data.live.iter().map(String::as_str).collect();
        let entries: &[JobHistoryEntry] = self.data.entries.as_deref().map_or(&[], |e| e.as_slice());
        let previous = logic::previous(entries, &live);
        if self.data.entries.is_some() && previous.is_empty() && live.is_empty() {
            return ui::empty_state(
                "list-video",
                "No clipping jobs yet",
                Some("Jobs appear here as soon as you generate clips. Queue as many videos as you like; up to two run at once.".into()),
                Some(
                    Btn::new("create-clips")
                        .primary()
                        .label("Create clips")
                        .on_click(|_, window, cx| workspace::go(Page::Create, window, cx))
                        .into_any_element(),
                ),
                t,
            )
            .into_any_element();
        }
        let query = self.query.read(cx).value();
        let visible = logic::visible(&previous, self.filter, &query);
        let chips = FILTERS
            .iter()
            .filter(|(id, _)| *id == "all" || logic::count(&previous, id) > 0)
            .map(|&(id, label)| {
                let selected = self.filter == id;
                div()
                    .id(id)
                    .flex()
                    .h(u(24.))
                    .items_center()
                    .gap(u(6.))
                    .rounded(u(6.))
                    .px(u(8.))
                    .text_caption()
                    .cursor_pointer()
                    .map(|chip| {
                        if selected {
                            chip.bg(t.selection).text_color(t.ink)
                        } else {
                            chip.text_color(t.ink_secondary).hover(|c| c.bg(t.tint(0.05)).text_color(t.ink))
                        }
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = id;
                        cx.notify();
                    }))
                    .child(label)
                    .child(
                        div()
                            .text_meta()
                            .text_color(if selected { t.ink_secondary } else { t.ink_tertiary })
                            .child(logic::count(&previous, id).to_string()),
                    )
            })
            .collect::<Vec<_>>();
        let body = if self.data.entries.is_none() {
            divided(
                (0..4)
                    .map(|_| {
                        div()
                            .flex()
                            .h(u(40.))
                            .items_center()
                            .gap(u(12.))
                            .px(u(16.))
                            .child(div().flex_shrink_0().size(u(8.)).rounded_full().overflow_hidden().child(ui::skeleton(t)))
                            .child(div().h(u(12.)).w(relative(0.4)).child(ui::skeleton(t)))
                            .child(div().ml_auto().h(u(12.)).w(u(96.)).child(ui::skeleton(t)))
                            .into_any_element()
                    })
                    .collect(),
                t,
            )
        } else if previous.is_empty() || visible.is_empty() {
            div()
                .px(u(16.))
                .py(u(12.))
                .text_caption()
                .text_color(t.ink_secondary)
                .child(if previous.is_empty() { "Finished jobs will appear here." } else { "No jobs match this filter." })
        } else {
            let rows = visible.iter().map(|entry| self.row(entry, wide, t)).collect();
            divided(rows, t)
        };
        ui::card(t)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(u(8.))
                    .border_b_1()
                    .border_color(t.stroke)
                    .py(u(6.))
                    .pl(u(16.))
                    .pr(u(6.))
                    .child(super::section_title("Previous", previous.len(), t).mr_auto())
                    .when(!previous.is_empty(), |d| {
                        d.child(div().flex().flex_wrap().items_center().gap(px(2.)).children(chips)).child(
                            div().map(|d| if wide < 640. { d.w_full() } else { d.w(u(192.)) }).child(
                                Input::new(&self.query)
                                    .small()
                                    .h(u(28.))
                                    .text_caption()
                                    .prefix(ui::icon("search").size(u(14.)).text_color(t.ink_secondary)),
                            ),
                        )
                    }),
            )
            .child(body)
            .into_any_element()
    }
}

impl Render for PreviousPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let content = self.content(window, &t, cx);
        self.measured.wrap(&self.page, content)
    }
}
