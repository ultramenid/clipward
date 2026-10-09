//! The editor's preview (Player.tsx): a hybrid of the exact render and an instant
//! approximation.
//!
//! The engine renders the edit in CHUNK_MS chunks through the export's own graph,
//! so a chunk is exactly what the export will contain. Until the chunk under the
//! playhead is ready, and while dragging, the source video plays underneath as a
//! quick approximation, marked "Preview…": a plain crop with the text, overlays,
//! dips and sounds drawn or played here (no captions, ducking or dissolves).
//!
//! Two exact players alternate: chunk i plays in slot i % 2 while the other loads
//! chunk i + 1, so crossing a boundary swaps to a player that already has its
//! frame. Once every chunk exists they are joined and that one file plays instead,
//! since switching players at each chunk's end stutters.
use super::state::{EditorState, Playhead, Selection};
use crate::captions;
use crate::state;
use crate::theme::ActiveTokens;
use crate::ui::{self, TypeScale, u};
use crate::video::MediaPlayer;
use clipward_core::clip_engine::render::project::{PIP_WIDTH_RANGE, Project, TEXT_SIZE_RANGE, VideoItem};
use clipward_core::overlays::OverlayKey;
use clipward_core::preview::{CHUNK_MS, SUPERSEDED};
use clipward_core::timeline::{self, Placement};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Wait after an edit before asking the engine, so a burst of edits renders once.
const RENDER_DELAY: Duration = Duration::from_millis(200);
/// Wait before rendering the rest of the edit in the background (edits come in bursts).
const PREFETCH_DELAY: Duration = Duration::from_millis(600);
const CHUNK: f64 = CHUNK_MS as f64;

/// What a pointer drag on the preview moves or resizes.
#[derive(Clone, Debug)]
enum Target {
    Text(String),
    Overlay(OverlayKey),
    Crop(usize),
    Pip(String),
}

#[derive(Clone, Debug)]
struct Drag {
    target: Target,
    /// Corner resize about the item's centre (window px) from `base`; else a move
    /// from (x, y) in canvas fractions.
    resize: Option<(Point<Pixels>, f32, f64)>,
    x: f64,
    y: f64,
    start: Point<Pixels>,
    moved: bool,
}

/// The inputs the render scheduling last ran with (the React effects' dependencies).
#[derive(Clone, PartialEq, Default)]
struct Plan {
    revision: u64,
    index: usize,
    playing: bool,
    scrubbing: bool,
    whole: bool,
    chunks: usize,
    pending: usize,
    error: bool,
    warming: bool,
}

pub struct Player {
    editor: Entity<EditorState>,
    playhead: Entity<Playhead>,
    slots: [MediaPlayer; 2],
    approx: MediaPlayer,
    sounds: HashMap<String, MediaPlayer>,
    pips: HashMap<String, MediaPlayer>,
    /// The project as JSON, recomputed when its revision changes (chunks follow edits).
    key: String,
    key_revision: Option<u64>,
    /// Numbers this edit for the engine, which drops renders of older ones.
    generation: u64,
    paths: HashMap<usize, PathBuf>,
    joined: Option<PathBuf>,
    joining: bool,
    /// Chunk files that won't play fall back to the instant preview.
    broken: HashSet<PathBuf>,
    pending: HashSet<usize>,
    render_error: Option<String>,
    request_timer: Option<Task<()>>,
    prefetch_timer: Option<Task<()>>,
    plan: Plan,
    /// The chunk index the waiting slot was last rewound for.
    rewound: Option<usize>,
    last_tick: Option<Instant>,
    /// The player's area and the fitted frame in it, window px, from the last paint.
    area: Rc<Cell<Bounds<Pixels>>>,
    frame: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,
    _subscriptions: Vec<Subscription>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Player {
    pub fn new(editor: Entity<EditorState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let playhead = editor.read(cx).playhead.clone();
        let subscriptions = vec![
            cx.observe(&editor, |_, _, cx| cx.notify()),
            cx.observe(&playhead, |_, _, cx| cx.notify()),
        ];
        let mut slots = [MediaPlayer::new(), MediaPlayer::new()];
        slots.iter_mut().for_each(|s| s.set_muted(true));
        Self {
            editor,
            playhead,
            slots,
            approx: MediaPlayer::new(),
            sounds: HashMap::new(),
            pips: HashMap::new(),
            key: String::new(),
            key_revision: None,
            generation: now_ms(),
            paths: HashMap::new(),
            joined: None,
            joining: false,
            broken: HashSet::new(),
            pending: HashSet::new(),
            render_error: None,
            request_timer: None,
            prefetch_timer: None,
            plan: Plan::default(),
            rewound: None,
            last_tick: None,
            area: Rc::new(Cell::new(Bounds::default())),
            frame: Rc::new(Cell::new(Bounds::default())),
            drag: None,
            _subscriptions: subscriptions,
        }
    }

    /// The edit changed: chunks of the old one no longer apply.
    fn sync_key(&mut self, project: &Project, revision: u64) {
        if self.key_revision == Some(revision) {
            return;
        }
        self.key_revision = Some(revision);
        let key = serde_json::to_string(project).unwrap_or_default();
        if key != self.key {
            self.key = key;
            self.generation = now_ms();
            self.paths.clear();
            self.joined = None;
            self.joining = false;
            self.pending.clear();
            self.request_timer = None;
            self.prefetch_timer = None;
        }
    }

    fn usable(&self, i: usize, whole: bool) -> Option<PathBuf> {
        if whole {
            return (i == 0).then(|| self.joined.clone()).flatten();
        }
        self.paths.get(&i).filter(|p| !self.broken.contains(*p)).cloned()
    }

    /// Ask the engine for chunk `i` of the current edit.
    fn request(&mut self, i: usize, cx: &mut Context<Self>) {
        if !self.pending.insert(i) {
            return;
        }
        let (dir, project) = {
            let editor = self.editor.read(cx);
            (editor.output_dir.clone(), editor.project().cloned())
        };
        let Some(project) = project else { return };
        let core = state::core(cx);
        let (key, generation) = (self.key.clone(), self.generation);
        let render = state::run(cx, async move {
            core.project_preview_chunk(&dir, &project, i as u32, generation).await
        });
        cx.spawn(async move |this, cx| {
            let result = render.await;
            let _ = this.update(cx, |this, cx| {
                if this.key != key {
                    return;
                }
                this.pending.remove(&i);
                match result {
                    Ok(path) => {
                        this.render_error = None;
                        this.paths.insert(i, path);
                    }
                    Err(message) if message.contains(SUPERSEDED) => {}
                    Err(message) => this.render_error = Some(message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Every chunk is there: join them into the one file played from then on.
    fn join(&mut self, cx: &mut Context<Self>) {
        self.joining = true;
        let (dir, project) = {
            let editor = self.editor.read(cx);
            (editor.output_dir.clone(), editor.project().cloned())
        };
        let Some(project) = project else { return };
        let core = state::core(cx);
        let (key, generation) = (self.key.clone(), self.generation);
        let join = state::run(cx, async move { core.project_preview_joined(&dir, &project, generation).await });
        cx.spawn(async move |this, cx| {
            let result = join.await;
            let _ = this.update(cx, |this, cx| {
                if this.key != key {
                    return;
                }
                this.joining = false;
                match result {
                    Ok(path) => this.joined = Some(path),
                    Err(message) if message.contains(SUPERSEDED) => {}
                    Err(message) => this.render_error = Some(message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The render scheduling of Player.tsx's effects, re-run when their inputs change.
    fn schedule(&mut self, plan: Plan, total: f64, cx: &mut Context<Self>) {
        if plan == self.plan {
            return;
        }
        let previous = std::mem::replace(&mut self.plan, plan.clone());
        let count = (total / CHUNK).ceil() as usize;
        let exists = |i: usize| (i as f64) * CHUNK < total;

        // The chunk under the playhead, and the next one while playing.
        let near_changed = (previous.revision, previous.index, previous.playing, previous.scrubbing, previous.whole)
            != (plan.revision, plan.index, plan.playing, plan.scrubbing, plan.whole);
        if near_changed {
            self.request_timer = None;
            if !plan.scrubbing && !plan.whole {
                let mut wanted = vec![plan.index];
                if plan.playing {
                    wanted.push(plan.index + 1);
                }
                wanted.retain(|&i| exists(i));
                self.request_timer = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(RENDER_DELAY).await;
                    let _ = this.update(cx, |this, cx| wanted.into_iter().for_each(|i| this.request(i, cx)));
                }));
            }
        }

        // Then the rest of the edit, from the playhead on, one at a time while nothing
        // else renders, so the engine's other slot stays free for a seek.
        self.prefetch_timer = None;
        if !plan.scrubbing && !plan.error && !plan.whole && plan.pending == 0 && count > 0 {
            let next = (0..count).map(|k| (plan.index + k) % count).find(|i| !self.paths.contains_key(i));
            if let Some(next) = next {
                // The pause only right after an edit: once one chunk is done the rest follow.
                let delay = if self.paths.is_empty() { PREFETCH_DELAY } else { Duration::ZERO };
                self.prefetch_timer = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update(cx, |this, cx| this.request(next, cx));
                }));
            }
        }

        // Opening: the loading screen waits for every chunk, so ask for all at once.
        if plan.warming && (!previous.warming || previous.revision != plan.revision) {
            (0..count).for_each(|i| self.request(i, cx));
        }

        if self.joined.is_none() && !self.joining && count > 0 && self.paths.len() >= count {
            self.join(cx);
        }
    }

    /// Keep every player on the playhead (Player.tsx's per-render sync).
    #[allow(clippy::too_many_arguments)]
    fn sync_players(
        &mut self,
        project: &Project,
        output_dir: &str,
        source: Option<&PathBuf>,
        time: f64,
        playing: bool,
        exact: bool,
        exact_path: Option<&PathBuf>,
        slot_paths: &[Option<PathBuf>; 2],
        active: usize,
        index: usize,
        span: f64,
        muted: bool,
        still: bool,
    ) {
        let speed = project.speed;
        // Slots: the active one follows the playhead (silent until it takes over); the
        // other waits at its start.
        for (s, slot) in self.slots.iter_mut().enumerate() {
            slot.open(slot_paths[s].as_deref(), true);
            if slot.failed() {
                if let Some(path) = slot.path().map(PathBuf::from) {
                    self.broken.insert(path);
                }
            }
        }
        let exact_slot = &mut self.slots[active];
        exact_slot.set_muted(muted || !exact);
        if exact_path.is_some() {
            exact_slot.follow((time - index as f64 * span) / speed, playing, speed as f32, 250. * speed);
        } else {
            exact_slot.set_playing(false, 1.);
        }
        let next = &mut self.slots[1 - active];
        next.set_playing(false, 1.);
        if self.rewound != Some(index) && next.ready() {
            self.rewound = Some(index);
            if next.current_ms() != 0. {
                next.seek(0., true);
            }
        }

        // The instant preview: the piece's own file (the source, or a video the user
        // added; an image shows as a still meanwhile).
        let placement = timeline::placement_at(&project.video, time);
        let approx_path = match placement.as_ref().and_then(|p| p.item.media.as_ref()) {
            Some(media) if !still => Some(PathBuf::from(output_dir).join(media)),
            _ => source.cloned(),
        };
        self.approx.open(approx_path.as_deref(), true);
        if !exact && !still {
            let piece_speed = placement.as_ref().map_or(1., |p| p.item.speed);
            let volume = placement.as_ref().map_or(1., |p| p.item.volume);
            self.approx.set_volume(volume.min(1.) as f32);
            self.approx.set_muted(muted || volume == 0.);
            let at = timeline::source_time_at(&project.video, time).unwrap_or(0) as f64;
            self.approx.follow(at, playing, (speed * piece_speed) as f32, 250. * piece_speed);
        } else {
            self.approx.set_playing(false, 1.);
        }

        // Sounds on the instant preview, each from its own player.
        // ponytail: no ducking here; the exact preview a moment later has it.
        let total = timeline::total_length(&project.video) as f64;
        let mut live = HashSet::new();
        for item in &project.audio {
            let path = match &item.file {
                Some(file) => PathBuf::from(output_dir).join(file),
                None => match source {
                    Some(source) => source.clone(),
                    None => continue,
                },
            };
            live.insert(item.id.clone());
            let player = self.sounds.entry(item.id.clone()).or_insert_with(MediaPlayer::new);
            player.open(Some(&path), false);
            let into = time - item.start_ms as f64;
            let len = timeline::sound_length(item) as f64;
            if exact || muted || into < 0. || into >= len || time >= total {
                player.set_playing(false, 1.);
                continue;
            }
            let fade_in = if item.fade_in_ms > 0 { (into / item.fade_in_ms as f64).min(1.) } else { 1. };
            let fade_out = if item.fade_out_ms > 0 { ((len - into) / item.fade_out_ms as f64).min(1.) } else { 1. };
            player.set_volume((item.volume * fade_in * fade_out).min(1.) as f32);
            player.set_muted(false);
            let at = item.src_in_ms as f64 + into * item.speed;
            let drift = if playing { 250. } else { 50. } * item.speed;
            player.follow(at, playing, (item.speed * speed) as f32, drift);
        }
        self.sounds.retain(|id, _| live.contains(id));

        // Picture-in-picture videos, following the playhead; the exact preview has them.
        let mut live = HashSet::new();
        for pip in &project.pip {
            let Some(file) = project.media.iter().find(|m| m.file == pip.media) else { continue };
            if file.image {
                continue;
            }
            let showing = time >= pip.start_ms as f64 && time < (pip.start_ms + pip.src_out_ms - pip.src_in_ms) as f64;
            if !showing {
                continue;
            }
            live.insert(pip.id.clone());
            let player = self.pips.entry(pip.id.clone()).or_insert_with(MediaPlayer::new);
            player.open(Some(&PathBuf::from(output_dir).join(&pip.media)), true);
            if exact {
                player.set_playing(false, 1.);
                continue;
            }
            player.set_muted(muted);
            player.set_volume(pip.volume.min(1.) as f32);
            let at = (pip.src_in_ms as f64) + time - pip.start_ms as f64;
            player.follow(at, playing, speed as f32, if playing { 250. } else { 10. });
        }
        self.pips.retain(|id, _| live.contains(id));
    }

    /// While playing, the visible player's clock drives the playhead.
    #[allow(clippy::too_many_arguments)]
    fn advance(
        &mut self,
        time: f64,
        exact: bool,
        still: bool,
        index: usize,
        span: f64,
        placement: Option<&Placement>,
        total: f64,
        speed: f64,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        let elapsed = self.last_tick.map_or(0., |at| now.duration_since(at).as_secs_f64() * 1000.);
        self.last_tick = Some(now);
        let next = if !exact && still {
            let Some(p) = placement else { return };
            (time + elapsed * speed).min(p.end as f64)
        } else if exact {
            let player = &self.slots[index % 2];
            if !player.ready() || !player.has_frame() {
                return;
            }
            if player.ended() {
                ((index + 1) as f64 * span).min(total)
            } else {
                index as f64 * span + player.current_ms() * speed
            }
        } else {
            let Some(p) = placement else { return };
            if !self.approx.ready() || self.approx.is_seeking() {
                return;
            }
            (p.start as f64 + (self.approx.current_ms() - p.item.src_in_ms as f64) / p.item.speed).min(p.end as f64)
        };
        // After the draw: a notify made while the window draws reaches no observers,
        // so the playhead line and the clock would stay put.
        if next >= total {
            let editor = self.editor.clone();
            cx.defer(move |cx| {
                editor.update(cx, |e, cx| {
                    e.seek(total, cx);
                    e.set_playing(false, cx);
                })
            });
        } else if next > time {
            let playhead = self.playhead.clone();
            cx.defer(move |cx| {
                playhead.update(cx, |p, cx| {
                    // Unless a seek or pause landed first.
                    if p.playing && p.time == time {
                        p.time = next;
                        cx.notify();
                    }
                })
            });
        }
    }

    // ---- drags --------------------------------------------------------------------

    fn start_drag(&mut self, target: Target, x: f64, y: f64, at: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.editor.read(cx).editable() {
            return;
        }
        self.drag = Some(Drag { target, resize: None, x, y, start: at, moved: false });
        cx.notify();
    }

    /// A corner handle: resizes the item centred at (x, y) (canvas fractions) from `base`.
    fn start_resize(&mut self, target: Target, x: f64, y: f64, base: f64, at: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.editor.read(cx).editable() {
            return;
        }
        let frame = self.frame.get();
        let centre = point(
            frame.origin.x + frame.size.width * x as f32,
            frame.origin.y + frame.size.height * y as f32,
        );
        let dist = ((at - centre).magnitude() as f32).max(4.);
        self.drag = Some(Drag { target, resize: Some((centre, dist, base)), x, y, start: at, moved: false });
        cx.notify();
    }

    fn drag_to(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(mut drag) = self.drag.clone() else { return };
        let frame = self.frame.get();
        let (dx, dy) = (f32::from(at.x - drag.start.x), f32::from(at.y - drag.start.y));
        if !drag.moved {
            if dx.hypot(dy) < 3. {
                return;
            }
            drag.moved = true;
            self.drag = Some(drag.clone());
            self.editor.update(cx, |e, cx| {
                e.checkpoint(cx);
                e.set_scrubbing(true, cx);
            });
        }
        let editor = self.editor.clone();
        if let Some((centre, dist, base)) = drag.resize {
            let factor = (at - centre).magnitude() / dist as f64;
            editor.update(cx, |e, cx| match &drag.target {
                Target::Overlay(key) => e.resize_overlay(*key, factor, base, cx),
                Target::Pip(id) => {
                    let width = timeline::js_round((base * factor).clamp(PIP_WIDTH_RANGE.0, PIP_WIDTH_RANGE.1) * 1000.) / 1000.;
                    e.edit_pip(|pip| pip.iter().cloned().map(|mut p| { if &p.id == id { p.width = width; } p }).collect(), false, cx);
                }
                Target::Text(id) => {
                    let size = (timeline::js_round(base * factor) as i64).clamp(TEXT_SIZE_RANGE.0, TEXT_SIZE_RANGE.1);
                    e.edit_texts(|texts| texts.iter().cloned().map(|mut t| { if &t.id == id { t.size = size; } t }).collect(), false, cx);
                }
                Target::Crop(_) => {}
            });
            return;
        }
        let (w, h) = (f32::from(frame.size.width).max(1.), f32::from(frame.size.height).max(1.));
        let clamp = |v: f64| timeline::js_round(v.clamp(0., 1.) * 1000.) / 1000.;
        let time = self.playhead.read(cx).time;
        editor.update(cx, |e, cx| match &drag.target {
            Target::Crop(index) => {
                // The picture follows the pointer: the window moves the other way, in source units.
                let Some(project) = e.project() else { return };
                let Some(p) = timeline::placements(&project.video).into_iter().nth(*index) else { return };
                let item = p.item.clone();
                let local = time - p.start as f64;
                let crop = timeline::crop_at(&item, local);
                let window = crop_rect(project, &item, crop);
                let x = clamp(drag.x - (dx / w) as f64 * window.w);
                let y = clamp(drag.y - (dy / h) as f64 * window.h);
                let next = clipward_core::clip_engine::render::project::Crop { x, y, zoom: crop.zoom };
                let index = *index;
                e.edit_video(
                    |video| {
                        let mut video = video.to_vec();
                        if let Some(piece) = video.get_mut(index) {
                            if piece.keyframes.is_empty() {
                                piece.crop = Some(next);
                            } else {
                                piece.keyframes = timeline::set_crop_key(&piece.keyframes, local, next);
                            }
                        }
                        video
                    },
                    false,
                    cx,
                );
            }
            Target::Overlay(key) => {
                let (x, y) = (clamp(drag.x + (dx / w) as f64), clamp(drag.y + (dy / h) as f64));
                e.place_overlay(*key, |b| { b.x = x; b.y = y; }, false, cx);
            }
            Target::Pip(id) => {
                let (x, y) = (clamp(drag.x + (dx / w) as f64), clamp(drag.y + (dy / h) as f64));
                e.edit_pip(|pip| pip.iter().cloned().map(|mut p| { if &p.id == id { p.x = x; p.y = y; } p }).collect(), false, cx);
            }
            Target::Text(id) => {
                let (x, y) = (clamp(drag.x + (dx / w) as f64), clamp(drag.y + (dy / h) as f64));
                e.edit_texts(|texts| texts.iter().cloned().map(|mut t| { if &t.id == id { t.x = x; t.y = y; } t }).collect(), false, cx);
            }
        });
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.take() {
            if drag.moved {
                self.editor.update(cx, |e, cx| e.set_scrubbing(false, cx));
            }
            cx.notify();
        }
    }
}

/// The crop window of a piece, as fractions of its picture.
fn crop_rect(project: &Project, item: &VideoItem, crop: clipward_core::clip_engine::render::project::Crop) -> timeline::Rect {
    let read = item
        .media
        .as_ref()
        .and_then(|m| project.media.iter().find(|f| &f.file == m))
        .map_or((project.source_width as f64, project.source_height as f64), |f| (f.width as f64, f.height as f64));
    timeline::crop_window(read, (project.width as f64, project.height as f64), crop)
}

/// The largest `aspect` box inside `area`, centred.
fn fit(area: Bounds<Pixels>, aspect: f32) -> Bounds<Pixels> {
    let (w, h) = (f32::from(area.size.width), f32::from(area.size.height));
    let (fw, fh) = if w / h.max(1.) > aspect { (h * aspect, h) } else { (w, w / aspect) };
    Bounds {
        origin: point(area.origin.x + px((w - fw) / 2.), area.origin.y + px((h - fh) / 2.)),
        size: size(px(fw), px(fh)),
    }
}

/// A centred anchor at (x, y) (canvas fractions): content is laid out centred on it.
fn anchored(x: f64, y: f64, frame: Size<Pixels>) -> Div {
    let (w, h) = (f32::from(frame.width), f32::from(frame.height));
    div()
        .absolute()
        .left(px(x as f32 * w - w))
        .top(px(y as f32 * h - h))
        .w(px(w * 2.))
        .h(px(h * 2.))
        .flex()
        .items_center()
        .justify_center()
}

/// The dashed (here: solid hairline) outline of the selected or hovered item.
fn outline(el: Stateful<Div>, selected: bool) -> Stateful<Div> {
    if selected {
        el.border_1().border_color(white().opacity(0.8))
    } else {
        el.border_1().border_color(transparent_black()).hover(|s| s.border_color(white().opacity(0.4)))
    }
}

impl Render for Player {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let editor = self.editor.read(cx);
        let Some(project) = editor.project().cloned() else {
            return div().size_full().into_any_element();
        };
        let revision = editor.history.revision();
        let output_dir = editor.output_dir.clone();
        let source = editor.opened.as_ref().and_then(|o| o.source_path.clone());
        let editable = editor.editable();
        let selection = editor.selection.clone();
        let overlays = editor.overlays.clone();
        let warming = editor.warmup.is_some();
        let muted = editor.recording.is_some();
        let (time, playing, scrubbing) = {
            let p = self.playhead.read(cx);
            (p.shown(), p.playing, p.scrubbing || (!p.playing && p.hover.is_some()))
        };
        self.sync_key(&project, revision);

        let total = timeline::total_length(&project.video) as f64;
        let count = (total / CHUNK).ceil() as usize;
        let whole = self.joined.as_ref().is_some_and(|p| !self.broken.contains(p));
        let span = if whole { total.max(1.) } else { CHUNK };
        let index = ((time / span).floor() as usize).min(((total / span).ceil() as usize).max(1) - 1);
        let active = index % 2;
        let slot_paths = [0, 1].map(|s| self.usable(if s == active { index } else { index + 1 }, whole));
        let exact_path = if scrubbing { None } else { slot_paths[active].clone() };
        let exact = exact_path.is_some()
            && self.slots[active].path() == exact_path.as_deref()
            && self.slots[active].has_frame();

        let plan = Plan {
            revision,
            index,
            playing,
            scrubbing,
            whole,
            chunks: self.paths.len(),
            pending: self.pending.len(),
            error: self.render_error.is_some(),
            warming,
        };
        self.schedule(plan, total, cx);
        if warming {
            let done = if self.render_error.is_some() { count + 1 } else { self.paths.len() + usize::from(whole) };
            let total_steps = count + 1;
            // After the draw, as `advance` does, so the loading screen hears of it.
            let editor = self.editor.clone();
            cx.defer(move |cx| {
                editor.update(cx, |e, cx| {
                    let next = (done < total_steps).then_some((done, total_steps));
                    if e.warmup.is_some() && e.warmup != next {
                        e.warmup = next;
                        cx.notify();
                    }
                })
            });
        }

        let placement = timeline::placement_at(&project.video, time);
        let piece_media = placement.as_ref().and_then(|p| p.item.media.as_ref()).and_then(|m| project.media.iter().find(|f| &f.file == m));
        // An image piece: drawn as a still, its time driven by the wall clock.
        let still = piece_media.is_some_and(|m| m.image);
        self.sync_players(&project, &output_dir, source.as_ref(), time, playing, exact, exact_path.as_ref(), &slot_paths, active, index, span, muted, still);

        let mut changed = false;
        for player in self.slots.iter_mut().chain([&mut self.approx]).chain(self.pips.values_mut()) {
            changed |= player.poll_frame(window);
        }
        if playing {
            self.advance(time, exact, still, index, span, placement.as_ref(), total, project.speed, cx);
        } else {
            self.last_tick = None;
        }
        // Keep frames coming while something moves or loads.
        let busy = playing
            || changed
            || self.slots.iter().chain([&self.approx]).any(|p| p.path().is_some() && (!p.ready() || p.is_seeking()));
        if busy {
            window.request_animation_frame();
        }

        // ---- the frame -------------------------------------------------------------
        let frame = self.frame.get();
        let fsize = frame.size;
        let scale = f32::from(fsize.width) / project.width.max(1) as f32;
        let canvas_px = |v: f64| px(v as f32 * scale);
        let vertical = project.height > project.width;
        let crop_piece = placement.as_ref().filter(|p| p.item.framing == "crop");
        let crop = crop_piece.map(|p| {
            let now = timeline::crop_at(p.item, time - p.start as f64);
            (now, crop_rect(&project, p.item, now))
        });
        let cover = vertical && placement.as_ref().is_some_and(|p| p.item.framing == "auto") && piece_media.is_none();
        let dark = timeline::dip_at(&project.video, time);
        let look = placement.as_ref().and_then(|p| p.item.color);
        let entity = cx.entity();

        let picture = |el: Div| -> Div {
            match crop {
                // A crop piece: the picture scaled so its window fills the frame.
                Some((_, r)) => el
                    .absolute()
                    .w(px(f32::from(fsize.width) / r.w as f32))
                    .h(px(f32::from(fsize.height) / r.h as f32))
                    .left(px(-(r.x / r.w) as f32 * f32::from(fsize.width)))
                    .top(px(-(r.y / r.h) as f32 * f32::from(fsize.height))),
                None => el.absolute().size_full(),
            }
        };
        let fit_mode = if crop.is_some() { ObjectFit::Fill } else if cover { ObjectFit::Cover } else { ObjectFit::Contain };

        let mut layers: Vec<AnyElement> = Vec::new();
        if !exact {
            if still {
                if let Some(media) = placement.as_ref().and_then(|p| p.item.media.as_ref()) {
                    layers.push(picture(div()).child(img(PathBuf::from(&output_dir).join(media)).size_full().object_fit(fit_mode)).into_any_element());
                }
            } else if let Some(surface) = self.approx.surface(fit_mode) {
                layers.push(picture(div()).child(surface.size_full()).into_any_element());
            }
            // The look's warmth as a tint (GPUI can't filter a video surface; the exact
            // preview, a moment later, has the real colour).
            if let Some(color) = look {
                let warmth = (6500. - color.temperature) / 4500.;
                if warmth != 0. {
                    let tint = if warmth > 0. { rgb(0xff8c14) } else { rgb(0x2878ff) };
                    let alpha = (warmth.abs() * 0.3).min(0.35) as f32 * 0.5;
                    layers.push(div().absolute().size_full().bg(Hsla::from(tint).opacity(alpha)).into_any_element());
                }
            }
        }
        if exact {
            if let Some(surface) = self.slots[active].surface(ObjectFit::Contain) {
                layers.push(div().absolute().size_full().child(surface.size_full()).into_any_element());
            }
        }
        if !exact && dark > 0. {
            layers.push(div().absolute().size_full().bg(black().opacity(dark as f32)).into_any_element());
        }
        // Drag the picture to move the crop.
        if let (Some(p), Some((now, _)), true) = (crop_piece, crop, editable) {
            let index = p.index;
            layers.push(
                div()
                    .id("crop-drag")
                    .absolute()
                    .size_full()
                    .cursor_grab()
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        this.editor.update(cx, |ed, cx| ed.select(Some(Selection::Piece(index)), cx));
                        this.start_drag(Target::Crop(index), now.x, now.y, e.position, cx);
                    }))
                    .into_any_element(),
            );
        }
        // Picture-in-picture.
        for pip in &project.pip {
            let Some(file) = project.media.iter().find(|m| m.file == pip.media) else { continue };
            if time < pip.start_ms as f64 || time >= (pip.start_ms + pip.src_out_ms - pip.src_in_ms) as f64 {
                continue;
            }
            let w = canvas_px(pip.width * project.width as f64);
            let h = canvas_px(pip.width * project.width as f64 * file.height as f64 / file.width.max(1) as f64);
            let selected = selection == Some(Selection::Pip(pip.id.clone()));
            let content: AnyElement = if exact {
                div().into_any_element()
            } else if file.image {
                img(PathBuf::from(&output_dir).join(&pip.media)).size_full().object_fit(ObjectFit::Fill).into_any_element()
            } else {
                self.pips.get(&pip.id).and_then(|p| p.surface(ObjectFit::Fill)).map_or(div().into_any_element(), |s| s.size_full().into_any_element())
            };
            let (id, x, y, width) = (pip.id.clone(), pip.x, pip.y, pip.width);
            let id2 = id.clone();
            let el = outline(div().id(SharedString::from(format!("pip-{id}"))), selected)
                .relative()
                .w(w)
                .h(h)
                .cursor_move()
                .child(content)
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let id = id.clone();
                    this.editor.update(cx, |ed, cx| {
                        ed.select(Some(Selection::Pip(id.clone())), cx);
                        ed.set_playing(false, cx);
                    });
                    this.start_drag(Target::Pip(id), x, y, e.position, cx);
                }))
                .when(selected && editable, |el| {
                    el.child(resize_handle(cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.start_resize(Target::Pip(id2.clone()), x, y, width, e.position, cx);
                    })))
                });
            layers.push(anchored(x, y, fsize).child(el).into_any_element());
        }
        // Title, banner and logos.
        for o in overlays.iter().filter(|o| time >= o.box_.start_ms as f64 && time < o.box_.end_ms as f64) {
            let selected = selection == Some(Selection::Overlay(o.key));
            let (key, x, y) = (o.key, o.box_.x, o.box_.y);
            let base = match key.logo_index() {
                Some(i) => project.logos.get(i).map_or(0.15, |l| l.size),
                None => o.box_.scale,
            };
            let el = outline(div().id(SharedString::from(format!("overlay-{}", o.label))), selected)
                .relative()
                .w(canvas_px(o.width))
                .h(canvas_px(o.height))
                .cursor(if editable { CursorStyle::OpenHand } else { CursorStyle::PointingHand })
                .when(!exact, |el| el.child(img(o.path.clone()).size_full()))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    this.editor.update(cx, |ed, cx| {
                        ed.select(Some(Selection::Overlay(key)), cx);
                        ed.set_playing(false, cx);
                    });
                    this.start_drag(Target::Overlay(key), x, y, e.position, cx);
                }))
                .when(selected && editable, |el| {
                    el.child(resize_handle(cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.start_resize(Target::Overlay(key), x, y, base, e.position, cx);
                    })))
                });
            layers.push(anchored(x, y, fsize).child(el).into_any_element());
        }
        // Text layers: drawn here on the instant preview; on the exact one (which has
        // them burned in) only the box stays, invisible, as the handle to drag.
        for layer in project.texts.iter().filter(|l| time >= l.start_ms as f64 && time < l.end_ms as f64) {
            let motion = timeline::text_motion(layer, time);
            let mut word = 0usize;
            let lines: Vec<Vec<(String, bool)>> = layer
                .text
                .split('\n')
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|line| {
                    line.split_whitespace()
                        .map(|w| {
                            word += 1;
                            (w.to_string(), word <= motion.words)
                        })
                        .collect()
                })
                .collect();
            let font = canvas_px(layer.size as f64 * motion.scale);
            let unit = scale * (layer.size as f32 / 84.);
            let selected = selection == Some(Selection::Text(layer.id.clone()));
            let (id, x, y, base) = (layer.id.clone(), layer.x, layer.y, layer.size as f64);
            let id2 = id.clone();
            let drop = canvas_px(motion.drop * layer.size as f64);
            let el = outline(div().id(SharedString::from(format!("text-{id}"))), selected)
                .relative()
                .mt(drop * 2.)
                .cursor(if editable { CursorStyle::OpenHand } else { CursorStyle::PointingHand })
                .child(
                    captions::text_layer(&layer.preset, &lines, font, unit)
                        .opacity(if exact { 0. } else { motion.opacity as f32 }),
                )
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let id = id.clone();
                    this.editor.update(cx, |ed, cx| {
                        ed.select(Some(Selection::Text(id.clone())), cx);
                        ed.set_playing(false, cx);
                    });
                    this.start_drag(Target::Text(id), x, y, e.position, cx);
                }))
                .when(selected && editable, |el| {
                    el.child(resize_handle(cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.start_resize(Target::Text(id2.clone()), x, y, base, e.position, cx);
                    })))
                });
            layers.push(anchored(x, y, fsize).child(el).into_any_element());
        }
        if !exact {
            layers.push(
                div()
                    .absolute()
                    .left(u(8.))
                    .top(u(8.))
                    .flex()
                    .items_center()
                    .gap(u(4.))
                    .px(u(6.))
                    .rounded(u(4.))
                    .bg(black().opacity(0.6))
                    .text_meta()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(white().opacity(0.9))
                    .when(!scrubbing && self.render_error.is_none(), |d| d.child(ui::spinner(12., white().opacity(0.9))))
                    .child("Preview…")
                    .into_any_element(),
            );
        }
        if let (Some(error), false) = (&self.render_error, exact) {
            layers.push(
                div()
                    .absolute()
                    .left(u(8.))
                    .right(u(8.))
                    .bottom(u(8.))
                    .rounded(u(4.))
                    .bg(black().opacity(0.7))
                    .px(u(8.))
                    .py(u(4.))
                    .text_caption()
                    .text_color(t.danger)
                    .child(error.clone())
                    .into_any_element(),
            );
        }

        let (area_cell, frame_cell) = (self.area.clone(), self.frame.clone());
        let area = self.area.get();
        let aspect = project.width as f32 / project.height.max(1) as f32;
        let drag_entity = entity.clone();
        let dragging = self.drag.is_some();
        div()
            .size_full()
            .relative()
            .child(
                // Measures the area and fits the frame (a new size draws again), and
                // during a drag follows the pointer anywhere in the window.
                canvas(
                    move |bounds, window, _| {
                        if area_cell.replace(bounds) != bounds {
                            frame_cell.set(fit(bounds, aspect));
                            window.refresh();
                        }
                    },
                    move |_, _, window, _| {
                        if !dragging {
                            return;
                        }
                        let moving = drag_entity.clone();
                        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                moving.update(cx, |this, cx| this.drag_to(e.position, cx));
                            }
                        });
                        let ending = drag_entity.clone();
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                ending.update(cx, |this, cx| this.end_drag(cx));
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .id("frame")
                    .absolute()
                    .left(frame.origin.x - area.origin.x)
                    .top(frame.origin.y - area.origin.y)
                    .w(fsize.width)
                    .h(fsize.height)
                    .overflow_hidden()
                    .rounded(u(8.))
                    .bg(black())
                    .shadow_lg()
                    .children(layers),
            )
            .into_any_element()
    }
}

/// A corner square on the selected item: drag it to resize about the centre.
fn resize_handle(on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
    div()
        .id("resize")
        .absolute()
        .right(px(-8.))
        .bottom(px(-8.))
        .size(px(12.))
        .rounded(px(2.))
        .bg(white())
        .border_1()
        .border_color(black().opacity(0.6))
        .cursor(CursorStyle::ResizeUpLeftDownRight)
        .on_mouse_down(MouseButton::Left, on_down)
}
