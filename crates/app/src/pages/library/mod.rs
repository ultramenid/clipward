//! The Library (LibraryPage.tsx): every completed run as a card, newest first,
//! searchable by title. A run opens as a `ClipList` in place of the grid.
//!
//! `clip_list`, `clip_card`, `run_stats` and `thumbnails` are shared with the Jobs
//! page: see `ClipList::new`.
pub mod clip_card;
pub mod clip_list;
pub mod run_stats;
pub mod thumbnails;

use crate::models::Settings;
use crate::shell;
use crate::state;
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, CalloutTone, TypeScale, u};
use crate::workspace::{self, Page};
use clip_card::media_chip;
use clip_list::{ClipList, ClipListBack, auto_grid, error_message};
use clipward_core::file_manager::JobHistoryEntry;
use clipward_core::format::{format_relative_date, format_usd};
use clipward_core::job_view::parse_job_output;
use clipward_core::library::{
    best_clip, clip_file_path, filter_runs, library_summary, thumbnail_seek,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use run_stats::tnum;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use thumbnails::{Thumb, Thumbnails};

/// One turn of the refresh glyph.
const TURN_MS: u64 = 700;

pub struct LibraryPage {
    /// None until the first load lands.
    entries: Option<Rc<Vec<JobHistoryEntry>>>,
    search: Entity<InputState>,
    open: Option<(Entity<ClipList>, Subscription)>,
    error: Option<String>,
    refreshing: bool,
    /// When the refresh glyph started turning; it stops on a whole turn.
    turning_since: Option<Instant>,
    request: u64,
    open_request: u64,
    /// Each run's best clip (path, seek), whose thumbnail fronts its card.
    run_thumbs: HashMap<String, (String, Option<f64>)>,
    thumbnails: Entity<Thumbnails>,
    scroll: ScrollHandle,
    grid_width: Rc<Cell<Pixels>>,
    _subscriptions: Vec<Subscription>,
}

impl LibraryPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search by video title"));
        let thumbnails = Thumbnails::global(cx);
        let subscriptions = vec![
            cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.observe(&thumbnails, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            entries: None,
            search,
            open: None,
            error: None,
            refreshing: false,
            turning_since: None,
            request: 0,
            open_request: 0,
            run_thumbs: HashMap::new(),
            thumbnails,
            scroll: ScrollHandle::new(),
            grid_width: Rc::new(Cell::new(Pixels::ZERO)),
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.request += 1;
        let request = self.request;
        self.refreshing = true;
        self.turning_since.get_or_insert_with(Instant::now);
        self.error = None;
        let core = state::core(cx);
        let job = state::run(cx, async move { core.history_list().await });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if request != this.request {
                    return;
                }
                match result {
                    Ok(entries) => {
                        this.entries = Some(Rc::new(
                            entries
                                .into_iter()
                                .filter(|e| e.status == "completed")
                                .collect(),
                        ));
                        this.find_run_thumbs(cx);
                    }
                    Err(error) => {
                        this.entries.get_or_insert_with(Default::default);
                        this.error = Some(error_message(
                            error,
                            "Could not load the library. Please refresh to retry.",
                        ));
                    }
                }
                this.refreshing = false;
                this.settle_glyph(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Let the refresh glyph finish its turn, then rest.
    fn settle_glyph(&mut self, cx: &mut Context<Self>) {
        let Some(since) = self.turning_since else {
            return;
        };
        let elapsed = since.elapsed().as_millis() as u64;
        let left = TURN_MS - elapsed % TURN_MS;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(left))
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.refreshing {
                    this.turning_since = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Look up each new run's best clip and queue its thumbnail.
    fn find_run_thumbs(&mut self, cx: &mut Context<Self>) {
        let dirs: Vec<String> = self
            .entries
            .iter()
            .flat_map(|entries| entries.iter())
            .map(|entry| entry.output_dir.clone())
            .filter(|dir| !self.run_thumbs.contains_key(dir))
            .collect();
        let core = state::core(cx);
        let job = state::run(cx, async move {
            Ok(dirs
                .into_iter()
                .filter_map(|dir| {
                    let output = parse_job_output(&core.history_get_job(&dir).ok()??)?;
                    let best = best_clip(&output.clips)?;
                    Some((
                        dir,
                        clip_file_path(&best.s3_url).to_string(),
                        thumbnail_seek(best),
                    ))
                })
                .collect::<Vec<_>>())
        });
        cx.spawn(async move |this, cx| {
            let Ok(found) = job.await else { return };
            let _ = this.update(cx, |this, cx| {
                for (dir, path, seek) in found {
                    this.run_thumbs.insert(dir, (path, seek));
                }
                this.queue_run_thumbs(cx);
            });
        })
        .detach();
    }

    /// (Re)queue the run cards' thumbnails; ready ones are left alone.
    fn queue_run_thumbs(&mut self, cx: &mut Context<Self>) {
        let order: Vec<(String, Option<f64>)> = self
            .entries
            .iter()
            .flat_map(|entries| entries.iter())
            .filter_map(|entry| self.run_thumbs.get(&entry.output_dir).cloned())
            .collect();
        self.thumbnails.update(cx, |thumbs, cx| {
            for (path, seek) in &order {
                thumbs.load(path, *seek, cx);
            }
        });
    }

    fn open_run(&mut self, dir: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_request += 1;
        let request = self.open_request;
        self.error = None;
        let core = state::core(cx);
        let read = dir.clone();
        let job = state::run(cx, async move { core.history_get_job(&read) });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if request != this.open_request {
                    return;
                }
                match result.map(|raw| raw.map(|raw| parse_job_output(&raw))) {
                    Ok(Some(Some(output))) => {
                        let list = cx.new(|cx| ClipList::new(dir, output, "Library", window, cx));
                        let back = cx.subscribe(&list, |this, _, _: &ClipListBack, cx| {
                            this.open = None;
                            // An edit may have re-exported a run's best clip.
                            this.queue_run_thumbs(cx);
                            cx.notify();
                        });
                        this.open = Some((list, back));
                    }
                    Ok(Some(None)) => this.error = Some("This run has an unsupported or damaged result file.".into()),
                    Ok(None) => {
                        this.error = Some(
                            "This run is no longer available. Its files may have moved or been removed.".into(),
                        )
                    }
                    Err(error) => this.error = Some(error_message(error, "Could not open this run.")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_folder(&mut self, dir: &str, cx: &mut Context<Self>) {
        match shell::open_path(dir, cx) {
            Ok(true) => {}
            Ok(false) => self.error = Some("This run folder is no longer available.".into()),
            Err(error) => {
                self.error = Some(error_message(error, "Could not open this run folder."))
            }
        }
        cx.notify();
    }

    fn refresh_button(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let disabled = self.refreshing;
        let glyph = ui::icon("refresh-cw").size(u(14.));
        div()
            .id("refresh")
            .flex()
            .items_center()
            .justify_center()
            .size(u(28.))
            .rounded(u(6.))
            .text_color(if disabled {
                t.tint(0.3)
            } else {
                t.ink_secondary
            })
            .when(!disabled, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(t.tint(0.1)).text_color(t.ink))
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx)))
            })
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Refresh").build(window, cx)
            })
            .child(if self.turning_since.is_some() {
                glyph
                    .with_animation(
                        "refresh-spin",
                        Animation::new(Duration::from_millis(TURN_MS)).repeat(),
                        |icon, delta| icon.rotate(percentage(delta)),
                    )
                    .into_any_element()
            } else {
                glyph.into_any_element()
            })
            .into_any_element()
    }

    fn run_card(
        &mut self,
        entry: &JobHistoryEntry,
        index: usize,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let failed = entry.status != "completed";
        let thumb = self
            .run_thumbs
            .get(&entry.output_dir)
            .and_then(|(path, _)| match self.thumbnails.read(cx).get(path) {
                Some(Thumb::Ready { path, .. }) => Some(path.clone()),
                _ => None,
            })
            .filter(|_| !failed);
        let backdrop = thumb.as_ref().and_then(|path| {
            self.thumbnails
                .update(cx, |thumbs, cx| thumbs.backdrop(path, cx))
        });
        let t = *t;
        let skeleton = move || div().size_full().child(ui::skeleton(&t)).into_any_element();

        let media = div()
            .relative()
            .w_full()
            .aspect_ratio(16. / 9.)
            .overflow_hidden()
            .rounded(u(8.))
            .bg(black().opacity(0.4))
            .map(|d| match (&thumb, failed) {
                (Some(path), _) => d
                    // Vertical clips sit on a blurred copy of themselves to fill the 16:9 frame.
                    .when_some(backdrop, |d, backdrop| {
                        d.child(
                            img(backdrop)
                                .absolute()
                                .left(relative(-0.125))
                                .top(relative(-0.125))
                                .w(relative(1.25))
                                .h(relative(1.25))
                                .object_fit(ObjectFit::Cover),
                        )
                    })
                    .child(
                        img(path.clone())
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Contain)
                            .with_fallback(skeleton),
                    ),
                (None, true) => d.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            ui::icon("triangle-alert")
                                .size(u(20.))
                                .text_color(t.danger.opacity(0.7)),
                        ),
                ),
                (None, false) => d.child(skeleton()),
            })
            .when(!failed, |d| {
                d.child(
                    media_chip()
                        .absolute()
                        .right(u(8.))
                        .bottom(u(8.))
                        .flex()
                        .items_center()
                        .gap(u(4.))
                        .h(u(18.))
                        .px(u(6.))
                        .rounded(u(4.))
                        .text_meta()
                        .font_weight(FontWeight::MEDIUM)
                        .font_features(tnum())
                        .child(ui::icon("clapperboard").size(u(12.)))
                        .child(format!(
                            "{} clip{}",
                            entry.clip_count,
                            if entry.clip_count == 1 { "" } else { "s" }
                        )),
                )
            });

        let title: SharedString = if failed {
            let kind = if entry.status == "incomplete" {
                "Unfinished"
            } else {
                "Unreadable"
            };
            format!("{kind} run · Open folder").into()
        } else {
            entry.video_title.clone().into()
        };
        let full_title: SharedString = entry.video_title.clone().into();
        let open_dir = entry.output_dir.clone();
        div()
            .id(ElementId::NamedInteger("run-card".into(), index as u64))
            .flex()
            .flex_col()
            .min_w_0()
            .rounded(u(12.))
            .border_1()
            .border_color(t.stroke)
            .bg(t.tint(0.03))
            .p(u(4.))
            .cursor_pointer()
            .hover(|s| s.border_color(t.tint(0.15)).bg(t.tint(0.05)))
            .on_click(cx.listener(move |this, _, window, cx| {
                if failed {
                    this.open_folder(&open_dir, cx);
                } else {
                    this.open_run(open_dir.clone(), window, cx);
                }
            }))
            .child(media)
            .child(
                div()
                    .px(u(6.))
                    .pb(u(6.))
                    .pt(u(8.))
                    .child(
                        div()
                            .id(ElementId::NamedInteger("run-title".into(), index as u64))
                            .min_h(u(36.))
                            .text_label()
                            .text_color(t.ink)
                            .line_clamp(2)
                            .text_ellipsis()
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(full_title.clone())
                                    .build(window, cx)
                            })
                            .child(title),
                    )
                    .child(
                        div()
                            .mt(u(2.))
                            .flex()
                            .items_center()
                            .gap(u(6.))
                            .text_meta()
                            .text_color(t.ink_secondary)
                            .font_features(tnum())
                            .child(
                                div()
                                    .whitespace_nowrap()
                                    .child(format_relative_date(&entry.date)),
                            )
                            .when_some(entry.total_cost_usd, |d, cost| {
                                d.child(div().text_color(t.ink_tertiary).child("·"))
                                    .child(format_usd(cost))
                            }),
                    ),
            )
            .into_any_element()
    }

    fn skeleton_card(t: &Tokens) -> AnyElement {
        ui::card(t)
            .p(u(4.))
            .child(
                div()
                    .w_full()
                    .aspect_ratio(16. / 9.)
                    .rounded(u(8.))
                    .overflow_hidden()
                    .child(ui::skeleton(t)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(u(8.))
                    .px(u(6.))
                    .pb(u(6.))
                    .pt(u(10.))
                    .child(div().h(u(14.)).w(relative(0.75)).child(ui::skeleton(t)))
                    .child(div().h(u(12.)).w(relative(1. / 3.)).child(ui::skeleton(t))),
            )
            .into_any_element()
    }
}

impl Render for LibraryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((list, _)) = &self.open {
            return list.clone().into_any_element();
        }
        let t = *cx.tokens();
        let output_dir = Settings::global(cx)
            .read(cx)
            .value
            .as_ref()
            .map(|s| s.output_directory.clone());
        let query = self.search.read(cx).value();
        let entries = self.entries.clone();
        let description = library_summary(entries.as_deref().map_or(&[], Vec::as_slice));

        let actions = div()
            .flex()
            .items_center()
            .gap(u(6.))
            .child(
                ui::Btn::new("go-jobs")
                    .ghost()
                    .icon("list-video")
                    .label("Jobs")
                    .on_click(|_, window, cx| workspace::go(Page::Jobs, window, cx)),
            )
            .child(self.refresh_button(&t, cx))
            .when_some(output_dir.filter(|dir| !dir.is_empty()), |d, dir| {
                d.child(
                    ui::Btn::new("open-library-folder")
                        .icon("folder-open")
                        .label("Open folder")
                        .on_click(move |_, _, cx| {
                            let _ = shell::open_path(&dir, cx);
                        }),
                )
            });

        let body = match &entries {
            None => auto_grid(&self.grid_width, 220., 10., (0..6).map(|_| Self::skeleton_card(&t)).collect(), window),
            Some(list) if list.is_empty() => ui::empty_state(
                "clapperboard",
                "No clips yet",
                Some("Generate clips from a long video and every run will show up here, newest first.".into()),
                Some(
                    ui::Btn::new("create-first")
                        .primary()
                        .icon("sparkles")
                        .label("Create your first clips")
                        .on_click(|_, window, cx| workspace::go(Page::Create, window, cx))
                        .into_any_element(),
                ),
                &t,
            ),
            Some(list) => {
                let list = list.clone();
                let shown = filter_runs(&list, &query);
                if shown.is_empty() {
                    ui::card(&t)
                        .flex()
                        .flex_col()
                        .items_center()
                        .px(u(20.))
                        .py(u(40.))
                        .child(ui::icon("search").size(u(16.)).text_color(t.ink_tertiary))
                        .child(
                            div()
                                .mt(u(10.))
                                .text_body()
                                .text_color(t.ink_secondary)
                                .child(format!("No runs match “{query}”.")),
                        )
                } else {
                    let cards = shown.into_iter().enumerate().map(|(i, entry)| self.run_card(entry, i, &t, cx)).collect();
                    auto_grid(&self.grid_width, 220., 10., cards, window)
                }
            }
        };

        div()
            .id("library-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                ui::page(ui::PAGE_WIDE)
                    .child(ui::page_header(
                        "Library",
                        Some(description.into()),
                        Some(actions.into_any_element()),
                        &t,
                    ))
                    .when_some(self.error.clone(), |d, error| {
                        d.child(div().mt(u(16.)).child(ui::callout(
                            CalloutTone::Danger,
                            None,
                            Some(div().child(error).into_any_element()),
                            None,
                            Some(Box::new(cx.listener(|this, _, _, cx| {
                                this.error = None;
                                cx.notify();
                            }))),
                            &t,
                        )))
                    })
                    .when(entries.as_ref().is_some_and(|e| !e.is_empty()), |d| {
                        d.child(
                            div().mt(u(16.)).max_w(u(320.)).child(
                                Input::new(&self.search)
                                    .prefix(
                                        ui::icon("search").size(u(14.)).text_color(t.ink_tertiary),
                                    )
                                    .aria_label("Search runs"),
                            ),
                        )
                    })
                    .child(div().mt(u(16.)).child(body)),
            )
            .into_any_element()
    }
}
