//! Appearance settings: theme, accent, tint, window glass strength, interface
//! scale and the collapsed sidebar rail. Not secret; stored in settings.json.
//!
//! The settings, ranges and defaults follow MonoCode's Settings → Appearance
//! (`src/features/settings/model/appearance.ts` and `uiScale.ts`, MIT,
//! Copyright (c) 2026 Nick; see THIRD_PARTY_NOTICES.md). The renderer mirrors
//! them in `src/shared/appearance.ts`; keep the two in step.
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

pub const HUE: Range = Range::new(0.0, 360.0, 240.0);
pub const SATURATION: Range = Range::new(0.0, 100.0, 0.0);
pub const DARK_LIGHTNESS: Range = Range::new(0.0, 30.0, 9.0);
pub const SIDEBAR_OPACITY: Range = Range::new(0.15, 1.0, 0.85);
pub const BLUR_RADIUS: Range = Range::new(1.0, 64.0, 24.0);
pub const UI_SCALE: Range = Range::new(0.5, 2.0, 1.0);
pub const UI_SCALE_STEP: f64 = 0.1;

#[derive(Debug, Clone, Copy)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

impl Range {
    const fn new(min: f64, max: f64, default: f64) -> Self {
        Self { min, max, default }
    }

    fn clamp(self, value: f64) -> f64 {
        if value.is_finite() {
            value.clamp(self.min, self.max)
        } else {
            self.default
        }
    }

    fn contains(self, value: f64) -> bool {
        value.is_finite() && (self.min..=self.max).contains(&value)
    }
}

/// Follow the OS, or force a scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    System,
    Light,
    #[default]
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Appearance {
    pub theme: ThemePreference,
    /// `#rrggbb`, or None for Clipward's own accent.
    pub accent_color: Option<String>,
    /// Hue of the neutral chrome, in degrees.
    pub hue: f64,
    /// How strongly the hue tints the chrome, in percent. Zero is neutral grey.
    pub saturation: f64,
    /// Lightness of the dark theme's base, in percent. Zero is true black.
    pub dark_lightness: f64,
    /// Opacity of the sidebar (and of the main pane with `main_pane_glass`)
    /// over the window glass.
    pub sidebar_opacity: f64,
    /// macOS WindowServer blur behind the window, in points.
    pub blur_radius: f64,
    /// Extend the translucent treatment from the sidebar to the main pane.
    pub main_pane_glass: bool,
    /// Webview zoom, in steps of 0.1.
    pub ui_scale: f64,
    /// Show the sidebar as the 52px icon rail.
    pub collapsed_rail: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: ThemePreference::default(),
            accent_color: None,
            hue: HUE.default,
            saturation: SATURATION.default,
            dark_lightness: DARK_LIGHTNESS.default,
            sidebar_opacity: SIDEBAR_OPACITY.default,
            blur_radius: BLUR_RADIUS.default,
            main_pane_glass: true,
            ui_scale: UI_SCALE.default,
            collapsed_rail: false,
        }
    }
}

fn is_hex_color(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

fn round_to(value: f64, step: f64) -> f64 {
    (value / step).round() * step
}

/// Snap to the 0.1 steps without float drift (1.0 + 0.1 * 3 is 1.3, not 1.3000000000000003).
fn round_scale(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

impl Appearance {
    /// Every value clamped into range and snapped to its slider's step.
    pub fn normalized(self) -> Self {
        Self {
            accent_color: self
                .accent_color
                .filter(|color| is_hex_color(color))
                .map(|color| color.to_ascii_lowercase()),
            hue: HUE.clamp(self.hue).round(),
            saturation: SATURATION.clamp(self.saturation).round(),
            dark_lightness: DARK_LIGHTNESS.clamp(self.dark_lightness).round(),
            sidebar_opacity: round_to(SIDEBAR_OPACITY.clamp(self.sidebar_opacity), 0.01),
            blur_radius: BLUR_RADIUS.clamp(self.blur_radius).round(),
            ui_scale: round_scale(UI_SCALE.clamp(self.ui_scale)),
            ..self
        }
    }

    /// The same, but refuses values outside their ranges or a malformed colour.
    pub fn validated(self) -> Result<Self, String> {
        let checks = [
            ("hue", HUE, self.hue),
            ("saturation", SATURATION, self.saturation),
            ("dark-mode lightness", DARK_LIGHTNESS, self.dark_lightness),
            ("sidebar opacity", SIDEBAR_OPACITY, self.sidebar_opacity),
            ("blur radius", BLUR_RADIUS, self.blur_radius),
            ("interface scale", UI_SCALE, self.ui_scale),
        ];
        for (name, range, value) in checks {
            if !range.contains(value) {
                return Err(format!("Invalid {name}"));
            }
        }
        if self
            .accent_color
            .as_deref()
            .is_some_and(|color| !is_hex_color(color))
        {
            return Err("Invalid accent color".into());
        }
        Ok(self.normalized())
    }

    /// Read a stored object field by field: a missing or malformed field takes
    /// its default and the others survive, as MonoCode's per-key storage does.
    fn from_stored(value: &Map<String, Value>) -> Self {
        let defaults = Self::default();
        let number =
            |key: &str, default: f64| value.get(key).and_then(Value::as_f64).unwrap_or(default);
        let flag =
            |key: &str, default: bool| value.get(key).and_then(Value::as_bool).unwrap_or(default);
        Self {
            theme: value
                .get("theme")
                .and_then(|theme| ThemePreference::deserialize(theme).ok())
                .unwrap_or(defaults.theme),
            accent_color: value
                .get("accentColor")
                .and_then(Value::as_str)
                .map(str::to_string),
            hue: number("hue", defaults.hue),
            saturation: number("saturation", defaults.saturation),
            dark_lightness: number("darkLightness", defaults.dark_lightness),
            sidebar_opacity: number("sidebarOpacity", defaults.sidebar_opacity),
            blur_radius: number("blurRadius", defaults.blur_radius),
            main_pane_glass: flag("mainPaneGlass", defaults.main_pane_glass),
            ui_scale: number("uiScale", defaults.ui_scale),
            collapsed_rail: flag("collapsedRail", defaults.collapsed_rail),
        }
        .normalized()
    }
}

/// `settings.json` reader: never fails the whole file over appearance.
pub fn deserialize_lenient<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Appearance, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::Object(map) => Appearance::from_stored(&map),
        _ => Appearance::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_monocode() {
        let look = Appearance::default();
        assert_eq!(look.theme, ThemePreference::Dark);
        assert_eq!(look.accent_color, None);
        assert_eq!(
            (look.hue, look.saturation, look.dark_lightness),
            (240.0, 0.0, 9.0)
        );
        assert_eq!((look.sidebar_opacity, look.blur_radius), (0.85, 24.0));
        assert!(look.main_pane_glass);
        assert_eq!(look.ui_scale, 1.0);
        assert!(!look.collapsed_rail);
    }

    #[test]
    fn normalizing_clamps_and_snaps() {
        let look = Appearance {
            accent_color: Some("#AABBCC".into()),
            hue: 400.4,
            saturation: -3.0,
            dark_lightness: 100.0,
            sidebar_opacity: 0.0,
            blur_radius: 99.0,
            ui_scale: 1.05,
            ..Appearance::default()
        }
        .normalized();
        assert_eq!(look.accent_color.as_deref(), Some("#aabbcc"));
        assert_eq!(
            (look.hue, look.saturation, look.dark_lightness),
            (360.0, 0.0, 30.0)
        );
        assert_eq!((look.sidebar_opacity, look.blur_radius), (0.15, 64.0));
        assert_eq!(look.ui_scale, 1.1);
        let nan = Appearance {
            ui_scale: f64::NAN,
            accent_color: Some("tomato".into()),
            ..Appearance::default()
        }
        .normalized();
        assert_eq!(nan.ui_scale, 1.0);
        assert_eq!(nan.accent_color, None);
    }

    #[test]
    fn scale_steps_do_not_drift() {
        let mut scale = 1.0;
        for _ in 0..10 {
            scale = round_scale(UI_SCALE.clamp(scale + UI_SCALE_STEP));
        }
        assert_eq!(scale, UI_SCALE.max);
        assert_eq!(round_scale(1.0 + 0.1 * 3.0), 1.3);
    }

    #[test]
    fn validation_refuses_out_of_range_values() {
        assert!(Appearance::default().validated().is_ok());
        for bad in [
            Appearance {
                hue: -1.0,
                ..Appearance::default()
            },
            Appearance {
                saturation: 101.0,
                ..Appearance::default()
            },
            Appearance {
                dark_lightness: 31.0,
                ..Appearance::default()
            },
            Appearance {
                sidebar_opacity: 0.1,
                ..Appearance::default()
            },
            Appearance {
                blur_radius: 65.0,
                ..Appearance::default()
            },
            Appearance {
                ui_scale: 2.5,
                ..Appearance::default()
            },
            Appearance {
                ui_scale: f64::INFINITY,
                ..Appearance::default()
            },
            Appearance {
                accent_color: Some("red".into()),
                ..Appearance::default()
            },
        ] {
            assert!(bad.validated().is_err());
        }
    }

    #[test]
    fn renderer_shape_round_trips() {
        let json = r##"{"theme":"system","accentColor":"#10b981","hue":200,"saturation":12,"darkLightness":4,"sidebarOpacity":0.6,"blurRadius":40,"mainPaneGlass":false,"uiScale":1.2,"collapsedRail":true}"##;
        let look: Appearance = serde_json::from_str(json).unwrap();
        assert_eq!(look.theme, ThemePreference::System);
        assert!(!look.main_pane_glass);
        let back: Appearance =
            serde_json::from_value(serde_json::to_value(&look).unwrap()).unwrap();
        assert_eq!(back, look);
        assert!(serde_json::from_str::<Appearance>(r#"{"theme":"sepia"}"#).is_err());
    }
}
