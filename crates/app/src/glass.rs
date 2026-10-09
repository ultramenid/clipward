//! Window glass on macOS: the desktop blurred behind the window by a chosen radius
//! (Settings → Appearance → Blur radius).
//!
//! GPUI's `Blurred` background adds an `NSVisualEffectView` with a fixed material.
//! The WindowServer blur (`CGSSetWindowBackgroundBlurRadius`, a private call looked up
//! at runtime) sets the radius; that view is then faded to 1% alpha so the radius
//! shows, while AppKit keeps compositing the backdrop (without it, single frames show
//! unblurred). If the private call is missing, the view stays at full strength and
//! its material does the blurring.
#![cfg(target_os = "macos")]

use clipward_core::appearance::BLUR_RADIUS;
use gpui_kit::Window;
use objc2::msg_send;
use objc2::runtime::AnyObject;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::{c_int, c_void};
use std::sync::OnceLock;

type CgsConnection = c_int;
type ConnectionFn = unsafe extern "C" fn() -> CgsConnection;
type SetBlurFn = unsafe extern "C" fn(CgsConnection, c_int, c_int) -> c_int;

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const std::ffi::c_char) -> *mut c_void;
}
/// `RTLD_DEFAULT` on macOS.
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

fn dlsym_fn<T: Copy>(symbol: &[u8]) -> Option<T> {
    // SAFETY: `symbol` is NUL-terminated and T is a function pointer with the symbol's
    // C signature.
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
        dlsym_fn(b"CGSDefaultConnectionForThread\0").or_else(|| dlsym_fn(b"CGSMainConnectionID\0"))
    }))?;
    // SAFETY: takes no arguments and returns the connection id.
    let connection = unsafe { function() };
    (connection != 0).then_some(connection)
}

/// The window's NSWindow, through its content view.
fn ns_window(window: &Window) -> Option<*mut AnyObject> {
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw() else {
        return None;
    };
    let view = handle.ns_view.as_ptr().cast::<AnyObject>();
    // SAFETY: GPUI's live content view, messaged on the main thread.
    let window: *mut AnyObject = unsafe { msg_send![view, window] };
    (!window.is_null()).then_some(window)
}

/// The radius in points, clamped to the setting's range.
pub fn blur_radius(radius: f64) -> u8 {
    if radius.is_finite() {
        radius.round().clamp(BLUR_RADIUS.min, BLUR_RADIUS.max) as u8
    } else {
        BLUR_RADIUS.default as u8
    }
}

/// Blur the desktop behind the window by `radius` (0 turns it off) and fade GPUI's
/// effect view so the radius shows. Call after setting the `Blurred` background.
pub fn set_blur(window: &Window, radius: u8) {
    let Some(ns_window) = ns_window(window) else {
        return;
    };
    let applied = (|| {
        let set_blur = set_blur_fn()?;
        let connection = connection()?;
        // SAFETY: `windowNumber` is an NSInteger property of NSWindow.
        let number: isize = unsafe { msg_send![ns_window, windowNumber] };
        // SAFETY: (connection, window number, radius), as the Tauri shell called it.
        (number > 0 && unsafe { set_blur(connection, number as c_int, c_int::from(radius)) } == 0)
            .then_some(())
    })()
    .is_some();
    if applied && radius > 0 {
        fade_effect_view(ns_window);
    }
}

/// Fade GPUI's `BlurredView` (the bottom subview of the content view) to 1%.
fn fade_effect_view(ns_window: *mut AnyObject) {
    // SAFETY: plain NSWindow/NSView messages on the main thread; nil is checked.
    unsafe {
        let content: *mut AnyObject = msg_send![ns_window, contentView];
        let Some(content) = content.as_ref() else {
            return;
        };
        let subviews: *mut AnyObject = msg_send![content, subviews];
        let Some(subviews) = subviews.as_ref() else {
            return;
        };
        let count: usize = msg_send![subviews, count];
        for index in 0..count {
            let view: *mut AnyObject = msg_send![subviews, objectAtIndex: index];
            let Some(view) = view.as_ref() else {
                continue;
            };
            let class: *const AnyObject = msg_send![view, class];
            let name: *mut AnyObject = msg_send![class, description];
            let Some(name) = name.as_ref() else {
                continue;
            };
            let utf8: *const std::ffi::c_char = msg_send![name, UTF8String];
            if !utf8.is_null() && std::ffi::CStr::from_ptr(utf8).to_bytes() == b"BlurredView" {
                let () = msg_send![view, setAlphaValue: 0.01f64];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_radius_stays_in_range() {
        assert_eq!(blur_radius(0.0), 1);
        assert_eq!(blur_radius(24.4), 24);
        assert_eq!(blur_radius(500.0), 64);
        assert_eq!(blur_radius(f64::NAN), 24);
    }
}
