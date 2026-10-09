//! Every track of the timeline and every drag on it (Timeline.tsx `Tracks`).
//!
//! The main track plays its pieces back to back, CapCut style: drag a piece's edges to
//! trim it, drag the piece to reorder, click the ruler or a piece to move the playhead.
//! Above it sit the overlays, picture-in-picture, text and caption words; below, the
//! sounds. A drag follows the pointer anywhere in the window until the button comes up.
use super::media::{self, FileKey, Frames, StripKey, Wants};
use crate::editor::state::{EditorState, Selection, ZOOM_MAX, ZOOM_MIN};
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, Truncate, TypeScale, u};
use clipward_core::clip_engine::render::project::{AudioItem, OverlayBox, PipItem, Project, TextLayer, VideoItem};
use clipward_core::format::{format_ruler, to_fixed};
use clipward_core::overlays::OverlayKey;
use clipward_core::timeline::{self, Edge, EditedWord, snap_to_frame, sound_length};
use clipward_core::timeline_view::{self as view, TrackLayout};
use gpui_kit::component::input::{Enter, Escape, IndentInline, Input, InputEvent, InputState, OutdentInline};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

/// The zoom before the first fit (EditorPage.tsx's initial state), px per second.
pub const DEFAULT_ZOOM: f64 = 60.;
/// Lay out this much past each side of the view, CSS px.
const OVERSCAN: f64 = 400.;
/// Longest a typed caption word may be.
const MAX_WORD_CHARS: usize = 64;

fn f(p: Pixels) -> f64 {
    f32::from(p) as f64
}

/// What a press landed on.
#[derive(Clone, Debug)]
pub enum Target {
    /// The ruler or the playhead.
    Seek,
    Piece(usize),
    Trim(usize, Edge),
    Text(String, Option<Edge>),
    Sound(String, Option<Edge>),
    Overlay(OverlayKey, Option<Edge>),
    /// A caption word by its place in the edited words.
    Word(usize, Option<Edge>),
    Pip(String, Option<Edge>),
}

/// A press in progress, with what it started from.
enum Kind {
    Seek,
    Move { index: usize, video: Vec<VideoItem>, dx: f64 },
    Trim { index: usize, edge: Edge, video: Vec<VideoItem> },
    Text { layer: TextLayer, edge: Option<Edge> },
    Sound { item: AudioItem, edge: Option<Edge>, file_ms: i64 },
    Overlay { key: OverlayKey, box_: OverlayBox, edge: Option<Edge> },
    Word { word: EditedWord, index: usize, edge: Option<Edge> },
    Pip { item: PipItem, edge: Option<Edge>, file_ms: i64 },
}

struct Drag {
    kind: Kind,
    /// Where the press started, window px.
    x: Pixels,
    moved: bool,
    clicks: usize,
}

/// The inline box retyping a caption word.
struct Typing {
    index: usize,
    input: Entity<InputState>,
    _blur: Subscription,
}

/// CSS px to window px at the current interface scale, and time to x.
#[derive(Clone, Copy)]
struct Geo {
    s: f32,
    zoom: f64,
}

impl Geo {
    fn px(&self, css: f64) -> Pixels {
        px(css as f32 * self.s)
    }
    fn x(&self, ms: f64) -> Pixels {
        self.px(view::x_at(ms, self.zoom))
    }
    fn len(&self, ms: f64) -> f64 {
        ms * self.zoom / 1000.
    }
}

pub struct Tracks {
    editor: Entity<EditorState>,
    scroll: ScrollHandle,
    /// The scroller's bounds, window px, as last painted.
    viewport: Rc<Cell<Bounds<Pixels>>>,
    /// Its width, CSS px.
    view_width: f64,
    /// Window px per CSS px (the interface scale).
    scale: f32,
    /// The zoom the last fit chose; the user zoomed by hand once the zoom differs.
    fitted: Option<f64>,
    drag: Option<Drag>,
    /// The piece being dragged along the track and how far, CSS px.
    moving: Option<(usize, f64)>,
    /// The mouse is over the timeline (the preview follows it).
    hovering: bool,
    words: Rc<Vec<EditedWord>>,
    words_revision: Option<u64>,
    layout: Option<TrackLayout>,
    /// The word being typed over, by its place in `words`.
    typing: Option<usize>,
    input: Option<Typing>,
    _subscriptions: Vec<Subscription>,
}

impl Tracks {
    pub fn new(editor: Entity<EditorState>, scroll: ScrollHandle, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&editor, |this, _, cx| this.sync(cx)),
            // Thumbnails and waveforms arriving.
            cx.observe_global::<media::MediaCache>(|_, cx| cx.notify()),
        ];
        let mut this = Self {
            editor,
            scroll,
            viewport: Rc::new(Cell::new(Bounds::default())),
            view_width: 0.,
            scale: 1.,
            fitted: None,
            drag: None,
            moving: None,
            hovering: false,
            words: Rc::new(Vec::new()),
            words_revision: None,
            layout: None,
            typing: None,
            input: None,
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    /// The tracks' height, CSS px (0 until the clip loads).
    pub fn height(&self) -> f64 {
        self.layout.map_or(0., |l| l.height)
    }

    /// Window px per CSS px, and how far the tracks are scrolled, CSS px.
    pub fn geometry(&self) -> (f32, f64) {
        (self.scale, self.scroll_x())
    }

    fn scroll_x(&self) -> f64 {
        -f(self.scroll.offset().x) / self.scale as f64
    }

    /// After every edit, selection or zoom: the caption words and where tracks sit.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        let Some(project) = editor.project() else {
            self.layout = None;
            cx.notify();
            return;
        };
        let revision = editor.history.revision();
        if self.words_revision != Some(revision) {
            self.words_revision = Some(revision);
            self.words = Rc::new(if project.captions {
                timeline::edited_words(&project.video, &project.transcript)
            } else {
                Vec::new()
            });
        }
        self.layout = Some(view::track_layout(
            editor.overlays.len(),
            view::lane_rows(&timeline::pip_lanes(&project.pip)),
            view::lane_rows(&timeline::text_lanes(&project.texts)),
            !self.words.is_empty(),
            editor.track_height as f64,
            view::lane_rows(&timeline::sound_lanes(&project.audio)),
        ));
        let unfitted = editor.zoom.is_none();
        if unfitted {
            self.fit(cx);
        }
        cx.notify();
    }

    /// Fit the whole edit on first show, and again when the view resizes, until the
    /// user zooms by hand (the zoom then no longer matches the last fit).
    fn fit(&mut self, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        let total = editor.total();
        if self.view_width <= 0. || total <= 0. || (editor.zoom.is_some() && editor.zoom != self.fitted) {
            return;
        }
        let zoom = view::fit_zoom(self.view_width, total as i64, ZOOM_MIN);
        if editor.zoom == Some(zoom) {
            return;
        }
        self.fitted = Some(zoom);
        self.editor.update(cx, |e, cx| e.fit_zoom(zoom, cx));
    }

    fn resized(&mut self, width: f64, cx: &mut Context<Self>) {
        if (width - self.view_width).abs() >= 0.5 {
            self.view_width = width;
            self.fit(cx);
            cx.notify();
        }
    }

    fn zoom(&self, cx: &App) -> f64 {
        self.editor.read(cx).zoom.unwrap_or(DEFAULT_ZOOM)
    }

    /// The frame-snapped time under window x `x`, within the edit.
    fn time_at(&self, x: Pixels, cx: &App) -> f64 {
        let editor = self.editor.read(cx);
        let Some(project) = editor.project() else { return 0. };
        let content_x = f(x - self.viewport.get().origin.x) / self.scale as f64 + self.scroll_x();
        view::time_at(content_x, self.zoom(cx), &project.fps, editor.total() as i64)
    }

    /// Pinch or ⌘/Ctrl + scroll: zoom by `factor` about the pointer at window x `x`.
    fn zoom_at(&mut self, x: Pixels, factor: f64, cx: &mut Context<Self>) {
        if !self.editor.read(cx).editable() {
            return;
        }
        let pointer = f(x - self.viewport.get().origin.x) / self.scale as f64;
        let Some((zoom, scroll)) = view::zoom_about(self.zoom(cx), factor, ZOOM_MIN, ZOOM_MAX, pointer, self.scroll_x()) else {
            return;
        };
        self.editor.update(cx, |e, cx| e.set_zoom(zoom, cx));
        // Applied with the new width on the next layout, which clamps it.
        self.scroll.set_offset(point(px(-(scroll * self.scale as f64) as f32), px(0.)));
        cx.notify();
    }

    // ---- drags ----------------------------------------------------------------------

    /// A press on `target`; returns a listener for it.
    fn press(&self, target: Target, cx: &Context<Self>) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, e: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
            this.start(target.clone(), e, cx);
        })
    }

    pub fn start(&mut self, target: Target, e: &MouseDownEvent, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        let Some(project) = editor.project().filter(|_| editor.editable()) else { return };
        let kind = match target {
            Target::Seek => Some(Kind::Seek),
            Target::Piece(index) => Some(Kind::Move { index, video: project.video.clone(), dx: 0. }),
            Target::Trim(index, edge) => Some(Kind::Trim { index, edge, video: project.video.clone() }),
            Target::Text(id, edge) => project.texts.iter().find(|t| t.id == id).map(|layer| Kind::Text { layer: layer.clone(), edge }),
            Target::Sound(id, edge) => project.audio.iter().find(|a| a.id == id).map(|item| {
                let file_ms = match &item.file {
                    None => editor.opened.as_ref().map_or(item.src_out_ms, |o| o.source_duration_ms),
                    Some(file) => media::cache(cx)
                        .and_then(|c| c.peaks(&(editor.output_dir.clone(), Some(file.clone()))))
                        .map_or(item.src_out_ms, |p| view::waveform_length(p.len())),
                };
                Kind::Sound { item: item.clone(), edge, file_ms }
            }),
            Target::Overlay(key, edge) => {
                editor.overlays.iter().find(|o| o.key == key).map(|o| Kind::Overlay { key, box_: o.box_, edge })
            }
            Target::Word(index, edge) => self.words.get(index).map(|w| Kind::Word { word: w.clone(), index, edge }),
            Target::Pip(id, edge) => project.pip.iter().find(|p| p.id == id).map(|item| {
                let file_ms = project.media.iter().find(|m| m.file == item.media).map_or(item.src_out_ms, |m| m.duration_ms);
                Kind::Pip { item: item.clone(), edge, file_ms }
            }),
        };
        let Some(kind) = kind else { return };
        let seek = matches!(kind, Kind::Seek);
        self.drag = Some(Drag { kind, x: e.position.x, moved: false, clicks: e.click_count });
        self.hovering = false;
        let time = seek.then(|| self.time_at(e.position.x, cx));
        self.editor.update(cx, |e, cx| {
            e.set_hover(None, cx);
            if let Some(time) = time {
                e.set_scrubbing(true, cx);
                e.seek_paused(time, cx);
            }
        });
        cx.notify();
    }

    fn mouse_moved(&mut self, position: Point<Pixels>, inside: bool, cx: &mut Context<Self>) {
        if let Some(mut drag) = self.drag.take() {
            self.drag_to(&mut drag, position, cx);
            self.drag = Some(drag);
        } else if inside && self.editor.read(cx).editable() {
            // Hover scrubbing: the preview follows the mouse until it leaves.
            self.hovering = true;
            let time = self.time_at(position.x, cx);
            self.editor.update(cx, |e, cx| e.set_hover(Some(time), cx));
        } else {
            self.leave(cx);
        }
    }

    fn leave(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.hovering) {
            self.editor.update(cx, |e, cx| e.set_hover(None, cx));
        }
    }

    fn drag_to(&mut self, d: &mut Drag, position: Point<Pixels>, cx: &mut Context<Self>) {
        if let Kind::Seek = d.kind {
            let time = self.time_at(position.x, cx);
            self.editor.update(cx, |e, cx| e.seek_paused(time, cx));
            return;
        }
        let dx = f(position.x - d.x) / self.scale as f64;
        if !d.moved {
            if dx.abs() < view::DRAG_SLOP {
                return;
            }
            d.moved = true;
            let checkpoint = !matches!(d.kind, Kind::Move { .. });
            self.editor.update(cx, |e, cx| {
                e.set_scrubbing(true, cx);
                if checkpoint {
                    e.checkpoint(cx);
                }
            });
        }
        let editor = self.editor.read(cx);
        let Some(project) = editor.project() else { return };
        let fps = project.fps.clone();
        let total = editor.total() as i64;
        let ms = dx / (self.zoom(cx) / 1000.);
        let snap = |t: f64| snap_to_frame(t, &fps) as f64;
        match &mut d.kind {
            Kind::Seek => {}
            Kind::Word { word, edge, .. } => {
                let (start, end, seek) = view::drag_word(word, *edge, ms, total);
                let video = project.video.clone();
                let word = word.clone();
                self.editor.update(cx, |e, cx| {
                    e.edit_transcript(|tr| timeline::set_word_time(&video, tr, &word, start, end), cx);
                    e.seek_paused(seek, cx);
                });
            }
            Kind::Overlay { key, box_, edge } => {
                let (placed, seek) = view::drag_overlay(box_, *edge, snap_to_frame(ms, &fps), total);
                let key = *key;
                self.editor.update(cx, |e, cx| {
                    e.set_overlay_box(key, Some(placed), false, cx);
                    e.seek_paused(seek as f64, cx);
                });
            }
            Kind::Pip { item, edge, file_ms } => {
                let from = if *edge == Some(Edge::Out) { item.start_ms + sound_length(item) } else { item.start_ms };
                let at = snap(from as f64 + ms);
                let (id, edge, file_ms) = (item.id.clone(), *edge, *file_ms);
                self.editor.update(cx, |e, cx| {
                    e.edit_pip(
                        |pip| match edge {
                            Some(edge) => timeline::trim_sound(pip, &id, edge, at, file_ms),
                            None => timeline::move_sound(pip, &id, at),
                        },
                        false,
                        cx,
                    );
                    e.seek_paused(at.clamp(0., total as f64), cx);
                });
            }
            Kind::Sound { item, edge, file_ms } => {
                let from = if *edge == Some(Edge::Out) { item.start_ms + sound_length(item) } else { item.start_ms };
                let at = snap(from as f64 + ms);
                let (id, edge, file_ms) = (item.id.clone(), *edge, *file_ms);
                self.editor.update(cx, |e, cx| {
                    e.edit_sounds(
                        |audio| match edge {
                            Some(edge) => timeline::trim_sound(audio, &id, edge, at, file_ms),
                            None => timeline::move_sound(audio, &id, at),
                        },
                        false,
                        cx,
                    );
                    e.seek_paused(at.clamp(0., total as f64), cx);
                });
            }
            Kind::Text { layer, edge } => {
                let from = if *edge == Some(Edge::Out) { layer.end_ms } else { layer.start_ms };
                let at = snap(from as f64 + ms);
                let (id, edge) = (layer.id.clone(), *edge);
                self.editor.update(cx, |e, cx| {
                    e.edit_texts(
                        |texts| match edge {
                            Some(edge) => timeline::trim_text(texts, &id, edge, at, total),
                            None => timeline::move_text(texts, &id, at, total),
                        },
                        false,
                        cx,
                    );
                    e.seek_paused(at.min(total as f64), cx);
                });
            }
            Kind::Trim { index, edge, video } => {
                let Some(item) = video.get(*index) else { return };
                let base = if *edge == Edge::In { item.src_in_ms } else { item.src_out_ms };
                // A sped-up piece moves through its source faster than the timeline.
                let src = snap(base as f64 + ms * item.speed);
                let source_ms = editor.opened.as_ref().map_or(0, |o| o.source_duration_ms);
                // An added video trims within its own length.
                let bound = view::trim_bound(project, item, source_ms);
                let next = timeline::trim_item(video, *index, *edge, src, bound);
                // Show the frame at the edge being dragged.
                let seek = timeline::placements(&next)
                    .get(*index)
                    .map(|p| if *edge == Edge::In { p.start } else { p.start.max(p.end - 1) });
                self.editor.update(cx, |e, cx| {
                    e.edit_video(|_| next, false, cx);
                    if let Some(seek) = seek {
                        e.seek_paused(seek as f64, cx);
                    }
                });
            }
            Kind::Move { index, dx: moved, .. } => {
                *moved = dx;
                self.moving = Some((*index, dx));
                cx.notify();
            }
        }
    }

    fn end_drag(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        cx.notify();
        let time = self.time_at(position.x, cx);
        let total = self.editor.read(cx).total();
        let moved = d.moved;
        let zoom = self.zoom(cx);
        let editor = self.editor.clone();
        let release = |select: Selection, seek: f64, cx: &mut Context<Self>| {
            editor.update(cx, |e, cx| {
                if moved {
                    e.set_scrubbing(false, cx);
                } else {
                    e.select(Some(select), cx);
                    e.seek_paused(seek, cx);
                }
            });
        };
        match d.kind {
            Kind::Seek => editor.update(cx, |e, cx| e.set_scrubbing(false, cx)),
            Kind::Word { word, index, .. } => {
                editor.update(cx, |e, cx| {
                    if moved {
                        e.set_scrubbing(false, cx);
                    } else {
                        e.seek_paused(word.start as f64, cx);
                    }
                });
                // The second click of a double-click: type over the word.
                if !moved && d.clicks >= 2 {
                    self.type_word(Some(index), window, cx);
                }
            }
            Kind::Overlay { key, box_, .. } => {
                release(Selection::Overlay(key), (box_.start_ms as f64).max(time.min((box_.end_ms - 1) as f64)), cx)
            }
            Kind::Pip { item, .. } => release(Selection::Pip(item.id), time.min(total), cx),
            Kind::Sound { item, .. } => release(Selection::Sound(item.id), time.min(total), cx),
            Kind::Text { layer, .. } => {
                release(Selection::Text(layer.id), (layer.start_ms as f64).max(time.min((layer.end_ms - 1) as f64)), cx)
            }
            Kind::Trim { index, .. } | Kind::Move { index, .. } if !moved => {
                editor.update(cx, |e, cx| {
                    e.select(Some(Selection::Piece(index)), cx);
                    e.seek_paused(time, cx);
                });
            }
            Kind::Trim { .. } => editor.update(cx, |e, cx| e.set_scrubbing(false, cx)),
            Kind::Move { index, video, dx } => {
                self.moving = None;
                // Drop where the piece's middle landed.
                let target = view::drop_index(&video, index, dx / (zoom / 1000.));
                editor.update(cx, |e, cx| {
                    if target != index {
                        e.edit_video(|_| timeline::move_item(&video, index, target), true, cx);
                        e.select(Some(Selection::Piece(target)), cx);
                    }
                    e.set_scrubbing(false, cx);
                });
            }
        }
    }

    // ---- caption word typing ------------------------------------------------------------

    /// Open the box over word `index` (None closes it), focused with its text selected.
    fn type_word(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.typing = index;
        self.input = None;
        cx.notify();
        // After the edit that may have come before lands: the box shows the word as it is now.
        cx.defer_in(window, |this, window, cx| this.open_input(window, cx));
    }

    fn open_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.typing else { return };
        if self.input.as_ref().is_some_and(|t| t.index == index) {
            return;
        }
        let Some(word) = self.words.get(index) else {
            self.typing = None;
            return;
        };
        let text = word.text.clone();
        let input = cx.new(|cx| {
            InputState::new(window, cx).default_value(text).validate(|s, _| s.chars().count() <= MAX_WORD_CHARS)
        });
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        let blur = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Blur) && this.input.as_ref().is_some_and(|t| &t.input == input) {
                this.blur_word(window, cx);
            }
        });
        self.input = Some(Typing { index, input, _blur: blur });
        cx.notify();
    }

    /// Leaving the box saves the word.
    fn blur_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(typing) = self.input.as_ref() else { return };
        let value = typing.input.read(cx).value().to_string();
        self.save_word(&value, cx);
        self.type_word(None, window, cx);
    }

    fn save_word(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(word) = self.typing.and_then(|i| self.words.get(i)).cloned() else { return };
        if view::clean_word(text) == word.text {
            return;
        }
        let text = text.to_string();
        self.editor.update(cx, |e, cx| {
            e.checkpoint(cx);
            e.edit_transcript(|tr| timeline::set_word(tr, word.seg, word.word, &text), cx);
        });
    }

    /// Enter saves and closes; Tab saves and moves on (Shift+Tab back).
    fn commit_word(&mut self, tab: bool, shift: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(index), Some(typing)) = (self.typing, self.input.as_ref()) else { return };
        let value = typing.input.read(cx).value().to_string();
        let kept = !value.trim().is_empty();
        let words = self.words.clone();
        self.save_word(&value, cx);
        let next = view::next_word(index, shift, kept, tab, words.len());
        if let Some(word) = next.and_then(|n| words.get(n)) {
            let start = word.start as f64;
            self.editor.update(cx, |e, cx| e.seek_paused(start, cx));
        }
        self.type_word(next, window, cx);
    }

    fn cancel_word(&mut self, cx: &mut Context<Self>) {
        self.typing = None;
        self.input = None;
        cx.notify();
    }
}

/// Lets a timeline fit the zoom without the clamp `set_zoom` applies (as the React
/// `onZoomToFit` did): a very short clip fits wider than ZOOM_MAX.
impl EditorState {
    fn fit_zoom(&mut self, zoom: f64, cx: &mut Context<Self>) {
        if self.zoom != Some(zoom) {
            self.zoom = Some(zoom);
            cx.notify();
        }
    }
}

// ---- rendering ------------------------------------------------------------------------

impl Render for Tracks {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.scale = f32::from(window.rem_size()) / 16.;
        let t = *cx.tokens();
        let entity = cx.entity();
        let editor = self.editor.clone();
        let editor = editor.read(cx);
        let (Some(project), Some(layout)) = (editor.project(), self.layout) else {
            return div().size_full().into_any_element();
        };
        let g = Geo { s: self.scale, zoom: editor.zoom.unwrap_or(DEFAULT_ZOOM) };
        let total = editor.total();
        let extent = view::extent(project);
        let width = view::content_width(extent, g.zoom, self.view_width);
        let scroll_x = self.scroll_x();
        let view_width = if self.view_width > 0. { self.view_width } else { 1600. };
        let visible = (scroll_x - OVERSCAN, scroll_x + view_width + OVERSCAN);
        let mut wants = Wants::default();

        let mut content = div().relative().w(g.px(width)).h(g.px(layout.height));
        content = content.child(self.ruler(g, width, visible, &t, cx));
        if let Some(top) = layout.overlays {
            content = content.children(self.overlay_track(g, top, total, editor, &t, cx));
        }
        if let Some(top) = layout.pip {
            content = content.children(self.pip_track(g, top, project, editor, &t, cx));
        }
        if let Some(top) = layout.texts {
            content = content.children(self.text_track(g, top, total, project, editor, &t, cx));
        }
        if let Some(top) = layout.words {
            content = content.children(self.word_track(g, top, visible, &t, cx));
        }
        content = content.children(self.main_track(g, layout.main, project, editor, visible, &mut wants, &t, cx));
        if let Some(top) = layout.sounds {
            content = content.children(self.sound_track(g, top, project, editor, &mut wants, &t, cx));
        }
        content = content.children(transitions(g, layout.height, project, &t));
        // Past the end of the video: sounds there are cut from the export.
        if extent as f64 > total {
            content = content.child(hatch(g, total, extent as f64, layout.height, &t));
        }

        if !wants.is_empty() {
            cx.defer(move |cx| media::request(wants, cx));
        }

        // Measures the view, takes ⌘/pinch zooms before the scroller scrolls, follows the
        // mouse for hover scrubbing and, during a drag, anywhere in the window.
        let viewport = self.viewport.clone();
        let last_width = self.view_width;
        let scale = self.scale;
        let dragging = self.drag.as_ref().map(|d| match d.kind {
            Kind::Move { .. } => CursorStyle::ClosedHand,
            Kind::Seek | Kind::Trim { .. } => CursorStyle::ResizeLeftRight,
            Kind::Text { edge, .. }
            | Kind::Sound { edge, .. }
            | Kind::Overlay { edge, .. }
            | Kind::Word { edge, .. }
            | Kind::Pip { edge, .. } => {
                if edge.is_some() {
                    CursorStyle::ResizeLeftRight
                } else {
                    CursorStyle::OpenHand
                }
            }
        });
        let measure = canvas(
            {
                let entity = entity.clone();
                move |bounds, window, cx| {
                    viewport.set(bounds);
                    let width = f(bounds.size.width) / scale as f64;
                    if (width - last_width).abs() >= 0.5 {
                        cx.defer(move |cx| entity.update(cx, |this, cx| this.resized(width, cx)));
                    }
                    window.insert_hitbox(bounds, HitboxBehavior::Normal)
                }
            },
            move |_, hitbox, window, _| {
                if let Some(cursor) = dragging {
                    window.set_window_cursor_style(cursor);
                }
                let (moves, hover) = (entity.clone(), hitbox.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        let inside = hover.is_hovered(window);
                        moves.update(cx, |this, cx| this.mouse_moved(e.position, inside, cx));
                    }
                });
                let exits = entity.clone();
                window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        exits.update(cx, |this, cx| {
                            if this.drag.is_none() {
                                this.leave(cx)
                            }
                        });
                    }
                });
                if dragging.is_some() {
                    let ups = entity.clone();
                    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Bubble {
                            ups.update(cx, |this, cx| this.end_drag(e.position, window, cx));
                        }
                    });
                }
                let (wheels, over) = (entity.clone(), hitbox.clone());
                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture
                        && (e.modifiers.platform || e.modifiers.control)
                        && over.is_hovered(window)
                    {
                        cx.stop_propagation();
                        // GPUI's y grows as content moves down: the browser's deltaY reversed.
                        let dy = f(e.delta.pixel_delta(window.line_height()).y);
                        let factor = view::wheel_zoom_factor(-dy);
                        wheels.update(cx, |this, cx| this.zoom_at(e.position.x, factor, cx));
                    }
                });
                let (pinches, under) = (entity.clone(), hitbox);
                window.on_mouse_event(move |e: &PinchEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && under.is_hovered(window) {
                        cx.stop_propagation();
                        let factor = 1. + e.delta as f64;
                        pinches.update(cx, |this, cx| this.zoom_at(e.position.x, factor, cx));
                    }
                });
            },
        )
        .absolute()
        .size_full();

        div()
            .relative()
            .size_full()
            .child(
                div()
                    .id("timeline-scroller")
                    .size_full()
                    .overflow_x_scroll()
                    .overflow_y_hidden()
                    .track_scroll(&self.scroll)
                    .child(content),
            )
            .child(measure)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .child(gpui_kit::component::scroll::Scrollbar::horizontal(&self.scroll)),
            )
            .into_any_element()
    }
}

/// An edge handle on an item: drag it to trim.
fn edge(edge: Edge, width: f32, hover: Hsla, on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static) -> Div {
    div()
        .absolute()
        .top_0()
        .h_full()
        .w(u(width))
        .map(|d| if edge == Edge::In { d.left_0() } else { d.right_0() })
        .cursor(CursorStyle::ResizeLeftRight)
        .hover(move |s| s.bg(hover))
        .on_mouse_down(MouseButton::Left, on_down)
}

/// A lane item's box: icon and label, a tinted fill and border, both edges.
#[allow(clippy::too_many_arguments)]
fn lane_box(
    id: ElementId,
    left: Pixels,
    top: Pixels,
    width: Pixels,
    height: f32,
    hue: Hsla,
    fill: f32,
    selected: bool,
    t: &Tokens,
) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .left(left)
        .top(top)
        .w(width)
        .h(u(height))
        .flex()
        .items_center()
        .gap(u(4.))
        .overflow_hidden()
        .rounded(u(6.))
        .border_1()
        .px(u(8.))
        .text_meta()
        .whitespace_nowrap()
        .bg(hue.opacity(fill))
        .text_color(t.tint(0.8))
        .cursor(CursorStyle::OpenHand)
        .map(|d| {
            if selected {
                d.border_color(hue)
            } else {
                d.border_color(hue.opacity(0.3)).hover(move |s| s.border_color(hue.opacity(0.6)))
            }
        })
}

fn label(text: impl Into<SharedString>) -> Div {
    div().min_w_0().one_line().child(text.into())
}

impl Tracks {
    fn ruler(&self, g: Geo, width: f64, visible: (f64, f64), t: &Tokens, cx: &Context<Self>) -> impl IntoElement {
        let ticks = view::ruler_ticks(g.zoom, width, visible.0, visible.1);
        div()
            .id("ruler")
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h(u(view::RULER_H as f32))
            .border_b_1()
            .border_color(t.stroke)
            .cursor(CursorStyle::PointingHand)
            .on_mouse_down(MouseButton::Left, self.press(Target::Seek, cx))
            .children(ticks.into_iter().map(|(x, seconds)| {
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .left(g.px(x))
                    .border_l_1()
                    .border_color(t.tint(0.15))
                    .pl(u(4.))
                    .text_meta()
                    .whitespace_nowrap()
                    .text_color(t.ink_tertiary)
                    .child(format_ruler(seconds))
            }))
    }

    /// Title, banner and logos: a dashed box while automatic.
    fn overlay_track(&self, g: Geo, top: f64, total: f64, editor: &EditorState, t: &Tokens, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut items: Vec<(bool, AnyElement)> = editor
            .overlays
            .iter()
            .enumerate()
            .map(|(row, o)| {
                let selected = editor.selection == Some(Selection::Overlay(o.key));
                let from = (o.box_.start_ms as f64).min(total);
                let to = (o.box_.end_ms as f64).min(total);
                let element = div()
                    .id(("overlay", row))
                    .absolute()
                    .left(g.x(from))
                    .top(g.px(top + row as f64 * view::OVERLAY_ROW))
                    .w(g.px((g.len(to - from) - 1.).max(4.)))
                    .h(u(view::OVERLAY_H as f32))
                    .flex()
                    .items_center()
                    .gap(u(4.))
                    .overflow_hidden()
                    .rounded(u(4.))
                    .border_1()
                    .px(u(8.))
                    .text_meta()
                    .whitespace_nowrap()
                    .bg(t.tint(0.1))
                    .text_color(t.tint(0.8))
                    .cursor(CursorStyle::OpenHand)
                    .when(!o.placed, |d| d.border_dashed())
                    .map(|d| {
                        if selected {
                            d.border_color(t.tint(0.7))
                        } else {
                            let hover = t.tint(0.5);
                            d.border_color(t.tint(0.25)).hover(move |s| s.border_color(hover))
                        }
                    })
                    .on_mouse_down(MouseButton::Left, self.press(Target::Overlay(o.key, None), cx))
                    .child(ui::icon("layers").size(u(12.)).flex_shrink_0())
                    .child(label(o.label.clone()))
                    .child(edge(Edge::In, 6., t.tint(0.5), self.press(Target::Overlay(o.key, Some(Edge::In)), cx)))
                    .child(edge(Edge::Out, 6., t.tint(0.5), self.press(Target::Overlay(o.key, Some(Edge::Out)), cx)))
                    .into_any_element();
                (selected, element)
            })
            .collect();
        on_top(&mut items)
    }

    fn pip_track(&self, g: Geo, top: f64, project: &Project, editor: &EditorState, t: &Tokens, cx: &Context<Self>) -> Vec<AnyElement> {
        let lanes = timeline::pip_lanes(&project.pip);
        let mut items: Vec<(bool, AnyElement)> = project
            .pip
            .iter()
            .map(|item| {
                let selected = editor.selection == Some(Selection::Pip(item.id.clone()));
                let name = project.media.iter().find(|m| m.file == item.media).map_or(item.media.clone(), |m| m.name.clone());
                let lane = lanes.get(&item.id).copied().unwrap_or(0) as f64;
                let element = lane_box(
                    ElementId::Name(format!("pip-{}", item.id).into()),
                    g.x(item.start_ms as f64),
                    g.px(top + lane * view::LANE_ROW),
                    g.px((g.len(sound_length(item) as f64) - 1.).max(4.)),
                    view::LANE_H as f32,
                    t.accent,
                    0.1,
                    selected,
                    t,
                )
                .on_mouse_down(MouseButton::Left, self.press(Target::Pip(item.id.clone(), None), cx))
                .child(ui::icon("picture-in-picture-2").size(u(12.)).flex_shrink_0())
                .child(label(name))
                .child(edge(Edge::In, 6., t.accent.opacity(0.7), self.press(Target::Pip(item.id.clone(), Some(Edge::In)), cx)))
                .child(edge(Edge::Out, 6., t.accent.opacity(0.7), self.press(Target::Pip(item.id.clone(), Some(Edge::Out)), cx)))
                .into_any_element();
                (selected, element)
            })
            .collect();
        on_top(&mut items)
    }

    #[allow(clippy::too_many_arguments)]
    fn text_track(&self, g: Geo, top: f64, total: f64, project: &Project, editor: &EditorState, t: &Tokens, cx: &Context<Self>) -> Vec<AnyElement> {
        let lanes = timeline::text_lanes(&project.texts);
        let mut items: Vec<(bool, AnyElement)> = project
            .texts
            .iter()
            .map(|layer| {
                let selected = editor.selection == Some(Selection::Text(layer.id.clone()));
                let start = (layer.start_ms as f64).min(total);
                let end = (layer.end_ms as f64).min(total);
                let lane = lanes.get(&layer.id).copied().unwrap_or(0) as f64;
                let text = layer.text.split_whitespace().collect::<Vec<_>>().join(" ");
                let element = lane_box(
                    ElementId::Name(format!("text-{}", layer.id).into()),
                    g.x(start),
                    g.px(top + lane * view::LANE_ROW),
                    g.px((g.len(end - start) - 1.).max(4.)),
                    view::LANE_H as f32,
                    t.warning,
                    0.2,
                    selected,
                    t,
                )
                .on_mouse_down(MouseButton::Left, self.press(Target::Text(layer.id.clone(), None), cx))
                .child(ui::icon("type").size(u(12.)).flex_shrink_0())
                .child(label(text))
                .child(edge(Edge::In, 6., t.warning.opacity(0.7), self.press(Target::Text(layer.id.clone(), Some(Edge::In)), cx)))
                .child(edge(Edge::Out, 6., t.warning.opacity(0.7), self.press(Target::Text(layer.id.clone(), Some(Edge::Out)), cx)))
                .into_any_element();
                (selected, element)
            })
            .collect();
        on_top(&mut items)
    }

    /// Caption words: drag a word, or its edges, to retime it; double-click to retype it.
    /// Only the words near the view are laid out.
    fn word_track(&self, g: Geo, top: f64, visible: (f64, f64), t: &Tokens, cx: &Context<Self>) -> Vec<AnyElement> {
        let per_ms = g.zoom / 1000.;
        let range = view::visible_words(&self.words, (visible.0 - view::GUTTER) / per_ms, (visible.1 - view::GUTTER) / per_ms);
        let (border, fill, hover_border, hover_fill, ink) = (t.tint(0.15), t.tint(0.1), t.tint(0.5), t.tint(0.2), t.tint(0.8));
        let mut out: Vec<AnyElement> = Vec::with_capacity(range.len() + 1);
        for i in range {
            let w = &self.words[i];
            let span = g.len(w.end - w.start as f64);
            if view::x_at(w.end, g.zoom) < visible.0 {
                continue;
            }
            let tip: SharedString = format!("{} (double-click to edit)", w.text).into();
            out.push(
                div()
                    .id(("word", i))
                    .absolute()
                    .top(g.px(top))
                    .left(g.x(w.start as f64))
                    .w(g.px((span - 1.).max(2.)))
                    .h(u(view::WORDS_H as f32))
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .rounded(u(4.))
                    .border_1()
                    .border_color(border)
                    .bg(fill)
                    .px(u(4.))
                    .text_size(u(11.))
                    .line_height(u(11.))
                    .whitespace_nowrap()
                    .text_color(ink)
                    .cursor(CursorStyle::OpenHand)
                    .hover(move |s| s.border_color(hover_border).bg(hover_fill))
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    .on_mouse_down(MouseButton::Left, self.press(Target::Word(i, None), cx))
                    // Too narrow for a readable word: the box alone (the tooltip has it).
                    .when(span >= 20., |d| d.child(label(w.text.clone())))
                    .child(edge(Edge::In, 4., hover_border, self.press(Target::Word(i, Some(Edge::In)), cx)))
                    .child(edge(Edge::Out, 4., hover_border, self.press(Target::Word(i, Some(Edge::Out)), cx)))
                    .into_any_element(),
            );
        }
        if let (Some(typing), Some(w)) = (self.input.as_ref(), self.input.as_ref().and_then(|t| self.words.get(t.index))) {
            out.push(
                div()
                    .id("word-input")
                    .absolute()
                    .top(g.px(top))
                    .left(g.x(w.start as f64))
                    .w(g.px(g.len(w.end - w.start as f64).max(96.)))
                    .h(u(view::WORDS_H as f32))
                    .rounded(u(4.))
                    .border_1()
                    .border_color(t.accent)
                    .bg(t.bg)
                    .px(u(4.))
                    .text_size(u(11.))
                    .text_color(t.ink)
                    .flex()
                    .items_center()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // A press anywhere else leaves the box, as a blur does.
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| this.blur_word(window, cx)))
                    .capture_action(cx.listener(|this, a: &Enter, window, cx| {
                        cx.stop_propagation();
                        this.commit_word(false, a.shift, window, cx);
                    }))
                    .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                        cx.stop_propagation();
                        this.commit_word(true, false, window, cx);
                    }))
                    .capture_action(cx.listener(|this, _: &OutdentInline, window, cx| {
                        cx.stop_propagation();
                        this.commit_word(true, true, window, cx);
                    }))
                    .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                        cx.stop_propagation();
                        this.cancel_word(cx);
                    }))
                    .child(
                        Input::new(&typing.input)
                            .appearance(false)
                            .h_full()
                            .px(px(0.))
                            .py(px(0.))
                            .text_size(u(11.))
                            .line_height(u(14.)),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    /// The pieces, back to back: frames above the source's sound, a chip, crop keyframes.
    #[allow(clippy::too_many_arguments)]
    fn main_track(
        &self,
        g: Geo,
        top: f64,
        project: &Project,
        editor: &EditorState,
        visible: (f64, f64),
        wants: &mut Wants,
        t: &Tokens,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let track = editor.track_height as f64;
        let cache = media::cache(cx);
        let dir = editor.output_dir.clone();
        let source_peaks: Option<media::Peaks> = cache.and_then(|c| c.peaks(&(dir.clone(), None))).cloned();
        let has_source = project.video.iter().any(|v| v.media.is_none());
        if has_source && source_peaks.is_none() {
            wants.peaks.push((dir.clone(), None));
        }
        let mut items: Vec<(u8, AnyElement)> = Vec::new();
        for p in timeline::placements(&project.video) {
            let item = p.item;
            let selected = editor.selection == Some(Selection::Piece(p.index));
            let dx = self.moving.filter(|m| m.0 == p.index).map(|m| m.1);
            let piece_width = (g.len((p.end - p.start) as f64) - 1.).max(2.);
            let left = view::x_at(p.start as f64, g.zoom) + dx.unwrap_or(0.);
            let file = item.media.clone();
            let media_file = file.as_ref().and_then(|m| project.media.iter().find(|f| &f.file == m));
            let (fw, fh) = media_file.map_or((project.source_width, project.source_height), |m| (m.width, m.height));
            // The source's sound shows under the frames, the two halves of the piece;
            // added media fills it.
            let band = if file.is_some() { 0. } else { (track / 2.).round() };
            let strip_height = track - band;
            let crop_x = if item.framing == "crop" { item.crop.map_or(0.5, |c| c.x) } else { 0.5 };
            let still = media_file.is_some_and(|m| m.image);

            let strip: AnyElement = if still {
                let key: FileKey = (dir.clone(), file.clone());
                let image = cache.and_then(|c| c.image(&key)).cloned();
                if image.is_none() {
                    wants.images.push(key);
                }
                still_strip(image).absolute().top_0().left_0().w_full().h(g.px(strip_height)).into_any_element()
            } else {
                let tiles = view::filmstrip_tiles(
                    item.src_in_ms,
                    item.src_out_ms,
                    piece_width,
                    strip_height,
                    fw as f64 / (fh.max(1)) as f64,
                    project.width as f64 / project.height.max(1) as f64,
                );
                let x0 = left + 1.;
                let shown = view::visible_tiles(tiles.tile, tiles.seconds.len(), visible.0 - x0, visible.1 - x0);
                let mut frames: HashMap<u32, Frames> = HashMap::new();
                let key = |block: u32| -> StripKey { (dir.clone(), file.clone(), block) };
                let now = view::blocks_for(&tiles.seconds, shown.clone());
                for block in view::blocks_for(&tiles.seconds, 0..tiles.seconds.len()) {
                    match cache.and_then(|c| c.strip(&key(block))) {
                        Some(f) => {
                            frames.insert(block, f.clone());
                        }
                        None if now.contains(&block) => wants.strips_now.push(key(block)),
                        None => wants.strips_later.push(key(block)),
                    }
                }
                filmstrip(tiles, shown, frames, crop_x, g.s)
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .h(g.px(strip_height))
                    .into_any_element()
            };

            let mut piece = div()
                .id(ElementId::Name(format!("piece-{}", item.id).into()))
                .absolute()
                .left(g.px(left))
                .top(g.px(top))
                .w(g.px(piece_width))
                .h(g.px(track))
                .overflow_hidden()
                .rounded(u(6.))
                .border_1()
                .text_meta()
                .bg(black().opacity(0.4))
                .text_color(t.ink)
                .cursor(if dx.is_some() { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
                .map(|d| {
                    if selected {
                        d.border_color(t.accent)
                            .shadow(vec![BoxShadow::new(px(0.), px(0.), t.accent).spread_radius(px(1.))])
                    } else {
                        let hover = t.tint(0.4);
                        d.border_color(t.tint(0.15)).hover(move |s| s.border_color(hover))
                    }
                })
                .when(dx.is_some(), |d| d.opacity(0.8).shadow_lg())
                .on_mouse_down(MouseButton::Left, self.press(Target::Piece(p.index), cx))
                .child(strip);
            if band > 0. {
                let muted = item.volume == 0.;
                let peaks = source_peaks.clone();
                let (from, to) = (item.src_in_ms, item.src_out_ms);
                let color = t.accent.opacity(if muted { 0.25 } else { 0.8 });
                piece = piece.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .w_full()
                        .h(g.px(band))
                        .border_t_1()
                        .border_color(black().opacity(0.5))
                        .bg(black().opacity(0.6))
                        .py(u(4.))
                        .child(waveform(peaks, from, to, color).size_full()),
                );
            }
            if piece_width >= 48. {
                piece = piece.child(chip(item, media_file.map(|m| m.name.clone()), piece_width, t));
            }
            if item.framing == "crop" && !item.keyframes.is_empty() {
                let keys: Vec<f64> = item.keyframes.iter().map(|k| g.len(k.t_ms as f64)).collect();
                let warning = t.warning;
                piece = piece.child(
                    canvas(|_, _, _| {}, move |bounds, _, window, _| {
                        for x in keys {
                            // An 8 px square turned 45°, its bottom 4 px above the middle.
                            let centre = point(bounds.left() + px(x as f32 * g.s), bounds.top() + bounds.size.height / 2. - px(8. * g.s));
                            paint_diamond(centre, 4. * std::f32::consts::SQRT_2 * g.s + g.s, black().opacity(0.4), window);
                            paint_diamond(centre, 4. * std::f32::consts::SQRT_2 * g.s, warning, window);
                        }
                    })
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                );
            }
            let handle = |e: Edge| {
                let base = if selected { t.accent.opacity(0.5) } else { t.accent.opacity(0.) };
                let hover = t.accent.opacity(0.7);
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .w(u(8.))
                    .bg(base)
                    .map(|d| if e == Edge::In { d.left_0().rounded_l(u(6.)) } else { d.right_0().rounded_r(u(6.)) })
                    .cursor(CursorStyle::ResizeLeftRight)
                    .hover(move |s| s.bg(hover))
                    .on_mouse_down(MouseButton::Left, self.press(Target::Trim(p.index, e), cx))
            };
            piece = piece.child(handle(Edge::In)).child(handle(Edge::Out));
            let layer = if dx.is_some() { 2 } else { selected as u8 };
            items.push((layer, piece.into_any_element()));
        }
        items.sort_by_key(|(layer, _)| *layer);
        items.into_iter().map(|(_, e)| e).collect()
    }

    /// Sounds in lanes, each over its own waveform.
    #[allow(clippy::too_many_arguments)]
    fn sound_track(
        &self,
        g: Geo,
        top: f64,
        project: &Project,
        editor: &EditorState,
        wants: &mut Wants,
        t: &Tokens,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let lanes = timeline::sound_lanes(&project.audio);
        let cache = media::cache(cx);
        let mut items: Vec<(bool, AnyElement)> = project
            .audio
            .iter()
            .map(|item| {
                let selected = editor.selection == Some(Selection::Sound(item.id.clone()));
                let key: FileKey = (editor.output_dir.clone(), item.file.clone());
                let peaks = cache.and_then(|c| c.peaks(&key)).cloned();
                if peaks.is_none() {
                    wants.peaks.push(key);
                }
                let lane = lanes.get(&item.id).copied().unwrap_or(0) as f64;
                let element = lane_box(
                    ElementId::Name(format!("sound-{}", item.id).into()),
                    g.x(item.start_ms as f64),
                    g.px(top + lane * view::SOUND_ROW),
                    g.px((g.len(sound_length(item) as f64) - 1.).max(4.)),
                    view::SOUND_H as f32,
                    t.success,
                    0.15,
                    selected,
                    t,
                )
                .on_mouse_down(MouseButton::Left, self.press(Target::Sound(item.id.clone(), None), cx))
                .child(waveform(peaks, item.src_in_ms, item.src_out_ms, t.success.opacity(0.5)).absolute().top_0().left_0().size_full())
                .child(ui::icon("music").size(u(12.)).flex_shrink_0())
                .child(label(item.name.clone()))
                .when(item.duck, |d| {
                    d.child(div().flex_shrink_0().rounded(u(4.)).bg(t.tint(0.15)).px(u(4.)).child("Duck"))
                })
                .child(edge(Edge::In, 6., t.success.opacity(0.7), self.press(Target::Sound(item.id.clone(), Some(Edge::In)), cx)))
                .child(edge(Edge::Out, 6., t.success.opacity(0.7), self.press(Target::Sound(item.id.clone(), Some(Edge::Out)), cx)))
                .into_any_element();
                (selected, element)
            })
            .collect();
        on_top(&mut items)
    }
}

/// The selected item draws over its neighbours.
fn on_top(items: &mut Vec<(bool, AnyElement)>) -> Vec<AnyElement> {
    items.sort_by_key(|(selected, _)| *selected);
    items.drain(..).map(|(_, e)| e).collect()
}

/// The chip over a piece: its file, mute, framing, speed and length.
fn chip(item: &VideoItem, media: Option<String>, piece_width: f64, t: &Tokens) -> impl IntoElement {
    let tag = |text: SharedString| div().flex_shrink_0().rounded(u(4.)).bg(t.tint(0.2)).px(u(4.)).child(text);
    let framing: Option<SharedString> = match item.framing.as_str() {
        "fit" => Some("Fit".into()),
        "crop" => Some("Crop".into()),
        other => view::split_label(other).map(|n| format!("Split {n}").into()),
    };
    div()
        .absolute()
        .left(u(4.))
        .top(u(4.))
        .max_w(u((piece_width - 8.) as f32))
        .flex()
        .items_center()
        .gap(u(4.))
        .overflow_hidden()
        .whitespace_nowrap()
        .rounded(u(4.))
        .border_1()
        .border_color(white().opacity(0.12))
        .bg(black().opacity(0.5))
        .text_color(white().opacity(0.92))
        .px(u(6.))
        .py(u(1.))
        .when_some(media, |d, name| {
            d.child(ui::icon("film").size(u(12.)).flex_shrink_0()).child(label(name))
        })
        .when(item.volume == 0., |d| d.child(ui::icon("volume-x").size(u(12.)).flex_shrink_0()))
        .when_some(framing, |d, f| d.child(tag(f)))
        .when(item.speed != 1., |d| {
            d.child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(u(2.))
                    .rounded(u(4.))
                    .bg(t.tint(0.2))
                    .px(u(4.))
                    .child(ui::icon("gauge").size(u(12.)))
                    .child(format!("{}×", item.speed)),
            )
        })
        .child(div().flex_shrink_0().child(format!("{}s", to_fixed(timeline::item_length(item) as f64 / 1000., 1))))
}

/// Transitions: a dip to black across each cut, a dissolve over the overlap.
fn transitions(g: Geo, height: f64, project: &Project, t: &Tokens) -> Vec<AnyElement> {
    timeline::placements(&project.video)
        .into_iter()
        .filter(|p| p.item.transition_ms > 0)
        .map(|p| {
            let dissolve = p.item.dissolve && p.index > 0;
            let width = g.len(p.item.transition_ms as f64).max(8.);
            let left = view::x_at(p.start as f64, g.zoom) - if dissolve { 0. } else { width / 2. };
            let mark = div()
                .absolute()
                .left(g.px(left))
                .top(g.px(height - 4. - 12.))
                .w(g.px(width))
                .h(u(12.))
                .overflow_hidden()
                .rounded(u(2.))
                .border_1()
                .border_color(t.tint(0.4));
            let mark = if dissolve {
                mark.bg(linear_gradient(90., linear_color_stop(t.accent.opacity(0.8), 0.), linear_color_stop(t.tint(0.8), 1.)))
            } else {
                let half = || div().h_full().w_1_2();
                mark.flex()
                    .child(half().bg(linear_gradient(90., linear_color_stop(transparent_black(), 0.), linear_color_stop(black(), 1.))))
                    .child(half().bg(linear_gradient(90., linear_color_stop(black(), 0.), linear_color_stop(transparent_black(), 1.))))
            };
            mark.into_any_element()
        })
        .collect()
}

/// The stretch after the end of the video, hatched.
fn hatch(g: Geo, total: f64, extent: f64, height: f64, t: &Tokens) -> impl IntoElement {
    let stripe = hsla(0., 0., 127. / 255., 0.18);
    div()
        .absolute()
        .left(g.x(total))
        .top(u(view::RULER_H as f32))
        .w(g.px(g.len(extent - total)))
        .h(g.px(height - view::RULER_H))
        .bg(t.bg.opacity(0.6))
        .child(
            canvas(|_, _, _| {}, move |bounds, _, window, _| {
                let mask = window.content_mask().bounds;
                let s = g.s as f64;
                let w = f(bounds.size.width) / s;
                let h = f(bounds.size.height) / s;
                let from = f(mask.left() - bounds.left()) / s;
                let to = f(mask.right() - bounds.left()) / s;
                let mut path = PathBuilder::fill();
                for band in view::hatch_bands(w, h, from, to) {
                    let corners = band.map(|(x, y)| point(bounds.left() + px((x * s) as f32), bounds.top() + px((y * s) as f32)));
                    path.add_polygon(&corners, true);
                }
                if let Ok(path) = path.build() {
                    window.with_content_mask(Some(ContentMask { bounds }), |window| window.paint_path(path, stripe));
                }
            })
            .size_full(),
        )
}

fn paint_diamond(centre: Point<Pixels>, half: f32, color: Hsla, window: &mut Window) {
    let mut path = PathBuilder::fill();
    let (x, y) = (centre.x, centre.y);
    path.add_polygon(
        &[point(x, y - px(half)), point(x + px(half), y), point(x, y + px(half)), point(x - px(half), y)],
        true,
    );
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// Frames across the box (Filmstrip.tsx): tile i shows second `seconds[i]`, the
/// window of its frame at `crop_x`; tiles still loading are dark.
fn filmstrip(tiles: view::Tiles, shown: std::ops::Range<usize>, frames: HashMap<u32, Frames>, crop_x: f64, s: f32) -> Canvas<()> {
    canvas(|_, _, _| {}, move |bounds, _, window, _| {
        let s = s as f64;
        let (tile, frame_width) = (tiles.tile * s, tiles.frame_width * s);
        let height = bounds.size.height;
        for i in shown {
            let x = f(bounds.left()) + i as f64 * tile;
            let cell = Bounds::new(point(px(x as f32), bounds.top()), size(px(tile as f32), height));
            let (block, frame) = view::block_of(tiles.seconds[i]);
            match frames.get(&block).and_then(|f| f.get(frame)) {
                Some(image) => {
                    // The frame's slot in the strip, then the tile's window within the frame.
                    let left = x - (frame_width - tile) * crop_x;
                    let whole = Bounds::new(point(px(left as f32), bounds.top()), size(px(frame_width as f32), height));
                    let _ = window.paint_image(cell, whole, Corners::default(), image.clone(), 0, false);
                }
                None => window.paint_quad(fill(cell, black().opacity(0.3))),
            }
        }
    })
}

/// An added image across the box, repeated at its own shape.
fn still_strip(image: Option<std::sync::Arc<RenderImage>>) -> Canvas<()> {
    canvas(|_, _, _| {}, move |bounds, _, window, _| {
        let Some(image) = image else { return };
        let size_px = image.size(0);
        let height = bounds.size.height;
        let tile = f(height) * size_px.width.0 as f64 / (size_px.height.0.max(1)) as f64;
        if tile < 1. {
            return;
        }
        let mask = window.content_mask().bounds;
        let first = ((f(mask.left() - bounds.left()) / tile).floor().max(0.)) as usize;
        let last = ((f(mask.right().min(bounds.right()) - bounds.left()) / tile).ceil().max(0.)) as usize;
        for i in first..last {
            let x = f(bounds.left()) + i as f64 * tile;
            let whole = Bounds::new(point(px(x as f32), bounds.top()), size(px(tile as f32), height));
            let _ = window.paint_image(bounds, whole, Corners::default(), image.clone(), 0, false);
        }
    })
}

/// Mirrored loudness bars for [from, to) ms of a file, stretched to the box (Waveform.tsx).
fn waveform(peaks: Option<media::Peaks>, from: i64, to: i64, color: Hsla) -> Canvas<()> {
    canvas(|_, _, _| {}, move |bounds, _, window, _| {
        let Some(peaks) = peaks else { return };
        let range = view::peak_range(peaks.len(), from, to);
        if range.is_empty() {
            return;
        }
        let mask = window.content_mask().bounds;
        let bars = view::waveform_bars(
            &peaks[range],
            f(bounds.size.width),
            f(bounds.size.height),
            f(mask.left() - bounds.left()),
            f(mask.right() - bounds.left()),
        );
        // A path's index buffer is 16-bit: a few thousand bars a path.
        for chunk in bars.chunks(4000) {
            let mut path = PathBuilder::fill();
            for b in chunk {
                let at = |x: f64, y: f64| point(bounds.left() + px(x as f32), bounds.top() + px(y as f32));
                path.add_polygon(&[at(b.x0, b.y0), at(b.x1, b.y0), at(b.x1, b.y1), at(b.x0, b.y1)], true);
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        }
    })
}
