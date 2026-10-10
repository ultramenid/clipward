//! The timeline (Timeline.tsx, Filmstrip.tsx, Waveform.tsx). See docs/GPUI_PORT.md.
//!
//! Two cached views under this one: `Tracks` draws every track and handles every drag
//! (re-rendering on edits, selection, zoom and scrolling), `Lines` draws the playhead
//! and hover line and follows the `Playhead`, which changes every frame while playing.
//! The geometry and drag math live in `clipward_core::timeline_view`.
mod lines;
mod media;
mod tracks;

use super::state::EditorState;
use crate::ui::u;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lines::Lines;
use tracks::Tracks;

pub struct Timeline {
    editor: Entity<EditorState>,
    tracks: Entity<Tracks>,
    lines: Entity<Lines>,
    _subscription: Subscription,
}

impl Timeline {
    pub fn new(editor: Entity<EditorState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let scroll = ScrollHandle::new();
        let tracks = cx.new(|cx| Tracks::new(editor.clone(), scroll, cx));
        let lines = cx.new(|cx| Lines::new(editor.clone(), tracks.clone(), cx));
        // The height follows the tracks (lanes, captions, the main track's height).
        let subscription = cx.observe(&tracks, |_, _, cx| cx.notify());
        Self { editor, tracks, lines, _subscription: subscription }
    }

    /// The height the timeline takes: the ruler, every track and the main track at the
    /// editor's track height (nothing until the clip loads). A parent caching this view
    /// lays it out at this height.
    /// Zoom by `factor` about the playhead (the toolbar's zoom buttons).
    pub fn zoom_by(&self, factor: f64, cx: &mut App) {
        self.tracks.update(cx, |tracks, cx| tracks.zoom_by(factor, cx));
    }

    pub fn height(&self, cx: &App) -> Rems {
        u(self.tracks.read(cx).height() as f32)
    }
}

impl Render for Timeline {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let height = self.height(cx);
        let loaded = self.editor.read(cx).project().is_some();
        div()
            .relative()
            .w_full()
            .h(height)
            .flex_shrink_0()
            .when(loaded, |d| {
                d.child(self.tracks.clone().cached(StyleRefinement::default().size_full()))
                    .child(div().absolute().top_0().left_0().size_full().child(
                        self.lines.clone().cached(StyleRefinement::default().size_full()),
                    ))
            })
    }
}
