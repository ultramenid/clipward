//! Clipward's colours: the design tokens (see DESIGN.md),
//! computed from Settings → Appearance and the resolved colour scheme.
//!
//! Every fill, border and hover is the content colour at a low strength over the
//! base; only status earns a hue. The same values are projected onto gpui-kit's
//! theme so its components (inputs, selects, sliders, dialogs) match.
use clipward_core::appearance::{Appearance, ThemePreference};
use gpui_kit::component::{Theme as KitTheme, ThemeMode};
use gpui_kit::{App, Global, Hsla, WindowAppearance, hsla};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub dark: bool,
    /// Window base (`--bg`).
    pub bg: Hsla,
    pub sidebar: Hsla,
    /// Solid popover fallback.
    pub elevated: Hsla,
    /// Text, and the source of every tint (`--content`).
    pub ink: Hsla,
    /// Anything the user must read besides the main text.
    pub ink_secondary: Hsla,
    /// Group labels, shortcuts, counts, placeholders.
    pub ink_tertiary: Hsla,
    /// Structural hairlines.
    pub stroke: Hsla,
    pub selection_subtle: Hsla,
    pub selection: Hsla,
    pub selection_strong: Hsla,
    pub selection_hover: Hsla,
    pub accent: Hsla,
    /// Accent as text on the base.
    pub accent_fg: Hsla,
    /// Text on a solid accent fill.
    pub accent_ink: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    /// The virality score icon.
    pub brand_gold: Hsla,
    /// Base alpha of the window backdrop, sidebar, sticky strips and cards over glass.
    pub glass_window: f32,
    pub glass_sidebar: f32,
    pub glass_strip: f32,
    pub glass_surface: f32,
    /// The window's rem, CSS px: 16 × Settings → Appearance → Interface scale. Every
    /// `ui::u` size scales with it.
    pub rem: f32,
}

impl Global for Tokens {}

fn rgb(r: u8, g: u8, b: u8) -> Hsla {
    gpui_kit::Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
    .into()
}

/// `#rrggbb` → colour; None when malformed.
pub fn hex(value: &str) -> Option<Hsla> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(digits, 16).ok()?;
    Some(rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// `color` lightened toward white (`amount` > 0) or darkened toward black (< 0).
fn mix(color: Hsla, amount: f32) -> Hsla {
    let c = gpui_kit::Rgba::from(color);
    let toward = if amount >= 0.0 { 1.0 } else { 0.0 };
    let t = amount.abs();
    gpui_kit::Rgba {
        r: c.r + (toward - c.r) * t,
        g: c.g + (toward - c.g) * t,
        b: c.b + (toward - c.b) * t,
        a: c.a,
    }
    .into()
}

/// WCAG relative luminance above 0.179 takes black text (shared/appearance.ts).
fn ink_on(color: Hsla) -> Hsla {
    let c = gpui_kit::Rgba::from(color);
    let channel = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b);
    if luminance > 0.179 {
        rgb(0, 0, 0)
    } else {
        rgb(255, 255, 255)
    }
}

/// Whether the scheme is dark, from the preference and the OS appearance.
pub fn resolve_dark(preference: ThemePreference, os: WindowAppearance) -> bool {
    match preference {
        ThemePreference::Dark => true,
        ThemePreference::Light => false,
        ThemePreference::System => {
            matches!(os, WindowAppearance::Dark | WindowAppearance::VibrantDark)
        }
    }
}

impl Tokens {
    /// The tokens for an appearance. `glass` is whether window glass is on and
    /// supported (and transparency isn't reduced).
    pub fn new(appearance: &Appearance, dark: bool, glass: bool) -> Self {
        let hue = (appearance.hue as f32).rem_euclid(360.0) / 360.0;
        let sat = (appearance.saturation as f32 / 100.0).clamp(0.0, 1.0);
        let neutral = |lightness: f32, alpha: f32| hsla(hue, sat, lightness, alpha);
        let base = (appearance.dark_lightness as f32 / 100.0).clamp(0.0, 0.3);

        let (bg_l, sidebar_l, elevated_l, content_l) = if dark {
            (base, base * 0.9, base + 0.03, 0.92)
        } else {
            (0.97, 0.97, 1.0, 0.18)
        };
        // Text emphasis rises with the dark base (and over glass) to keep contrast.
        let lift = ((base - 0.09) * 1.1).max(0.0);
        let glass_dark = glass && dark;
        let (secondary, tertiary) = if !dark {
            (0.70, 0.54)
        } else if glass_dark {
            (0.64 + lift, 0.48 + lift)
        } else {
            (0.56 + lift, 0.40 + lift)
        };
        let (sel_subtle, sel, sel_strong, sel_hover) = if dark {
            (0.08, 0.10, 0.12, 0.15)
        } else {
            (0.05, 0.06, 0.07, 0.10)
        };

        let user_accent = appearance.accent_color.as_deref().and_then(hex);
        let (accent, accent_fg, accent_ink) = match user_accent {
            Some(color) => (
                color,
                mix(color, if dark { 0.3 } else { -0.25 }),
                ink_on(color),
            ),
            None if dark => (rgb(235, 235, 235), rgb(235, 235, 235), rgb(23, 23, 23)),
            None => (rgb(46, 46, 46), rgb(46, 46, 46), rgb(255, 255, 255)),
        };
        let (success, danger, warning) = if dark {
            (rgb(74, 222, 128), rgb(248, 113, 113), rgb(251, 191, 36))
        } else {
            (rgb(22, 150, 70), rgb(214, 48, 48), rgb(176, 118, 0))
        };
        let sidebar_opacity = appearance.sidebar_opacity as f32;
        let (glass_window, glass_sidebar, glass_strip, glass_surface) = if glass_dark {
            let main = if appearance.main_pane_glass {
                sidebar_opacity
            } else {
                1.0
            };
            (main, sidebar_opacity, 0.82, 0.72)
        } else {
            (1.0, 1.0, 1.0, 0.0)
        };

        Self {
            rem: 16.0 * appearance.ui_scale as f32,
            dark,
            bg: neutral(bg_l, 1.0),
            sidebar: neutral(sidebar_l, 1.0),
            elevated: neutral(elevated_l, 1.0),
            ink: neutral(content_l, 1.0),
            ink_secondary: neutral(content_l, secondary),
            ink_tertiary: neutral(content_l, tertiary),
            stroke: neutral(content_l, 0.07),
            selection_subtle: neutral(content_l, sel_subtle),
            selection: neutral(content_l, sel),
            selection_strong: neutral(content_l, sel_strong),
            selection_hover: neutral(content_l, sel_hover),
            accent,
            accent_fg,
            accent_ink,
            success,
            danger,
            warning,
            brand_gold: rgb(255, 213, 0),
            glass_window,
            glass_sidebar,
            glass_strip,
            glass_surface,
        }
    }

    /// The content colour at `strength` (0..1): `bg-content/5` is `tint(0.05)`.
    pub fn tint(&self, strength: f32) -> Hsla {
        self.ink.opacity(strength)
    }

    /// Make these the app's colours, including gpui-kit's components.
    pub fn install(self, cx: &mut App) {
        KitTheme::change(
            if self.dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            None,
            cx,
        );
        let t = self;
        // A second edit: `change` reloads the mode's stock colours over any in the same call.
        KitTheme::update(cx, |theme| {
            let c = &mut theme.colors;
            c.background = t.bg;
            c.foreground = t.ink;
            c.border = t.stroke;
            c.input = t.tint(0.12);
            c.ring = t.accent;
            c.caret = t.ink;
            c.selection = t.accent.opacity(0.3);
            c.muted = t.tint(0.05);
            c.muted_foreground = t.ink_secondary;
            c.accent = t.selection;
            c.accent_foreground = t.ink;
            c.popover = t.elevated;
            c.popover_foreground = t.ink;
            c.primary = t.accent;
            c.primary_hover = t.accent.opacity(0.9);
            c.primary_active = t.accent.opacity(0.8);
            c.primary_foreground = t.accent_ink;
            c.secondary = t.tint(0.06);
            c.secondary_hover = t.tint(0.1);
            c.secondary_active = t.tint(0.14);
            c.secondary_foreground = t.ink;
            c.button_primary = t.accent;
            c.button_primary_hover = t.accent.opacity(0.9);
            c.button_primary_active = t.accent.opacity(0.8);
            c.button_primary_foreground = t.accent_ink;
            c.button = t.tint(0.06);
            c.button_hover = t.tint(0.1);
            c.button_active = t.tint(0.14);
            c.button_foreground = t.ink;
            c.danger = t.danger;
            c.danger_foreground = t.accent_ink;
            c.success = t.success;
            c.warning = t.warning;
            c.list = t.bg;
            c.list_hover = t.selection_subtle;
            c.list_active = t.selection;
            c.list_active_border = t.accent;
            c.sidebar = t.sidebar;
            c.sidebar_foreground = t.ink;
            c.sidebar_border = t.stroke;
            c.sidebar_accent = t.selection;
            c.sidebar_accent_foreground = t.ink;
            c.sidebar_primary = t.accent;
            c.sidebar_primary_foreground = t.accent_ink;
            c.slider_bar = t.accent;
            c.slider_thumb = t.accent;
            c.switch = t.tint(0.18);
            c.switch_thumb = t.bg;
            c.progress_bar = t.accent;
            c.skeleton = t.tint(0.06);
            c.scrollbar = gpui_kit::transparent_black();
            c.scrollbar_thumb = t.tint(0.2);
            c.scrollbar_thumb_hover = t.tint(0.3);
            c.tab_bar_segmented = t.tint(0.05);
            c.tab_active = t.selection;
            c.tab_active_foreground = t.ink;
            c.tab_foreground = t.ink_secondary;
            c.title_bar = t.bg;
            c.title_bar_border = t.stroke;
            c.window_border = t.stroke;
            c.overlay = gpui_kit::black().opacity(0.5);
            c.link = t.accent_fg;
            theme.radius = gpui_kit::px(6.0);
            theme.radius_lg = gpui_kit::px(8.0);
            // The UI kit's root sets the window's rem to this on every frame, so the
            // interface scale goes here (a `set_rem_size` alone would be overwritten).
            theme.font_size = gpui_kit::px(t.rem);
        });
        cx.set_global(t);
        cx.refresh_windows();
    }
}

/// The app's tokens, for render code: `cx.tokens().ink`.
pub trait ActiveTokens {
    fn tokens(&self) -> &Tokens;
}

impl ActiveTokens for App {
    fn tokens(&self) -> &Tokens {
        self.global::<Tokens>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dark_base_is_171717() {
        let t = Tokens::new(&Appearance::default(), true, false);
        let bg = gpui_kit::Rgba::from(t.bg);
        assert_eq!((bg.r * 255.0).round() as u8, 23);
        assert_eq!(t.glass_window, 1.0);
    }

    #[test]
    fn light_mode_is_always_opaque() {
        let t = Tokens::new(&Appearance::default(), false, true);
        assert_eq!(
            (t.glass_window, t.glass_sidebar, t.glass_surface),
            (1.0, 1.0, 0.0)
        );
    }

    #[test]
    fn user_accent_picks_readable_ink() {
        let appearance = Appearance {
            accent_color: Some("#ffd500".into()),
            ..Appearance::default()
        };
        let t = Tokens::new(&appearance, true, false);
        assert_eq!(t.accent_ink, rgb(0, 0, 0));
    }
}
