//! Clipward's desktop app: GPUI views over `clipward_core`.
// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod captions;
mod editor;
mod glass;
mod menu;
mod models;
mod pages;
mod recorder;
mod shell;
#[cfg(feature = "snapshot")]
mod snapshot;
mod state;
mod theme;
mod ui;
// Windows and Linux play through ffmpeg; `--features portable-video` tries that on macOS.
#[cfg_attr(any(not(target_os = "macos"), feature = "portable-video"), path = "video_ffmpeg.rs")]
mod video;
mod workspace;

use clipward_core::{AppPaths, Core, logger};
use gpui_kit::component::button::Button;
use gpui_kit::*;
use models::Settings;
use serde_json::json;
use state::AppState;
use std::path::PathBuf;
use std::time::Duration;
use theme::ActiveTokens;
use ui::{TypeScale, u};
use workspace::Workspace;

/// The platform's UI face (SF Pro on macOS). A Linux system without Noto gets
/// GPUI's fallback stack (Ubuntu, Cantarell, DejaVu Sans…).
pub const UI_FONT: &str = if cfg!(target_os = "macos") {
    ".SystemUIFont"
} else if cfg!(windows) {
    "Segoe UI"
} else {
    "Noto Sans"
};
/// Machine text: paths, versions, error output, codes, typed timecodes.
pub const MONO_FONT: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else if cfg!(windows) {
    "Consolas"
} else {
    "DejaVu Sans Mono"
};

/// A packaged build: inside a macOS `.app` bundle (Contents/MacOS/clipward), whose
/// engine-bin/ and assets/ sit in Contents/Resources; on Windows and Linux, next to
/// a `resources/` folder beside the executable (scripts/bundle.sh).
fn bundle_resources() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    if cfg!(target_os = "macos") {
        let contents = dir.parent()?;
        (dir.file_name()? == "MacOS" && contents.file_name()? == "Contents")
            .then(|| contents.join("Resources"))
    } else {
        let resources = dir.join("resources");
        resources.join("engine-bin").is_dir().then_some(resources)
    }
}

/// Development only: a `--features snapshot` run with `CLIPWARD_SNAPSHOT` set.
fn snapshotting() -> bool {
    cfg!(feature = "snapshot") && std::env::var_os("CLIPWARD_SNAPSHOT").is_some()
}

/// Settings load first; until then a quiet logo, or the error with Retry.
struct AppRoot {
    workspace: Option<Entity<Workspace>>,
}

impl AppRoot {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut root = Self { workspace: None };
        root.load(window, cx);
        root
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = Settings::global(cx);
        settings.update(cx, |settings, cx| settings.load(cx));
        if settings.read(cx).value.is_some() {
            workspace::apply_appearance(&settings, window, cx);
            settings.update(cx, |settings, cx| settings.check_tools(cx));
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            #[cfg(feature = "snapshot")]
            snapshot::tour(workspace.clone(), window, cx);
            self.workspace = Some(workspace);
        }
        cx.notify();
    }
}

impl Render for AppRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(workspace) = &self.workspace {
            return workspace.clone().into_any_element();
        }
        let t = *cx.tokens();
        let failed = Settings::global(cx).read(cx).load_error.is_some();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.bg)
            .font_family(UI_FONT)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .child(if failed {
                ui::card(&t)
                    .px(u(20.))
                    .py(u(16.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(u(12.))
                    .child(
                        div()
                            .text_body()
                            .text_color(t.danger)
                            .child("Could not load settings. Please try again."),
                    )
                    .child(
                        Button::new("retry")
                            .label("Retry")
                            .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
                    )
                    .into_any_element()
            } else {
                div()
                    .text_heading()
                    .text_color(t.ink_secondary)
                    .child("Clipward")
                    .into_any_element()
            })
            .into_any_element()
    }
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        logger::error("main.panic", json!({ "at": location.replace(['/', '\\'], ".") }));
    }));
    // Lives as long as the process: jobs and renders run on it until quit.
    let runtime: &'static tokio::runtime::Runtime = Box::leak(Box::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("clipward-core")
            .build()
            .expect("start the async runtime"),
    ));
    let resources = bundle_resources();
    let bundled = resources.is_some();
    let paths = AppPaths::resolve(bundled, resources);
    #[cfg(any(not(target_os = "macos"), feature = "portable-video"))]
    video::init(&paths);
    let (job_tx, job_rx) = tokio::sync::mpsc::unbounded_channel();
    let core = Core::start(paths, runtime.handle().clone(), env!("CARGO_PKG_VERSION"), move |job| {
        let _ = job_tx.send(job.clone());
    });

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.set_global(AppState { core: core.clone() });
            models::init(job_rx, cx);
            workspace::bind_keys(cx);
            menu::init(cx);
            if !bundled {
                menu::set_unbundled_icon();
            }
            theme::Tokens::new(&Default::default(), true, false).install(cx);

            // Queued runs are recorded as cancelled; running ones are cancelled and
            // get up to 5 s to stop their media processes and clean up. GPUI only
            // polls quit futures for 200 ms, so wait here, before returning one.
            let quitting = core.clone();
            cx.on_app_quit(move |_| {
                if quitting.jobs.stop_all_for_quit() > 0 {
                    for _ in 0..50 {
                        if quitting.jobs.running_count() == 0 {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                async {}
            })
            .detach();

            #[cfg(feature = "snapshot")]
            if snapshot::video_probe(cx) {
                return;
            }
            #[cfg(feature = "snapshot")]
            if snapshot::editor_probe(cx) {
                return;
            }
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1280.), px(800.)), cx)),
                window_min_size: Some(size(px(720.), px(520.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Clipward".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.), px(14.))),
                }),
                app_owns_titlebar_drag: true,
                // Snapshot runs render off screen, so they never pop up over the user's work.
                show: !snapshotting(),
                focus: !snapshotting(),
                ..Default::default()
            };
            let (handle, _) =
                gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| AppRoot::new(window, cx)))
                    .expect("open the main window");
            // The UI kit's root fills the window with the theme's opaque background,
            // which would hide window glass. The workspace paints its own backdrop
            // (translucent over glass), so the root stays clear.
            let _ = handle.update(cx, |_, window, cx| {
                base::Root::update(window, cx, |root, _, _| {
                    root.style().background = Some(transparent_black().into());
                });
            });
            if !snapshotting() {
                cx.activate(true);
            }
        });
}
