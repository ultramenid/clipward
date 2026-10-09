//! Hex ↔ HSV for the custom colour picker, and the channel strings the theme's colour
//! tokens use. Ported from MonoCode's `src/shared/lib/colorUtils.ts` via
//! src/renderer/lib/color.ts (MIT, Copyright (c) 2026 Nick; see THIRD_PARTY_NOTICES.md).
pub use crate::appearance::is_hex_color;
use crate::format::js_round;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsv {
    /// Degrees, 0–360.
    pub h: f64,
    /// Percent.
    pub s: f64,
    /// Percent.
    pub v: f64,
}

/// `#rrggbb` in lower case; anything unreadable becomes mid grey.
pub fn normalize_hex(color: &str) -> String {
    if is_hex_color(color) {
        return color.to_ascii_lowercase();
    }
    // The renderer let a canvas read the rest. Of what a canvas reads, only #rgb and
    // padded hex can reach here.
    // ponytail: no colour names, rgb() or hsl(); nothing passes them (the picker sends hex).
    let text = color.trim_matches(|c: char| c.is_ascii_whitespace());
    match text.strip_prefix('#') {
        Some(digits) if digits.len() == 3 && digits.bytes().all(|b| b.is_ascii_hexdigit()) => {
            let doubled: String = digits.chars().flat_map(|c| [c, c]).collect();
            format!("#{}", doubled.to_ascii_lowercase())
        }
        _ if is_hex_color(text) => text.to_ascii_lowercase(),
        _ => "#808080".into(),
    }
}

/// The red, green and blue bytes of a colour, after `normalize_hex`.
fn channels(hex: &str) -> [u8; 3] {
    let value = normalize_hex(hex);
    [1, 3, 5].map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).unwrap_or(0))
}

pub fn hex_to_hsv(hex: &str) -> Hsv {
    let [r, g, b] = channels(hex).map(|channel| f64::from(channel) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let mut h = 0.0;
    if delta != 0.0 {
        h = if max == r {
            ((g - b) / delta) % 6.0
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        };
        h *= 60.0;
        if h < 0.0 {
            h += 360.0;
        }
    }

    let s = if max == 0.0 { 0.0 } else { delta / max };
    Hsv {
        h,
        s: s * 100.0,
        v: max * 100.0,
    }
}

pub fn hsv_to_hex(h: f64, s: f64, v: f64) -> String {
    let sat = s.clamp(0.0, 100.0) / 100.0;
    let val = v.clamp(0.0, 100.0) / 100.0;
    let hue = ((h % 360.0) + 360.0) % 360.0;
    let c = val * sat;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = val - c;

    let (r, g, b) = if hue < 60.0 {
        (c, x, 0.0)
    } else if hue < 120.0 {
        (x, c, 0.0)
    } else if hue < 180.0 {
        (0.0, c, x)
    } else if hue < 240.0 {
        (0.0, x, c)
    } else if hue < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    let byte = |channel: f64| js_round(((channel + m) * 255.0).clamp(0.0, 255.0)) as u8;
    format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b))
}

/// `#rrggbb` as the space-separated channels Clipward's colour tokens use.
pub fn hex_to_channels(hex: &str) -> String {
    let [r, g, b] = channels(hex);
    format!("{r} {g} {b}")
}

/// Channels of `hex` moved `amount` (0–1) toward `target` (0 black, 255 white).
pub fn mix_channels(hex: &str, target: u8, amount: f64) -> String {
    channels(hex)
        .map(|channel| {
            let channel = f64::from(channel);
            (js_round(channel + (f64::from(target) - channel) * amount) + 0.0).to_string()
        })
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_channels_for_the_theme_tokens() {
        assert_eq!(hex_to_channels("#4da3f5"), "77 163 245");
        assert_eq!(mix_channels("#000000", 255, 0.5), "128 128 128");
        assert_eq!(mix_channels("#ffffff", 0, 0.25), "191 191 191");
    }

    #[test]
    fn the_color_picker_round_trips_hex_through_hsv() {
        for hex in ["#4da3f5", "#ec4899", "#10b981", "#000000", "#ffffff"] {
            let Hsv { h, s, v } = hex_to_hsv(hex);
            assert_eq!(hsv_to_hex(h, s, v), hex);
        }
        assert_eq!(hsv_to_hex(0.0, 100.0, 100.0), "#ff0000");
        assert_eq!(hsv_to_hex(-120.0, 100.0, 100.0), "#0000ff");
    }

    #[test]
    fn hex_is_normalized_like_a_canvas_reads_it() {
        assert_eq!(normalize_hex("#AABBCC"), "#aabbcc");
        assert_eq!(normalize_hex(" #ABC "), "#aabbcc");
        assert_eq!(normalize_hex("#aabbcc80"), "#808080");
        assert_eq!(normalize_hex("junk"), "#808080");
        assert_eq!(hex_to_channels("junk"), "128 128 128");
    }
}
