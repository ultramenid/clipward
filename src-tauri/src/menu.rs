//! Application menu (port of src/main/menu.ts).
use crate::shared::{ISSUES_URL, REPO_URL};
use tauri::menu::{IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Wry};

pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let sep = || PredefinedMenuItem::separator(app);

    let file = if cfg!(target_os = "macos") {
        Submenu::with_items(
            app,
            "File",
            true,
            &[&PredefinedMenuItem::close_window(app, None)?],
        )?
    } else {
        Submenu::with_items(app, "File", true, &[&PredefinedMenuItem::quit(app, None)?])?
    };
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &sep()?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let reload = MenuItem::with_id(app, "reload", "Reload", true, Some("CmdOrCtrl+R"))?;
    let devtools = MenuItem::with_id(
        app,
        "devtools",
        "Toggle Developer Tools",
        true,
        Some("Alt+CmdOrCtrl+I"),
    )?;
    let reset_zoom =
        MenuItem::with_id(app, "zoom-reset", "Actual Size", true, Some("CmdOrCtrl+0"))?;
    let zoom_in = MenuItem::with_id(app, "zoom-in", "Zoom In", true, Some("CmdOrCtrl+="))?;
    let zoom_out = MenuItem::with_id(app, "zoom-out", "Zoom Out", true, Some("CmdOrCtrl+-"))?;
    let (sep1, sep2) = (sep()?, sep()?);
    let fullscreen = PredefinedMenuItem::fullscreen(app, None)?;
    let mut view_items: Vec<&dyn IsMenuItem<Wry>> = vec![&reload];
    if cfg!(debug_assertions) {
        view_items.push(&devtools);
    }
    view_items.extend([
        &sep1 as &dyn IsMenuItem<Wry>,
        &reset_zoom,
        &zoom_in,
        &zoom_out,
        &sep2,
    ]);
    if !cfg!(target_os = "linux") {
        view_items.push(&fullscreen);
    }
    let view = Submenu::with_items(app, "View", true, &view_items)?;

    // TODO(updater): add "Check for Updates…" once tauri-plugin-updater is wired.
    let help = Submenu::with_items(
        app,
        "Help",
        true,
        &[
            &MenuItem::with_id(app, "github", "Clipward on GitHub", true, None::<&str>)?,
            &MenuItem::with_id(app, "report-issue", "Report an Issue…", true, None::<&str>)?,
            &sep()?,
            &MenuItem::with_id(app, "show-logs", "Show Logs", true, None::<&str>)?,
        ],
    )?;

    #[cfg(target_os = "macos")]
    {
        let app_menu = Submenu::with_items(
            app,
            "Clipward",
            true,
            &[
                &PredefinedMenuItem::about(app, None, None)?,
                &sep()?,
                &PredefinedMenuItem::services(app, None)?,
                &sep()?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::show_all(app, None)?,
                &sep()?,
                &PredefinedMenuItem::quit(app, None)?,
            ],
        )?;
        return Menu::with_items(app, &[&app_menu, &file, &edit, &view, &help]);
    }
    #[cfg(not(target_os = "macos"))]
    Menu::with_items(app, &[&file, &edit, &view, &help])
}

pub fn handle_event(app: &AppHandle, event: MenuEvent) {
    let window = app.get_webview_window("main");
    match event.id().as_ref() {
        "reload" => {
            if let Some(window) = window {
                let _ = window.reload();
            }
        }
        "devtools" if cfg!(debug_assertions) =>
        {
            #[cfg(debug_assertions)]
            if let Some(window) = window {
                if window.is_devtools_open() {
                    window.close_devtools();
                } else {
                    window.open_devtools();
                }
            }
        }
        // Zoom is Settings → Appearance → Interface scale: saved, applied as
        // the webview zoom, and reported so the slider follows.
        id @ ("zoom-reset" | "zoom-in" | "zoom-out") => {
            use crate::appearance::{UI_SCALE, UI_SCALE_STEP};
            let state = app.state::<crate::AppState>();
            let Ok(current) = state.settings.load() else {
                return;
            };
            let scale = current.appearance.ui_scale;
            let next = match id {
                "zoom-in" => scale + UI_SCALE_STEP,
                "zoom-out" => scale - UI_SCALE_STEP,
                _ => UI_SCALE.default,
            };
            if let Ok(saved) = state.settings.set_ui_scale(next) {
                if let Some(window) = window {
                    let _ = window.set_zoom(saved.appearance.ui_scale);
                }
                let _ = app.emit_to("main", "settings:changed", saved);
            }
        }
        "github" => {
            let _ = crate::commands::open_trusted_url(app, REPO_URL);
        }
        "report-issue" => {
            let _ = crate::commands::open_trusted_url(app, ISSUES_URL);
        }
        "show-logs" => {
            crate::commands::show_log_file(app);
        }
        _ => {}
    }
}
