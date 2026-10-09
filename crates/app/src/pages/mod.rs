//! The four pages of the main window.
//!
//! While the port is in progress each page sits behind a cargo feature
//! (`page-create`, …; `all-pages` turns on every one), so one page in mid-edit
//! can't break another's build. A page that is off shows a placeholder.
use crate::theme::ActiveTokens;
use crate::ui::{TypeScale, u};
use crate::workspace::Page;
use gpui_kit::*;

#[cfg(feature = "page-create")]
pub mod create;
#[cfg(feature = "page-jobs")]
pub mod jobs;
#[cfg(feature = "page-library")]
pub mod library;
#[cfg(feature = "page-settings")]
pub mod settings;

/// A fresh view for `page`; each visit starts at the top with fresh local state.
pub fn open<V: 'static>(page: Page, window: &mut Window, cx: &mut Context<V>) -> AnyView {
    let _ = &window;
    match page {
        #[cfg(feature = "page-create")]
        Page::Create => cx.new(|cx| create::CreatePage::new(window, cx)).into(),
        #[cfg(feature = "page-library")]
        Page::Library => cx.new(|cx| library::LibraryPage::new(window, cx)).into(),
        #[cfg(feature = "page-jobs")]
        Page::Jobs => cx.new(|cx| jobs::JobsPage::new(window, cx)).into(),
        #[cfg(feature = "page-settings")]
        Page::Settings => cx.new(|cx| settings::SettingsPage::new(window, cx)).into(),
        #[allow(unreachable_patterns)]
        other => cx.new(|_| Placeholder(other)).into(),
    }
}

/// A page not ported yet.
pub struct Placeholder(pub Page);

impl Render for Placeholder {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(u(24.))
            .text_title()
            .text_color(t.ink_secondary)
            .child(format!("{:?}", self.0))
    }
}
