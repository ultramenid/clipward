//! Native window glass: the desktop shows through the window behind a
//! translucent renderer (`html.glass`, see DESIGN.md).
//!
//! macOS blurs the desktop behind the window with WindowServer
//! (`CGSSetWindowBackgroundBlurRadius`, a private call looked up at runtime),
//! so Settings → Appearance → Blur radius can change it. Under the webview sits
//! the `NSVisualEffectView` that `window-vibrancy` adds, faded to 1% alpha: it
//! no longer tints anything, but keeps AppKit compositing hover repaints and
//! captures through the blur (without it single frames show unblurred web
//! content). If the private call is missing, the view stays at full strength
//! and its sidebar material does the blurring instead. Windows gets Mica
//! (Windows 11) or acrylic (Windows 10), whose blur is fixed. Linux stays
//! opaque: compositors differ and WebKitGTK repaints transparent windows badly.
//!
//! The window is always created transparent on macOS and Windows (a window
//! can't become transparent later), so glass can be switched at runtime. When
//! glass is off the window and webview paint the solid base colour and no
//! effect is applied, which looks and composites like an opaque window.
//!
//! Adapted from MonoCode's `src-tauri/src/macos.rs` and `window.rs` (MIT,
//! Copyright (c) 2026 Nick; see THIRD_PARTY_NOTICES.md): the transparent
//! window, nearly-clear macOS background, WindowServer blur with its 1%
//! backing view, and the acrylic fallback.
use crate::logger;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use tauri::window::Color;
use tauri::WebviewWindow;

/// Whether this platform draws native glass at all.
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// Whether the blur radius can be changed (macOS WindowServer blur).
pub const BLUR_SUPPORTED: bool = cfg!(target_os = "macos");

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

/// The blur radius last asked for; glass applies it whenever it turns on.
static BLUR_RADIUS: AtomicU8 = AtomicU8::new(crate::appearance::BLUR_RADIUS.default as u8);
/// Whether glass is on, so a new radius takes effect at once.
static GLASS_ON: AtomicBool = AtomicBool::new(false);

/// The background colour to create the main window with.
pub fn initial_background(enabled: bool) -> Color {
    if SUPPORTED && enabled {
        CLEAR
    } else {
        OPAQUE
    }
}

/// Clamp a blur radius to the setting's range (1–64 points).
fn blur_radius(radius: f64) -> u8 {
    let range = crate::appearance::BLUR_RADIUS;
    if radius.is_finite() {
        radius.clamp(range.min, range.max).round() as u8
    } else {
        range.default as u8
    }
}

/// Remember the blur radius and, if glass is on, apply it now.
pub fn set_blur_radius(window: &WebviewWindow, radius: f64) {
    BLUR_RADIUS.store(blur_radius(radius), Ordering::Relaxed);
    if !BLUR_SUPPORTED || !GLASS_ON.load(Ordering::Relaxed) {
        return;
    }
    let target = window.clone();
    let _ = window.run_on_main_thread(move || {
        #[cfg(target_os = "macos")]
        if macos::set_blur(&target, BLUR_RADIUS.load(Ordering::Relaxed)) {
            macos::dim_vibrancy(&target);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = target;
    });
}

/// Turn glass on or off for `window`. Runs on the main thread (AppKit and
/// DWM calls must); failures are logged and leave the window opaque.
pub fn apply(window: &WebviewWindow, enabled: bool) {
    if !SUPPORTED {
        return;
    }
    GLASS_ON.store(enabled, Ordering::Relaxed);
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
        // Switching on twice must not stack a second effect view.
        let _ = window_vibrancy::clear_vibrancy(window);
        window_vibrancy::apply_vibrancy(
            window,
            window_vibrancy::NSVisualEffectMaterial::Sidebar,
            Some(window_vibrancy::NSVisualEffectState::FollowsWindowActiveState),
            None,
        )
        .map_err(|error| error.to_string())?;
        #[cfg(target_os = "macos")]
        if macos::set_blur(window, BLUR_RADIUS.load(Ordering::Relaxed)) {
            macos::dim_vibrancy(window);
            return Ok("windowserver-blur");
        }
        Ok("vibrancy")
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
        #[cfg(target_os = "macos")]
        macos::set_blur(window, 0);
        let _ = window_vibrancy::clear_vibrancy(window);
    } else if cfg!(target_os = "windows") {
        let _ = window_vibrancy::clear_mica(window);
        let _ = window_vibrancy::clear_acrylic(window);
    }
    let _ = window.set_background_color(Some(OPAQUE));
}

/// WindowServer background blur, looked up at runtime as in MonoCode's
/// `macos.rs`. Every call is a no-op when the private symbols are missing.
#[cfg(target_os = "macos")]
mod macos {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use std::ffi::{c_char, c_int, c_void};
    use std::sync::OnceLock;
    use tauri::WebviewWindow;

    type CgsConnection = usize;
    type SetBlurFn = unsafe extern "C" fn(CgsConnection, c_int, c_int) -> c_int;
    type ConnectionFn = unsafe extern "C" fn() -> CgsConnection;

    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    /// `NSView.tag` of the effect view `window-vibrancy` adds (its `NS_VIEW_TAG_BLUR_VIEW`).
    const VIBRANCY_VIEW_TAG: isize = 91376254;

    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    fn dlsym_fn<T: Copy>(symbol: &[u8]) -> Option<T> {
        // SAFETY: `symbol` is NUL-terminated and T is a function pointer with
        // the symbol's C signature.
        unsafe {
            let ptr = dlsym(RTLD_DEFAULT, symbol.as_ptr().cast());
            (!ptr.is_null()).then(|| std::mem::transmute_copy(&ptr))
        }
    }

    fn set_blur_fn() -> Option<SetBlurFn> {
        static FN: OnceLock<Option<SetBlurFn>> = OnceLock::new();
        *FN.get_or_init(|| dlsym_fn(b"CGSSetWindowBackgroundBlurRadius\0"))
    }

    fn connection() -> Option<CgsConnection> {
        static FN: OnceLock<Option<ConnectionFn>> = OnceLock::new();
        let function = (*FN.get_or_init(|| {
            dlsym_fn(b"CGSDefaultConnectionForThread\0")
                .or_else(|| dlsym_fn(b"CGSMainConnectionID\0"))
        }))?;
        // SAFETY: takes no arguments and returns the connection id.
        let connection = unsafe { function() };
        (connection != 0).then_some(connection)
    }

    fn ns_window(window: &WebviewWindow) -> Option<&AnyObject> {
        let ptr = window.ns_window().ok()?.cast::<AnyObject>();
        // SAFETY: Tauri hands out the live NSWindow, used here on the main thread.
        unsafe { ptr.as_ref() }
    }

    /// Blur the desktop behind the window by `radius` points (0 turns it
    /// off). Returns whether WindowServer took it.
    pub fn set_blur(window: &WebviewWindow, radius: u8) -> bool {
        let (Some(set_blur), Some(connection), Some(ns_window)) =
            (set_blur_fn(), connection(), ns_window(window))
        else {
            return false;
        };
        // SAFETY: `windowNumber` is an NSInteger property of NSWindow.
        let number: isize = unsafe { msg_send![ns_window, windowNumber] };
        if number <= 0 {
            return false;
        }
        // SAFETY: (connection, window number, radius), as MonoCode calls it.
        unsafe { set_blur(connection, number as c_int, c_int::from(radius)) == 0 }
    }

    /// Fade `window-vibrancy`'s effect view to 1%: the WindowServer blur
    /// shows through, and AppKit keeps compositing the backdrop.
    pub fn dim_vibrancy(window: &WebviewWindow) {
        let Some(ns_window) = ns_window(window) else {
            return;
        };
        // SAFETY: plain NSWindow/NSView messages on the main thread; nil is checked.
        unsafe {
            let content: *mut AnyObject = msg_send![ns_window, contentView];
            let Some(content) = content.as_ref() else {
                return;
            };
            let view: *mut AnyObject = msg_send![content, viewWithTag: VIBRANCY_VIEW_TAG];
            if let Some(view) = view.as_ref() {
                let () = msg_send![view, setAlphaValue: 0.01f64];
            }
        }
    }
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
        assert_eq!(BLUR_SUPPORTED, cfg!(target_os = "macos"));
    }

    #[test]
    fn blur_radius_stays_in_range() {
        assert_eq!(blur_radius(0.0), 1);
        assert_eq!(blur_radius(24.4), 24);
        assert_eq!(blur_radius(500.0), 64);
        assert_eq!(blur_radius(f64::NAN), 24);
    }
}
