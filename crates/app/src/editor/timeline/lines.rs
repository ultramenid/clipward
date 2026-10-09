//! The playhead and the hover line (Timeline.tsx `Timeline`). They move every frame
//! while playing, so they are their own small view following the `Playhead`: the
//! tracks under them re-render on edits alone.
use super::tracks::{DEFAULT_ZOOM, Target, Tracks};
use crate::editor::state::{EditorState, Playhead};
use crate::theme::ActiveTokens;
use clipward_core::timeline_view as view;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub struct Lines {
    editor: Entity<EditorState>,
    playhead: Entity<Playhead>,
    tracks: Entity<Tracks>,
    _subscriptions: Vec<Subscription>,
}

impl Lines {
    pub fn new(editor: Entity<EditorState>, tracks: Entity<Tracks>, cx: &mut Context<Self>) -> Self {
        let playhead = editor.read(cx).playhead.clone();
        let subscriptions = vec![
            cx.observe(&playhead, |_, _, cx| cx.notify()),
            // Zoom changes.
            cx.observe(&editor, |_, _, cx| cx.notify()),
            // Scrolling and resizing.
            cx.observe(&tracks, |_, _, cx| cx.notify()),
        ];
        Self { editor, playhead, tracks, _subscriptions: subscriptions }
    }
}

impl Render for Lines {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let zoom = self.editor.read(cx).zoom.unwrap_or(DEFAULT_ZOOM);
        let (scale, scroll_x) = self.tracks.read(cx).geometry();
        let playhead = self.playhead.read(cx);
        let at = |ms: f64| px(((view::x_at(ms, zoom) - scroll_x) as f32) * scale);
        // The time under the mouse shows while paused.
        let hover = if playhead.playing { None } else { playhead.hover };
        let x = at(playhead.time);
        let tracks = self.tracks.clone();
        let content = t.ink;
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .when_some(hover, |d, time| {
                d.child(div().absolute().top_0().bottom_0().left(at(time)).w(px(1.)).bg(t.tint(0.4)))
            })
            .child(div().absolute().top_0().bottom_0().left(x).w(px(1.)).bg(content))
            .child(
                // The diamond on top: an 8 px square turned 45°.
                canvas(|_, _, _| {}, move |bounds, _, window, _| {
                    let half = px(4. * std::f32::consts::SQRT_2 * scale);
                    let (cx_, cy) = (x + px(0.5), bounds.top() + px(4. * scale));
                    let left = bounds.left();
                    let mut path = PathBuilder::fill();
                    path.add_polygon(
                        &[
                            point(left + cx_, cy - half),
                            point(left + cx_ + half, cy),
                            point(left + cx_, cy + half),
                            point(left + cx_ - half, cy),
                        ],
                        true,
                    );
                    if let Ok(path) = path.build() {
                        window.paint_path(path, content);
                    }
                })
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(
                // Grab the playhead anywhere along its length, not just on the ruler.
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(x - px(4. * scale))
                    .w(px(10. * scale))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(MouseButton::Left, move |e, _, cx| {
                        cx.stop_propagation();
                        tracks.update(cx, |this, cx| this.start(Target::Seek, e, cx));
                    }),
            )
    }
}
