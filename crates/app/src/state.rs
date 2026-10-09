//! The app-wide handle on `clipward_core`, and running its async work.
//!
//! Core's async methods need tokio; GPUI's executor is its own. `run` hands a
//! future to the tokio runtime and gives back one the UI can await, so a view
//! does `let x = state::run(cx, async move { core.history_list().await });`
//! inside `cx.spawn`.
use clipward_core::Core;
use gpui_kit::{App, Global};
use std::future::Future;
use std::sync::Arc;

pub struct AppState {
    pub core: Arc<Core>,
}

impl Global for AppState {}

pub fn core(cx: &App) -> Arc<Core> {
    cx.global::<AppState>().core.clone()
}

/// Run `future` on the tokio runtime; the returned future resolves on whatever
/// executor awaits it. A panic in the task becomes an error, not a crash.
pub fn run<T, F>(cx: &App, future: F) -> impl Future<Output = Result<T, String>> + use<T, F>
where
    T: Send + 'static,
    F: Future<Output = Result<T, String>> + Send + 'static,
{
    let task = core(cx).runtime().spawn(future);
    async move {
        task.await
            .unwrap_or_else(|_| Err("Clipward hit an internal error. Retry the action.".into()))
    }
}
