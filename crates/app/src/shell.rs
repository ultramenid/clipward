//! The native parts around `Core`: file dialogs with type
//! filters, and opening or revealing files and links. Every path still goes through
//! the core's checks.
use crate::state;
use clipward_core::{logger, security};
use gpui_kit::App;
use serde_json::json;
use std::path::PathBuf;

/// A file type filter for the pickers: a label and its extensions (no dots).
pub struct Filter<'a>(pub &'a str, pub &'a [&'a str]);

fn dialog(title: &str, filters: &[Filter]) -> rfd::AsyncFileDialog {
    filters.iter().fold(
        rfd::AsyncFileDialog::new().set_title(title),
        |dialog, Filter(label, kinds)| dialog.add_filter(*label, kinds),
    )
}

/// Pick one file; None when cancelled.
pub async fn pick_file(title: &str, filters: &[Filter<'_>]) -> Option<PathBuf> {
    dialog(title, filters)
        .pick_file()
        .await
        .map(|file| file.path().to_path_buf())
}

/// Pick any number of files.
pub async fn pick_files(title: &str, filters: &[Filter<'_>]) -> Vec<PathBuf> {
    dialog(title, filters)
        .pick_files()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|file| file.path().to_path_buf())
        .collect()
}

/// Pick a folder (new folders allowed); None when cancelled.
pub async fn pick_folder(title: &str) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title(title)
        .set_can_create_directories(true)
        .pick_folder()
        .await
        .map(|dir| dir.path().to_path_buf())
}

/// Pick a source video: authorized for this session, as `dialog_select_video` did.
pub async fn pick_video(cx: &gpui_kit::AsyncApp) -> Result<Option<String>, String> {
    let filters = [
        Filter("Video Files", clipward_core::shared::VIDEO_EXTENSIONS),
        Filter("All Files", &["*"]),
    ];
    let Some(path) = pick_file("Choose a Video", &filters).await else {
        return Ok(None);
    };
    cx.update(|cx| state::core(cx).authorize_picked(&path)).map(Some)
}

/// Pick up to MAX_LOGOS logo images, each authorized for this session.
pub async fn pick_logos(cx: &gpui_kit::AsyncApp) -> Result<Vec<String>, String> {
    let filters = [Filter("Images", clipward_core::shared::LOGO_EXTENSIONS)];
    let paths = pick_files("Choose Logos", &filters).await;
    cx.update(|cx| {
        let core = state::core(cx);
        paths
            .iter()
            .take(clipward_core::shared::MAX_LOGOS)
            .map(|path| core.authorize_picked(path))
            .collect()
    })
}

/// Development-only: scripted end-to-end runs must never open the developer's browser.
fn external_links_blocked() -> bool {
    cfg!(debug_assertions) && std::env::var("CLIPWARD_E2E").as_deref() == Ok("1")
}

/// Open a trusted web link (GitHub, the issue tracker, provider key pages).
pub fn open_url(url: &str, cx: &App) -> Result<(), String> {
    if !security::is_trusted_external_url(url) {
        return Err("This external link is not supported".into());
    }
    if external_links_blocked() {
        logger::info("e2e.openExternal.blocked", json!({}));
        return Ok(());
    }
    cx.open_url(url);
    Ok(())
}

/// Open a library file or folder (or a link) with its default app; false when
/// the file is gone.
pub fn open_path(path: &str, cx: &App) -> Result<bool, String> {
    if security::is_web_url(path) {
        open_url(path, cx)?;
        return Ok(true);
    }
    let Some(path) = state::core(cx).openable_path(path)? else {
        return Ok(false);
    };
    cx.open_with_system(&path);
    Ok(true)
}

/// Show a library file in Finder; false when it is gone.
pub fn reveal(path: &str, cx: &App) -> Result<bool, String> {
    let Some(path) = state::core(cx).revealable_path(path)? else {
        return Ok(false);
    };
    cx.reveal_path(&path);
    Ok(true)
}

/// Show the log file in Finder (Help → Show Logs, Settings → About).
pub fn show_log_file(cx: &App) -> bool {
    let log = logger::log_file_path();
    let exists = std::path::Path::new(&log).exists();
    if exists {
        cx.reveal_path(std::path::Path::new(&log));
    }
    exists
}
