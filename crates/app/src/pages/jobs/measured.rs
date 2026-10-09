//! Child views the Jobs page draws from GPUI's view cache.
//!
//! A notify re-renders the view that called it and every view above it, so the
//! page re-renders whenever a timer, the arcade or an animation inside it ticks.
//! The parts that didn't change (the Previous list, the progress ring, the
//! stepper, the arcade) are entities drawn with `cached`, which reuses last
//! frame's output unless they notified themselves. A cached view is laid out
//! from a style instead of its content, so each one records the height it laid
//! out at, and the page uses it on the next frame.
use super::JobsPage;
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Default)]
pub(super) struct Measured(Rc<Cell<Option<Pixels>>>);

impl Measured {
    /// The view, from the cache once its height is known; drawn fresh until then.
    pub fn view<V: Render>(&self, view: &Entity<V>) -> AnyElement {
        match self.0.get() {
            Some(height) => {
                let mut style = StyleRefinement::default().w_full().flex_shrink_0();
                style = style.h(height);
                view.clone().cached(style).into_any_element()
            }
            None => view.clone().into_any_element(),
        }
    }

    /// A view's content, measured as it lays out. When its height changed, the
    /// page lays it out again on the next frame.
    pub fn wrap(&self, page: &WeakEntity<JobsPage>, content: impl IntoElement) -> Div {
        let cell = self.0.clone();
        let page = page.clone();
        div().relative().w_full().child(content).child(
            canvas(
                move |bounds, window, _| {
                    let height = bounds.size.height;
                    if cell.get() != Some(height) {
                        cell.set(Some(height));
                        window.on_next_frame(move |_, cx| {
                            let _ = page.update(cx, |_, cx| cx.notify());
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }
}
