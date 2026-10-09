//! The menu bar: app, File, Edit, View, Help. The
//! webview-only Reload and Developer Tools items are gone.
use crate::models::Settings;
use clipward_core::appearance::{zoom_in_ui_scale, zoom_out_ui_scale, UI_SCALE};
use clipward_core::shared::{ISSUES_URL, REPO_URL};
use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::*;

actions!(
    menu,
    [
        About,
        Hide,
        HideOthers,
        ShowAll,
        Quit,
        CloseWindow,
        ActualSize,
        ZoomIn,
        ZoomOut,
        ToggleFullScreen,
        OpenGitHub,
        ReportIssue,
        ShowLogs
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("secondary-0", ActualSize, None),
        KeyBinding::new("secondary-=", ZoomIn, None),
        KeyBinding::new("secondary-+", ZoomIn, None),
        KeyBinding::new("secondary--", ZoomOut, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
    ]);
    cx.on_action(|_: &About, _| show_about_panel());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
    });
    cx.on_action(|_: &ActualSize, cx| set_scale(|_| UI_SCALE.default, cx));
    cx.on_action(|_: &ZoomIn, cx| set_scale(zoom_in_ui_scale, cx));
    cx.on_action(|_: &ZoomOut, cx| set_scale(zoom_out_ui_scale, cx));
    cx.on_action(|_: &ToggleFullScreen, cx| {
        if let Some(window) = cx.active_window() {
            let _ = window.update(cx, |_, window, _| window.toggle_fullscreen());
        }
    });
    cx.on_action(|_: &OpenGitHub, cx| {
        let _ = crate::shell::open_url(REPO_URL, cx);
    });
    cx.on_action(|_: &ReportIssue, cx| {
        let _ = crate::shell::open_url(ISSUES_URL, cx);
    });
    cx.on_action(|_: &ShowLogs, cx| {
        crate::shell::show_log_file(cx);
    });

    let mut menus = Vec::new();
    if cfg!(target_os = "macos") {
        menus.push(Menu::new("Clipward").items([
            MenuItem::action("About Clipward", About),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Clipward", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Clipward", Quit),
        ]));
        menus.push(Menu::new("File").items([MenuItem::action("Close Window", CloseWindow)]));
    } else {
        menus.push(Menu::new("File").items([MenuItem::action("Quit", Quit)]));
    }
    menus.push(Menu::new("Edit").items([
        MenuItem::os_action("Undo", Undo, OsAction::Undo),
        MenuItem::os_action("Redo", Redo, OsAction::Redo),
        MenuItem::separator(),
        MenuItem::os_action("Cut", Cut, OsAction::Cut),
        MenuItem::os_action("Copy", Copy, OsAction::Copy),
        MenuItem::os_action("Paste", Paste, OsAction::Paste),
        MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
    ]));
    let mut view = vec![
        MenuItem::action("Actual Size", ActualSize),
        MenuItem::action("Zoom In", ZoomIn),
        MenuItem::action("Zoom Out", ZoomOut),
    ];
    if !cfg!(target_os = "linux") {
        view.push(MenuItem::separator());
        view.push(MenuItem::action("Enter Full Screen", ToggleFullScreen));
    }
    menus.push(Menu::new("View").items(view));
    menus.push(Menu::new("Help").items([
        MenuItem::action("Clipward on GitHub", OpenGitHub),
        MenuItem::action("Report an Issue…", ReportIssue),
        MenuItem::separator(),
        MenuItem::action("Show Logs", ShowLogs),
    ]));
    cx.set_menus(menus);
}

/// Interface scale from the View menu: saved like the Appearance slider.
fn set_scale(next: fn(f64) -> f64, cx: &mut App) {
    Settings::global(cx).update(cx, |settings, cx| {
        let mut appearance = settings.appearance();
        let scale = next(appearance.ui_scale);
        if scale != appearance.ui_scale {
            appearance.ui_scale = scale;
            settings.set_appearance(appearance, cx);
        }
    });
}

/// The standard macOS About panel: name, version and icon from the bundle.
/// Outside an app bundle (`cargo run`) macOS shows a generic executable in the Dock
/// and the About panel; give it Clipward's icon. A bundle names its own icon.icns.
pub fn set_unbundled_icon() {
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        static ICON: &[u8] = include_bytes!("../icons/icon.png");
        let data: *mut AnyObject =
            msg_send![class!(NSData), dataWithBytes: ICON.as_ptr().cast::<std::ffi::c_void>(), length: ICON.len()];
        let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: data];
        if image.is_null() {
            return;
        }
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
        // The application keeps its own reference.
        let _: () = msg_send![image, release];
    }
}

fn show_about_panel() {
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let none: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![app, orderFrontStandardAboutPanel: none];
    }
}
