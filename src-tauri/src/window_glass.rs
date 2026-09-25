//! Native window glass: the desktop shows through the window behind a
//! translucent renderer (`html.glass`, see DESIGN.md).
//!
//! macOS gets an `NSVisualEffectView` with the sidebar material under the
//! webview; Windows gets Mica (Windows 11) or acrylic (Windows 10). Linux stays
//! opaque: compositors differ and WebKitGTK repaints transparent windows badly.
//!
//! The window is always created transparent on macOS and Windows (a window
//! can't become transparent later), so glass can be switched at runtime. When
//! glass is off the window and webview paint the solid base colour and no
//! effect is applied, which looks and composites like an opaque window.
//!
//! Adapted from MonoCode's `src-tauri/src/macos.rs` and `window.rs` (MIT,
//! Copyright (c) 2026 Nick; see THIRD_PARTY_NOTICES.md): the transparent
//! window, nearly-clear macOS background and acrylic fallback. MonoCode blurs
//! with a private WindowServer call; Clipward uses the public visual-effect
//! material through `window-vibrancy` instead.
use crate::logger;
use serde_json::json;
use tauri::window::Color;
use tauri::WebviewWindow;

/// Whether this platform draws native glass at all.
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// The renderer's base (`--bg`, #171717): what an opaque window paints.
pub const OPAQUE: Color = Color(23, 23, 23, 255);

/// Behind glass. On macOS a fully clear window with a shadow draws a
/// chamfered gap at its rounded corners, so keep a hint of alpha (about 1%).
pub const CLEAR: Color = if cfg!(target_os = "macos") {
    Color(0, 0, 0, 3)
} else {
    Color(0, 0, 0, 0)
};

/// Acrylic tint on Windows 10, where Mica is unavailable: the dark base at
/// about 60% so text stays legible over bright wallpapers.
const ACRYLIC_TINT: window_vibrancy::Color = (18, 18, 18, 150);

/// The background colour to create the main window with.
pub fn initial_background(enabled: bool) -> Color {
    if SUPPORTED && enabled {
        CLEAR
    } else {
        OPAQUE
    }
}

/// Turn glass on or off for `window`. Runs on the main thread (AppKit and
/// DWM calls must); failures are logged and leave the window opaque.
pub fn apply(window: &WebviewWindow, enabled: bool) {
    if !SUPPORTED {
        return;
    }
    let target = window.clone();
    let scheduled = window.run_on_main_thread(move || {
        if enabled {
            match enable(&target) {
                Ok(material) => logger::info("window.glass", json!({ "material": material })),
                Err(error) => {
                    disable(&target);
                    logger::warn("window.glass_failed", json!({ "error": error }));
                }
            }
        } else {
            disable(&target);
        }
    });
    if scheduled.is_err() {
        logger::warn(
            "window.glass_failed",
            json!({ "error": "main thread unavailable" }),
        );
    }
}

fn enable(window: &WebviewWindow) -> Result<&'static str, String> {
    window
        .set_background_color(Some(CLEAR))
        .map_err(|error| error.to_string())?;
    if cfg!(target_os = "macos") {
        window_vibrancy::apply_vibrancy(
            window,
            window_vibrancy::NSVisualEffectMaterial::Sidebar,
            Some(window_vibrancy::NSVisualEffectState::FollowsWindowActiveState),
            None,
        )
        .map(|()| "vibrancy")
        .map_err(|error| error.to_string())
    } else if cfg!(target_os = "windows") {
        // Mica is Windows 11 only; acrylic covers Windows 10 1809 and later.
        window_vibrancy::apply_mica(window, Some(true))
            .map(|()| "mica")
            .or_else(|_| {
                window_vibrancy::apply_acrylic(window, Some(ACRYLIC_TINT)).map(|()| "acrylic")
            })
            .map_err(|error| error.to_string())
    } else {
        Err("unsupported platform".into())
    }
}

fn disable(window: &WebviewWindow) {
    if cfg!(target_os = "macos") {
        let _ = window_vibrancy::clear_vibrancy(window);
    } else if cfg!(target_os = "windows") {
        let _ = window_vibrancy::clear_mica(window);
        let _ = window_vibrancy::clear_acrylic(window);
    }
    let _ = window.set_background_color(Some(OPAQUE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_unless_supported_and_enabled() {
        assert_eq!(initial_background(false), OPAQUE);
        assert_eq!(
            initial_background(true),
            if SUPPORTED { CLEAR } else { OPAQUE }
        );
        assert_eq!(
            SUPPORTED,
            cfg!(any(target_os = "macos", target_os = "windows"))
        );
    }
}
