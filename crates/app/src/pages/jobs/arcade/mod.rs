//! Wait-screen arcade (Arcade.tsx), after MonoCode's empty-session grid: a band
//! of games that play themselves and slide every 16 s; taking control opens the
//! current one full screen.
//!
//! One fixed-tick loop steps the board on show and redraws only this view (the
//! page draws it from the view cache). It sleeps until the next tick. While
//! nobody is playing it stops stepping, and only checks back four times a
//! second, when the band is scrolled out of view, the window is in the
//! background, or the system asks for reduced motion (as the React band did).
//! While someone plays, the display's frames step the board instead, so the
//! motion keeps time with the screen.
mod paint;

use crate::models::Jobs;
use crate::state;
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::u;
use clipward_core::jobs_page::arcade::{self, GAMES, Game, Mode};
use clipward_core::jobs_page::{cubic_bezier, stage_label};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use paint::Board;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

const SLIDE_HOLD: Duration = Duration::from_secs(16);
const SLIDE_MS: u64 = 700;

/// The games, and the grid each was last laid out on (cols, rows, full screen):
/// a new grid starts a fresh game, as the canvases' resize did.
struct Boards {
    games: Vec<Box<dyn Game>>,
    grids: Vec<Option<(i32, i32, bool)>>,
}

pub struct Arcade {
    /// The job whose progress the full-screen status line shows.
    job_id: String,
    window: AnyWindowHandle,
    boards: Rc<RefCell<Boards>>,
    /// The board on show, and the one it slid from.
    index: usize,
    from: usize,
    slides: usize,
    playing: bool,
    hovered: bool,
    /// Session best per game and mode.
    best: HashMap<String, u32>,
    mode: Mode,
    focus: FocusHandle,
    /// Where the board on show was painted, and its pitch, for the mouse.
    painted: Rc<Cell<Option<(Bounds<Pixels>, f32)>>>,
    /// Whether the board on show was inside the visible area when last painted.
    visible: Rc<Cell<bool>>,
    last_frame: Instant,
    backlog_ms: f64,
    last_slide: Instant,
}

impl Arcade {
    pub fn new(job_id: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        let games = GAMES
            .iter()
            .enumerate()
            .map(|(i, def)| (def.create)(seed.wrapping_add(i as u64 * 0x9E37_79B9)))
            .collect();
        let mode = arcade::load_mode(&state::core(cx).paths.user_data);
        // Ends once the arcade is dropped (the update fails).
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(wait) = this.update(cx, |this, cx| this.frame(cx)) else {
                    break;
                };
                cx.background_executor().timer(wait).await;
            }
        })
        .detach();
        Self {
            job_id,
            window: window.window_handle(),
            boards: Rc::new(RefCell::new(Boards { games, grids: vec![None; GAMES.len()] })),
            index: 0,
            from: 0,
            slides: 0,
            playing: false,
            hovered: false,
            best: HashMap::new(),
            mode,
            focus: cx.focus_handle(),
            painted: Rc::new(Cell::new(None)),
            visible: Rc::new(Cell::new(true)),
            last_frame: Instant::now(),
            backlog_ms: 0.0,
            last_slide: Instant::now(),
        }
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    fn tick_ms(&self) -> f64 {
        let scale = if self.playing { self.mode.tick_scale() } else { 1.0 };
        GAMES[self.index].tick_ms * scale
    }

    /// Step the board on show by whole ticks; returns how long to sleep.
    fn frame(&mut self, cx: &mut Context<Self>) -> Duration {
        let now = Instant::now();
        if !self.playing && !self.hovered && self.last_slide.elapsed() >= SLIDE_HOLD {
            self.show((self.index + 1) % GAMES.len(), cx);
        }
        // Rendering steps the board while someone plays (see `render`).
        if self.playing {
            return Duration::from_millis(250);
        }
        let parked = cx.reduce_motion() || !self.visible.get() || cx.active_window() != Some(self.window);
        if parked {
            self.last_frame = now;
            self.backlog_ms = 0.0;
            return Duration::from_millis(250);
        }
        if self.catch_up() {
            cx.notify();
        }
        Duration::from_secs_f64((self.tick_ms() - self.backlog_ms).max(1.0) / 1000.0)
    }

    /// Runs the ticks due since the last call; true when any ran.
    fn catch_up(&mut self) -> bool {
        let now = Instant::now();
        let tick = self.tick_ms();
        self.backlog_ms += (now - self.last_frame).as_secs_f64().min(0.25) * 1000.0;
        self.last_frame = now;
        if self.backlog_ms < tick {
            return false;
        }
        let mut boards = self.boards.borrow_mut();
        if boards.grids[self.index].is_none() {
            self.backlog_ms = 0.0;
            return true;
        }
        let game = &mut boards.games[self.index];
        while self.backlog_ms >= tick {
            game.step(self.playing);
            self.backlog_ms -= tick;
        }
        if self.playing {
            let key = self.best_key();
            let best = self.best.entry(key).or_default();
            *best = (*best).max(game.score());
        }
        true
    }

    fn best_key(&self) -> String {
        format!("{}:{}", GAMES[self.index].id, self.mode.as_str())
    }

    fn show(&mut self, index: usize, cx: &mut Context<Self>) {
        if index != self.index {
            self.from = self.index;
            self.index = index;
            self.slides += 1;
        }
        self.last_slide = Instant::now();
        if self.playing {
            self.boards.borrow_mut().games[index].reset();
        }
        cx.notify();
    }

    fn pick_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.boards.borrow_mut().games[self.index].reset();
        // A failed write only means the choice lasts this session.
        let _ = arcade::save_mode(&state::core(cx).paths.user_data, mode);
        cx.notify();
    }

    fn take_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.playing = true;
        self.backlog_ms = 0.0;
        self.last_frame = Instant::now();
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        self.playing = false;
        self.last_slide = Instant::now();
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let m = keys.modifiers;
        if !self.playing || m.platform || m.control || m.alt {
            return;
        }
        let mut boards = self.boards.borrow_mut();
        let game = &mut boards.games[self.index];
        match keys.key.as_str() {
            "escape" => {
                drop(boards);
                self.release(cx);
            }
            "space" | "enter" if game.over() => game.reset(),
            key => match arcade::heading(key) {
                Some((x, y)) => game.steer(x, y),
                None => return,
            },
        }
        cx.stop_propagation();
    }

    fn key_up(&mut self, event: &KeyUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if self.playing && arcade::heading(&event.keystroke.key).is_some_and(|(x, _)| x != 0) {
            self.boards.borrow_mut().games[self.index].steer(0, 0);
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, _: &mut Context<Self>) {
        let Some((bounds, pitch)) = self.painted.get().filter(|_| self.playing) else {
            return;
        };
        let (left, _) = Board::offset(bounds, pitch);
        let column = (f32::from(event.position.x) - left) / pitch;
        self.boards.borrow_mut().games[self.index].point(column as f64);
    }

    /// The three boards side by side, sliding to the one on show.
    fn boards(&self, playing: bool, t: &Tokens) -> impl IntoElement {
        let (from, to) = (self.from as f32, self.index as f32);
        let t = *t;
        let row = div()
            .absolute()
            .top_0()
            .size_full()
            .children((0..GAMES.len()).map(|i| {
                let boards = self.boards.clone();
                let painted = (i == self.index).then(|| (self.painted.clone(), self.visible.clone()));
                div()
                    .absolute()
                    .top_0()
                    .left(relative(i as f32))
                    .size_full()
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _| {
                                let shown = bounds.intersects(&window.content_mask().bounds);
                                if let Some((_, visible)) = &painted {
                                    visible.set(shown);
                                }
                                if !shown {
                                    return;
                                }
                                let scale = f32::from(window.rem_size()) / 16.0;
                                let def = &GAMES[i];
                                let pitch = match def.fit {
                                    Some((cols, rows)) => (f32::from(bounds.size.width) / cols as f32)
                                        .min(f32::from(bounds.size.height) / rows as f32),
                                    None => def.pitch * if playing { 1.5 } else { 1.0 } * scale,
                                };
                                let (cols, rows) = Board::grid(bounds, pitch);
                                let mut boards = boards.borrow_mut();
                                if boards.grids[i] != Some((cols, rows, playing)) {
                                    boards.grids[i] = Some((cols, rows, playing));
                                    boards.games[i].resize(cols, rows);
                                }
                                if let Some((painted, _)) = &painted {
                                    painted.set(Some((bounds, pitch)));
                                }
                                Board {
                                    bounds,
                                    pitch,
                                    color: t.ink,
                                    bg: t.bg,
                                    dim: if playing { 1.0 } else { def.idle_dim },
                                    fade: !playing,
                                    matrix: def.matrix,
                                    window,
                                }
                                .paint(boards.games[i].as_ref());
                            },
                        )
                        .size_full(),
                    )
            }));
        if playing || from == to {
            return row.left(relative(-to)).into_any_element();
        }
        row.with_animation(
            ("arcade-slide", self.slides),
            Animation::new(Duration::from_millis(SLIDE_MS)).with_easing(cubic_bezier(0.4, 0.0, 0.2, 1.0)),
            move |row, delta| row.left(relative(-(from + (to - from) * delta))),
        )
        .into_any_element()
    }

    fn idle(&self, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let def = &GAMES[self.index];
        div()
            .id("arcade")
            .relative()
            .h(u(224.))
            .w_full()
            .flex_shrink_0()
            .font_family(crate::MONO_FONT)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = *hovered;
                this.last_slide = Instant::now();
                cx.notify();
            }))
            .child(div().absolute().size_full().overflow_hidden().child(self.boards(false, t)))
            .when(self.hovered, |band| {
                band.child(
                    div().absolute().size_full().flex().items_center().justify_center().child(
                        div()
                            .id("take-control")
                            .flex()
                            .items_center()
                            .gap(u(8.))
                            .border_1()
                            .border_color(t.ink.opacity(0.25))
                            .bg(t.bg.opacity(0.8))
                            .px(u(12.))
                            .py(u(6.))
                            .text_size(u(11.))
                            .text_color(t.ink.opacity(0.85))
                            .shadow_lg()
                            .cursor_pointer()
                            .hover(|b| b.border_color(t.ink.opacity(0.45)).bg(t.tint(0.1)).text_color(t.ink))
                            .on_click(cx.listener(|this, _, window, cx| this.take_control(window, cx)))
                            .child(div().text_color(t.ink.opacity(0.4)).child("["))
                            .child("take control")
                            .child(div().text_color(t.ink.opacity(0.25)).child("·"))
                            .child(def.label)
                            .child(
                                div().w(u(6.)).h(u(12.)).bg(t.ink.opacity(0.75)).with_animation(
                                    "cursor",
                                    Animation::new(Duration::from_millis(2000))
                                        .repeat()
                                        .with_easing(pulsating_between(0.5, 1.0)),
                                    |d, delta| d.opacity(delta),
                                ),
                            )
                            .child(div().text_color(t.ink.opacity(0.4)).child("]")),
                    )
                    // `opacity-0 transition-opacity duration-200 group-hover:opacity-100`
                    .with_animation("take-control-in", Animation::new(Duration::from_millis(200)), |d, delta| {
                        d.opacity(delta)
                    }),
                )
            })
            .child(
                div().absolute().left_0().right_0().bottom(u(8.)).flex().justify_center().children(
                    (0..GAMES.len()).map(|i| {
                        let on = i == self.index;
                        div()
                            .id(("arcade-dot", i))
                            .size(u(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.show(i, cx)))
                            .child(
                                div()
                                    .size(u(6.))
                                    .bg(t.ink.opacity(if on { 0.45 } else { 0.15 }))
                                    .when(!on, |d| d.hover(|d| d.bg(t.ink.opacity(0.3)))),
                            )
                    }),
                ),
            )
            .into_any_element()
    }

    fn tab(id: impl Into<ElementId>, label: &'static str, on: bool, t: &Tokens) -> Stateful<Div> {
        div()
            .id(id.into())
            .border_1()
            .px(u(8.))
            .py(u(4.))
            .cursor_pointer()
            .map(|b| {
                if on {
                    b.border_color(t.ink.opacity(0.4)).bg(t.tint(0.1)).text_color(t.ink)
                } else {
                    b.border_color(t.ink.opacity(0.1))
                        .text_color(t.ink.opacity(0.4))
                        .hover(|b| b.border_color(t.ink.opacity(0.25)).text_color(t.ink.opacity(0.7)))
                }
            })
            .child(label)
    }

    fn full_screen(&self, window: &Window, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let def = &GAMES[self.index];
        let (score, lives, over) = {
            let boards = self.boards.borrow();
            let game = &boards.games[self.index];
            (game.score(), game.lives(), game.over())
        };
        let best = self.best.get(&self.best_key()).copied().unwrap_or(0);
        let status = Jobs::global(cx).read(cx).jobs.get(&self.job_id).map(|job| {
            format!("{}% · {}", job.percent.min(100.0).round(), stage_label(&job.status))
        });
        let viewport = window.viewport_size();
        let dim = |a: f32| t.ink.opacity(a);
        let root = div()
            .id("arcade-full")
            .track_focus(&self.focus)
            .occlude()
            .relative()
            .w(viewport.width)
            .h(viewport.height)
            .overflow_hidden()
            .bg(t.bg)
            .font_family(crate::MONO_FONT)
            .text_size(u(11.))
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .child(
                div()
                    .absolute()
                    .left(u(16.))
                    .right(u(16.))
                    .top(u(56.))
                    .bottom(u(40.))
                    .overflow_hidden()
                    .child(self.boards(true, t)),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .whitespace_nowrap()
                    .px(u(16.))
                    .py(u(12.))
                    .text_color(dim(0.5))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .gap(u(12.))
                            .child(format!("score {score}"))
                            .child(div().text_color(dim(0.35)).child(format!("best {best}")))
                            .when(lives > 0, |d| {
                                d.child(div().text_color(dim(0.35)).child("•".repeat(lives as usize)))
                            }),
                    )
                    .child(div().flex().justify_center().gap(u(4.)).children(GAMES.iter().enumerate().map(
                        |(i, g)| {
                            Self::tab(("arcade-game", i), g.label, i == self.index, t)
                                .on_click(cx.listener(move |this, _, _, cx| this.show(i, cx)))
                        },
                    )))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(u(4.))
                            .children(Mode::ALL.into_iter().map(|m| {
                                Self::tab(("arcade-mode", m as usize), m.as_str(), m == self.mode, t)
                                    .on_click(cx.listener(move |this, _, _, cx| this.pick_mode(m, cx)))
                            }))
                            .child(div().w(u(8.)))
                            .child(
                                div()
                                    .id("arcade-release")
                                    .flex()
                                    .gap(u(6.))
                                    .border_1()
                                    .border_color(dim(0.2))
                                    .bg(t.bg.opacity(0.7))
                                    .px(u(8.))
                                    .py(u(4.))
                                    .text_color(dim(0.7))
                                    .cursor_pointer()
                                    .hover(|b| b.border_color(dim(0.4)).text_color(t.ink))
                                    .on_click(cx.listener(|this, _, _, cx| this.release(cx)))
                                    .child(div().text_color(dim(0.35)).child("["))
                                    .child("release")
                                    .child(div().text_color(dim(0.35)).child("·"))
                                    .child("esc")
                                    .child(div().text_color(dim(0.35)).child("]")),
                            ),
                    ),
            )
            .when(over, |root| {
                root.child(
                    div().absolute().size_full().flex().items_center().justify_center().child(
                        div()
                            .flex()
                            .gap(u(8.))
                            .border_1()
                            .border_color(dim(0.25))
                            .bg(t.bg.opacity(0.8))
                            .px(u(16.))
                            .py(u(8.))
                            .text_size(u(12.))
                            .text_color(dim(0.85))
                            .child("game over")
                            .child(div().text_color(dim(0.35)).child("·"))
                            .child("space to restart"),
                    ),
                )
            })
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .flex()
                    .justify_between()
                    .px(u(16.))
                    .py(u(12.))
                    .text_color(dim(0.35))
                    .child(def.hint)
                    .children(status),
            );
        deferred(
            anchored()
                .position_mode(AnchoredPositionMode::Window)
                .position(point(px(0.), px(0.)))
                .child(root),
        )
        .with_priority(10)
        .into_any_element()
    }
}

impl Render for Arcade {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        if self.playing {
            self.catch_up();
            window.request_animation_frame();
            // Keeps its place in the page while the board fills the window.
            return div()
                .h(u(224.))
                .w_full()
                .flex_shrink_0()
                .child(self.full_screen(window, &t, cx))
                .into_any_element();
        }
        self.idle(&t, cx)
    }
}
