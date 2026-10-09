//! A finished run's clips (ClipList.tsx), shared by the Library and Jobs pages.
//!
//! ```ignore
//! let list = cx.new(|cx| ClipList::new(output_dir, output, "All jobs", window, cx));
//! cx.subscribe(&list, |this, _, _: &ClipListBack, cx| { /* close it */ });
//! ```
//! `output_dir` is the run folder (holding job_output.json), `output` the parsed
//! run, and the label names the back link above the title. The list fills the page
//! area and scrolls on its own; Edit opens the clip editor in its place and, when
//! the editor changed something, re-reads the run itself.
use super::clip_card::CardProps;
use super::run_stats::run_stats;
use super::thumbnails::{Thumb, Thumbnails};
use crate::editor::{EditorClosed, EditorPage};
use crate::shell;
use crate::state;
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, CalloutTone, Truncate, TypeScale, u};
use clipward_core::file_manager::ExportClip;
use clipward_core::job_view::{
    JobOutput, framing_problem, is_manual_run, parse_job_output, read_costs, source_analysis_notice,
};
use clipward_core::library::{
    ClipSort, clip_file_path, clip_title, export_outcome, is_vertical, select_all_state,
    sorted_clips, thumbnail_seek, top_pick, video_speed,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::time::Duration;

/// The user pressed the back link above the title.
pub struct ClipListBack;

pub struct ClipList {
    output_dir: String,
    output: JobOutput,
    back_label: SharedString,
    sort: ClipSort,
    selected: BTreeSet<u64>,
    exporting: bool,
    error: Option<String>,
    notice: Option<SharedString>,
    /// Hides the notice; replacing or dropping it cancels the timer.
    notice_timer: Option<Task<()>>,
    /// Shown in place of the list while a clip is being edited.
    editor: Option<(Entity<EditorPage>, Subscription)>,
    pub(super) hovered: Option<u64>,
    pub(super) action_errors: HashMap<u64, String>,
    pub(super) thumbnails: Entity<Thumbnails>,
    scroll: ScrollHandle,
    grid_width: Rc<Cell<Pixels>>,
    /// Where the in-flow toolbar was last frame, to float a copy once it scrolls under the top.
    toolbar_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    _observe: Subscription,
}

impl EventEmitter<ClipListBack> for ClipList {}

/// `error`, or `fallback` when it says nothing (lib/utils errorMessage).
pub(crate) fn error_message(error: String, fallback: &str) -> String {
    if error.trim().is_empty() {
        fallback.into()
    } else {
        error
    }
}

impl ClipList {
    pub fn new(
        output_dir: String,
        output: JobOutput,
        back_label: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let thumbnails = Thumbnails::global(cx);
        let observe = cx.observe(&thumbnails, |_, _, cx| cx.notify());
        let sort = if is_manual_run(&output) {
            ClipSort::Timeline
        } else {
            ClipSort::Score
        };
        let mut this = Self {
            output_dir,
            output: output.clone(),
            back_label: back_label.into(),
            sort,
            selected: BTreeSet::new(),
            exporting: false,
            error: None,
            notice: None,
            notice_timer: None,
            editor: None,
            hovered: None,
            action_errors: HashMap::new(),
            thumbnails,
            scroll: ScrollHandle::new(),
            grid_width: Rc::new(Cell::new(Pixels::ZERO)),
            toolbar_bounds: Rc::new(Cell::new(None)),
            _observe: observe,
        };
        this.set_output(output, cx);
        this
    }

    /// Show `output` (first open, or re-read after an edit): selection and notices reset.
    fn set_output(&mut self, output: JobOutput, cx: &mut Context<Self>) {
        self.selected.clear();
        self.error = None;
        self.notice = None;
        self.notice_timer = None;
        self.action_errors.clear();
        let wanted: Vec<(String, Option<f64>)> = sorted_clips(&output, self.sort)
            .into_iter()
            .map(|clip| {
                (
                    clip_file_path(&clip.s3_url).to_string(),
                    thumbnail_seek(clip),
                )
            })
            .collect();
        self.thumbnails.update(cx, |thumbs, cx| {
            for (path, seek) in &wanted {
                thumbs.load(path, *seek, cx);
            }
        });
        self.output = output;
        cx.notify();
    }

    pub(super) fn open_editor(
        &mut self,
        clip_index: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir = self.output_dir.clone();
        let title: SharedString = self
            .output
            .clips
            .iter()
            .find(|clip| clip.clip_index == clip_index)
            .map(clip_title)
            .unwrap_or_default()
            .into();
        let editor = cx.new(|cx| EditorPage::new(dir, clip_index as u32, title, window, cx));
        let subscription = cx.subscribe(&editor, |this, _, event: &EditorClosed, cx| {
            this.editor = None;
            // The clip's length, picture or edit status changed: re-read the run.
            if event.changed {
                this.reload(cx);
            }
            cx.notify();
        });
        self.editor = Some((editor, subscription));
        cx.notify();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = self
            .output
            .clips
            .iter()
            .map(|clip| clip_file_path(&clip.s3_url).to_string())
            .collect();
        self.thumbnails.update(cx, |thumbs, _| {
            paths.iter().for_each(|path| thumbs.forget(path))
        });
        let core = state::core(cx);
        let dir = self.output_dir.clone();
        let job = state::run(cx, async move { core.history_get_job(&dir) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Some(raw)) => match parse_job_output(&raw) {
                    Some(output) => this.set_output(output, cx),
                    None => this.fail(
                        "This run has an unsupported or damaged result file.".into(),
                        cx,
                    ),
                },
                Ok(None) => this.fail(
                    "This run is no longer available. Its files may have moved or been removed."
                        .into(),
                    cx,
                ),
                Err(error) => this.fail(error_message(error, "Could not reload this run."), cx),
            });
        })
        .detach();
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.error = Some(message);
        cx.notify();
    }

    pub(super) fn toggle(&mut self, clip_index: u64, cx: &mut Context<Self>) {
        if !self.selected.remove(&clip_index) {
            self.selected.insert(clip_index);
        }
        cx.notify();
    }

    fn export_selected(&mut self, cx: &mut Context<Self>) {
        let picked: Vec<ExportClip> = sorted_clips(&self.output, self.sort)
            .into_iter()
            .filter(|clip| self.selected.contains(&clip.clip_index))
            .map(|clip| ExportClip {
                path: clip_file_path(&clip.s3_url).into(),
                name: clip_title(clip),
            })
            .collect();
        if picked.is_empty() || self.exporting {
            return;
        }
        self.error = None;
        let core = state::core(cx);
        if let Err(error) = core.check_export(&picked) {
            return self.fail(
                error_message(error, "Could not export clips. Please try again."),
                cx,
            );
        }
        self.exporting = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = match shell::pick_folder("Export Clips To…").await {
                None => Ok(None),
                Some(dest) => {
                    // Not `state::run`: its future borrows the `cx` it was given.
                    let job = core
                        .runtime()
                        .clone()
                        .spawn(async move { core.export_clips(picked, dest).await });
                    job.await
                        .unwrap_or_else(|_| {
                            Err("Clipward hit an internal error. Retry the action.".into())
                        })
                        .map(Some)
                }
            };
            let _ = this.update(cx, |this, cx| {
                this.exporting = false;
                match result.map(|r| r.as_ref().and_then(export_outcome)) {
                    Ok(Some(Ok(notice))) => this.show_notice(notice, cx),
                    Ok(Some(Err(message))) => this.error = Some(message.into()),
                    Ok(None) => {}
                    Err(error) => {
                        this.error = Some(error_message(
                            error,
                            "Could not export clips. Please try again.",
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn show_notice(&mut self, notice: String, cx: &mut Context<Self>) {
        self.notice = Some(notice.into());
        self.notice_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(3500))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.notice = None;
                cx.notify();
            });
        }));
    }

    /// Floating card toolbar: select all, count, export, sort.
    fn toolbar(&self, clip_count: usize, manual: bool, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let selected = self.selected.len();
        let state = select_all_state(selected, clip_count);
        let all: Vec<u64> = self.output.clips.iter().map(|c| c.clip_index).collect();
        let this = cx.entity().downgrade();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(u(12.))
            .rounded(u(8.))
            .border_1()
            .border_color(t.tint(0.1))
            .bg(if t.dark {
                t.bg.opacity(0.94)
            } else {
                t.elevated
            })
            .py(u(4.))
            .pl(u(10.))
            .pr(u(4.))
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(u(12.))
                    .child(ui::checkbox(
                        "select-all",
                        state,
                        {
                            let this = this.clone();
                            move |_, _, cx| {
                                let _ = this.update(cx, |this, cx| {
                                    if this.selected.len() == all.len() && !all.is_empty() {
                                        this.selected.clear();
                                    } else {
                                        this.selected = all.iter().copied().collect();
                                    }
                                    cx.notify();
                                });
                            }
                        },
                        t,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(u(4.))
                            .whitespace_nowrap()
                            .text_caption()
                            .text_color(t.ink_secondary)
                            .map(|d| {
                                if selected > 0 {
                                    d.child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(t.ink)
                                            .child(selected.to_string()),
                                    )
                                    .child("selected")
                                } else {
                                    d.child(format!("{clip_count} clips"))
                                }
                            }),
                    )
                    .when_some(self.notice.clone(), |d, notice| {
                        d.child(
                            div()
                                .id("export-notice")
                                .flex()
                                .min_w_0()
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
                                .child(
                                    ui::icon("check")
                                        .size(u(10.))
                                        .text_color(t.success)
                                        .flex_shrink_0(),
                                )
                                .child(div().min_w_0().one_line().child(notice.clone()))
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(notice.clone())
                                        .build(window, cx)
                                })
                                .with_animation(
                                    "notice-in",
                                    Animation::new(Duration::from_millis(200)),
                                    |d, delta| d.opacity(delta),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(u(8.))
                    .when(selected > 0, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(u(6.))
                                .child(
                                    ui::Btn::new("clear-selection")
                                        .ghost()
                                        .small()
                                        .label("Clear")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.selected.clear();
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    ui::Btn::new("export-selected")
                                        .primary()
                                        .small()
                                        .icon("download")
                                        .label(format!("Export {selected}"))
                                        .loading(self.exporting)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.export_selected(cx)),
                                        ),
                                )
                                .child(div().mx(u(4.)).h(u(16.)).w(px(1.)).bg(t.stroke)),
                        )
                    })
                    .when(!manual, |d| {
                        d.child(ui::segmented(
                            "sort-clips",
                            &self.sort,
                            vec![
                                (ClipSort::Score, "Best first".into(), None),
                                (ClipSort::Timeline, "Timeline".into(), None),
                            ],
                            true,
                            move |sort, _, cx| {
                                let _ = this.update(cx, |this, cx| {
                                    this.sort = *sort;
                                    cx.notify();
                                });
                            },
                            t,
                        ))
                    }),
            )
    }
}

/// Records an element's window bounds each frame and redraws when they change, for
/// layout GPUI can't express (auto-fill grids, sticky strips).
pub(crate) fn probe(cell: Rc<Cell<Option<Bounds<Pixels>>>>) -> impl IntoElement {
    canvas(
        move |bounds, window, _| {
            if cell.get() != Some(bounds) {
                cell.set(Some(bounds));
                window.request_animation_frame();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

pub(crate) use crate::ui::back_link;

impl Render for ClipList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((editor, _)) = &self.editor {
            return editor.clone().into_any_element();
        }
        let t = *cx.tokens();
        let output = &self.output;
        let manual = is_manual_run(output);
        let clips = sorted_clips(output, self.sort);
        let thumbs = self.thumbnails.read(cx);
        let measured =
            output
                .clips
                .iter()
                .find_map(|clip| match thumbs.get(clip_file_path(&clip.s3_url)) {
                    Some(Thumb::Ready { aspect, .. }) => *aspect,
                    _ => None,
                });
        let vertical = is_vertical(output, measured);
        let speed = video_speed(output);
        let costs = read_costs(output.metrics.as_ref().and_then(|m| m.api_costs.as_ref()));
        let framing = framing_problem(output, vertical);
        let analysis = source_analysis_notice(output);
        let top = top_pick(output);
        let rem = f32::from(window.rem_size());
        let wide = f32::from(window.viewport_size().width) / rem * 16. >= 960.;
        let title: SharedString = if output.source_video_title.is_empty() {
            "Untitled video".into()
        } else {
            output.source_video_title.clone().into()
        };

        let props: Vec<CardProps> = clips
            .iter()
            .map(|clip| CardProps {
                clip: (*clip).clone(),
                vertical,
                top_pick: top == Some(clip.clip_index),
                show_score: !manual,
                selected: self.selected.contains(&clip.clip_index),
                selecting: !self.selected.is_empty(),
            })
            .collect();
        let clip_count = clips.len();
        let stats = run_stats(output, costs.as_ref(), speed, wide, &t);

        // Stick once the strip's top passes 8px below the scroll area's top.
        let viewport = self.scroll.bounds();
        let stuck = self
            .toolbar_bounds
            .get()
            .is_some_and(|b| b.origin.y < viewport.origin.y + u(8.).to_pixels(window.rem_size()));
        let toolbar = self.toolbar(clip_count, manual, &t, cx);
        let floating = stuck.then(|| {
            let b = self.toolbar_bounds.get().unwrap_or_default();
            self.toolbar(clip_count, manual, &t, cx)
                .absolute()
                .top(u(8.))
                .left(b.origin.x - viewport.origin.x)
                .w(b.size.width)
        });

        let output_dir = self.output_dir.clone();
        let header = div()
            .child(div().mb(u(8.)).child(back_link(
                self.back_label.clone(),
                &t,
                cx.listener(|_, _, _, cx| cx.emit(ClipListBack)),
            )))
            .child(ui::page_header(
                title,
                None,
                (!output_dir.is_empty()).then(|| {
                    ui::Btn::new("open-run-folder")
                        .icon("folder-open")
                        .label("Open folder")
                        .on_click(move |_, _, cx| {
                            let _ = shell::open_path(&output_dir, cx);
                        })
                        .into_any_element()
                }),
                &t,
            ));

        let grid = if props.is_empty() {
            div().mt(u(16.)).child(ui::empty_state(
                "clapperboard",
                "No clips in this run",
                Some(
                    "The run completed without saved clips. Check the run log for details.".into(),
                ),
                None,
                &t,
            ))
        } else {
            let cards: Vec<AnyElement> = props
                .into_iter()
                .map(|p| self.render_card(p, &t, cx))
                .collect();
            auto_grid(
                &self.grid_width,
                if vertical { 140. } else { 220. },
                10.,
                cards,
                window,
            )
            .mt(u(12.))
        };

        let toolbar_cell = self.toolbar_bounds.clone();
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .id("clip-list-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        ui::page(ui::PAGE_WIDE)
                            .child(header)
                            .when_some(speed, |d, speed| {
                                d.child(
                                    div()
                                        .mt(u(12.))
                                        .text_caption()
                                        .text_color(t.ink_secondary)
                                        .child(format!("All clips exported at {speed}× speed · Original voice pitch")),
                                )
                            })
                            .when_some(self.error.clone(), |d, error| {
                                d.child(div().mt(u(12.)).child(ui::callout(
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
                            .child(div().mt(u(16.)).child(stats))
                            .children([framing.map(SharedString::from), analysis.map(SharedString::from)].into_iter().flatten().map(
                                |notice| {
                                    div().mt(u(12.)).child(ui::callout(
                                        CalloutTone::Warning,
                                        None,
                                        Some(div().text_color(t.ink_secondary).child(notice).into_any_element()),
                                        None,
                                        None,
                                        &t,
                                    ))
                                },
                            ))
                            .child(
                                div()
                                    .relative()
                                    .mt(u(16.))
                                    .child(probe(toolbar_cell))
                                    .child(toolbar.when(stuck, |d| d.opacity(0.))),
                            )
                            .child(grid),
                    ),
            )
            .children(floating)
            .into_any_element()
    }
}

/// `grid-cols-[repeat(auto-fill,minmax(MIN,1fr))]` with a `gap`, holding `cards`.
/// GPUI grids take a column count, so the width the grid had last frame picks it.
pub(crate) fn auto_grid(
    cell: &Rc<Cell<Pixels>>,
    min: f32,
    gap: f32,
    cards: Vec<AnyElement>,
    window: &Window,
) -> Div {
    let rem = window.rem_size();
    let (min_px, gap_px) = (u(min).to_pixels(rem), u(gap).to_pixels(rem));
    // Before the first measurement, guess from the window: sidebar, page padding, page cap.
    let measured = cell.get();
    let width = if measured > Pixels::ZERO {
        measured
    } else {
        let viewport = window.viewport_size().width;
        let sidebar = u(if viewport < u(1024.).to_pixels(rem) {
            52.
        } else {
            220.
        })
        .to_pixels(rem);
        (viewport - sidebar - u(64.).to_pixels(rem)).min(u(ui::PAGE_WIDE - 64.).to_pixels(rem))
    };
    let columns = ((width + gap_px) / (min_px + gap_px)).floor().max(1.) as u16;
    let cell = cell.clone();
    div()
        .relative()
        .w_full()
        .child(
            canvas(
                move |bounds, window, _| {
                    if cell.get() != bounds.size.width {
                        cell.set(bounds.size.width);
                        window.request_animation_frame();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .child(div().grid().grid_cols(columns).gap(gap_px).children(cards))
}
