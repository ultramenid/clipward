//! The 13 caption looks (CaptionPresetPicker.tsx PRESETS) and how GPUI draws them:
//! shared by the Create page's picker and the editor's text layers.
//!
//! GPUI text has no text-shadow, so each effect is drawn as copies of the text
//! behind the fill: the stroke as 16 copies in a ring, a drop shadow as an offset
//! copy (a soft one as a few faint copies), a halo or glow as faint copies in
//! widening rings.
use crate::state;
use crate::ui::u;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Shadow {
    Soft,
    Hard,
    Halo,
    None,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Future {
    Show,
    Dim,
    Hide,
}

pub(crate) struct Preset {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) font: &'static str,
    pub(crate) weight: f32,
    pub(crate) italic: bool,
    /// Text size in px for an 84px-tall sample.
    pub(crate) size: f32,
    pub(crate) primary: u32,
    pub(crate) highlight: u32,
    pub(crate) stroke: f32,
    pub(crate) shadow: Shadow,
    pub(crate) uppercase: bool,
    /// Rounded pill behind the active word.
    pub(crate) pill: Option<u32>,
    /// Bloom around the active word.
    pub(crate) glow: Option<u32>,
    /// Translucent plate behind the line: colour and alpha.
    pub(crate) plate: Option<(u32, f32)>,
    pub(crate) karaoke: bool,
    pub(crate) future: Future,
    pub(crate) words: [&'static str; 3],
}

pub(crate) const BASE: Preset = Preset {
    id: "",
    name: "",
    description: "",
    font: "Montserrat",
    weight: 900.,
    italic: false,
    size: 15.,
    primary: 0xFFFFFF,
    highlight: 0xFFFFFF,
    stroke: 0.,
    shadow: Shadow::None,
    uppercase: true,
    pill: None,
    glow: None,
    plate: None,
    karaoke: false,
    future: Future::Show,
    words: ["", "", ""],
};

pub(crate) const PRESETS: [Preset; 13] = [
    Preset {
        id: "pop",
        name: "Pop",
        description: "The all-rounder",
        highlight: 0xFFE234,
        stroke: 6.,
        shadow: Shadow::Soft,
        words: ["this", "changed", "everything"],
        ..BASE
    },
    Preset {
        id: "spotlight",
        name: "Spotlight",
        description: "Word on a pill",
        font: "Poppins",
        size: 14.,
        stroke: 5.,
        shadow: Shadow::Soft,
        pill: Some(0x7C5CFF),
        words: ["the", "real", "secret"],
        ..BASE
    },
    Preset {
        id: "impact",
        name: "Impact",
        description: "Tall, two words at a time",
        font: "Anton",
        weight: 400.,
        size: 21.,
        highlight: 0xFFD60A,
        stroke: 7.,
        shadow: Shadow::Hard,
        future: Future::Hide,
        words: ["ten", "million", "views"],
        ..BASE
    },
    Preset {
        id: "glow",
        name: "Glow",
        description: "Cyan bloom, tech & gaming",
        weight: 800.,
        highlight: 0x7DF9FF,
        shadow: Shadow::Halo,
        glow: Some(0x00C8FF),
        words: ["level", "up", "now"],
        ..BASE
    },
    Preset {
        id: "boxed",
        name: "Boxed",
        description: "Readable on any footage",
        font: "Archivo Black",
        weight: 400.,
        size: 13.,
        highlight: 0xFFD23F,
        plate: Some((0x000000, 0.62)),
        words: ["ship it", "faster", "today"],
        ..BASE
    },
    Preset {
        id: "sweep",
        name: "Sweep",
        description: "Colour follows the voice",
        font: "Poppins",
        weight: 800.,
        size: 14.,
        highlight: 0xFF5FA2,
        stroke: 5.,
        shadow: Shadow::Soft,
        karaoke: true,
        words: ["sing", "along", "now"],
        ..BASE
    },
    Preset {
        id: "editorial",
        name: "Editorial",
        description: "Serif for podcasts & stories",
        font: "Instrument Serif",
        weight: 400.,
        italic: true,
        size: 20.,
        highlight: 0xFFE6B8,
        shadow: Shadow::Halo,
        uppercase: false,
        future: Future::Dim,
        words: ["and that is", "why", "it works"],
        ..BASE
    },
    Preset {
        id: "hype",
        name: "Hype",
        description: "Heavy stroke, high energy",
        size: 16.,
        highlight: 0x39FF6A,
        stroke: 8.,
        shadow: Shadow::Hard,
        words: ["let’s", "go", "now"],
        ..BASE
    },
    Preset {
        id: "punch",
        name: "Punch",
        description: "One huge word at a time",
        font: "Anton",
        weight: 400.,
        size: 28.,
        stroke: 8.,
        shadow: Shadow::Hard,
        words: ["", "boom", ""],
        ..BASE
    },
    Preset {
        id: "neon",
        name: "Neon",
        description: "Magenta bloom, music & lifestyle",
        font: "Poppins",
        weight: 800.,
        size: 14.,
        highlight: 0xFF9CEB,
        shadow: Shadow::Halo,
        glow: Some(0xFF2EC4),
        words: ["feel", "the", "beat"],
        ..BASE
    },
    Preset {
        id: "headline",
        name: "Headline",
        description: "Word on a red news tag",
        font: "Archivo Black",
        weight: 400.,
        size: 13.,
        stroke: 4.,
        shadow: Shadow::Soft,
        pill: Some(0xE5202E),
        words: ["this", "just", "in"],
        ..BASE
    },
    Preset {
        id: "paper",
        name: "Paper",
        description: "Dark type on a white card",
        font: "Poppins",
        weight: 800.,
        size: 13.,
        primary: 0x111111,
        highlight: 0x6D28D9,
        uppercase: false,
        plate: Some((0xFFFFFF, 0.94)),
        words: ["here is", "how", "it works"],
        ..BASE
    },
    Preset {
        id: "subtle",
        name: "Subtle",
        description: "Light touch for interviews & vlogs",
        weight: 800.,
        size: 13.,
        highlight: 0xC4F1FF,
        shadow: Shadow::Halo,
        uppercase: false,
        future: Future::Dim,
        words: ["I think", "that’s", "fair"],
        ..BASE
    },
];

pub(crate) fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Offsets (dx, dy) on a ring of `radius`, `steps` around.
pub(crate) fn ring(radius: f32, steps: usize) -> impl Iterator<Item = (f32, f32)> {
    (0..steps).map(move |i| {
        let angle = i as f32 / steps as f32 * std::f32::consts::TAU;
        (angle.cos() * radius, angle.sin() * radius)
    })
}

/// Display name of a preset, for Review.
pub fn preset_name(id: &str) -> &str {
    PRESETS.iter().find(|p| p.id == id).map_or(id, |p| p.name)
}

/// Register the engine's caption fonts, so the samples use the faces clips get.
pub fn load_fonts(cx: &mut App) {
    let dir = state::core(cx).paths.assets_dir().join("fonts");
    let fonts: Vec<Cow<'static, [u8]>> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "ttf"))
        .filter_map(|path| std::fs::read(path).ok())
        .map(Cow::Owned)
        .collect();
    // Without them the samples fall back to the system face.
    let _ = cx.text_system().add_fonts(fonts);
}

/// The look for a preset id (the first preset when unknown, as presetTextStyle did).
pub(crate) fn preset(id: &str) -> &'static Preset {
    PRESETS.iter().find(|p| p.id == id).unwrap_or(&PRESETS[0])
}

/// A text layer in its preset's look (CaptionPresetPicker.tsx `presetTextStyle`),
/// for the editor's instant preview. `lines` are words with whether each shows yet
/// (typewriter: words still to come keep their place but don't show). `font` is
/// the text size on screen; `unit` turns the TS style's px (stroke widths, shadow
/// offsets, at the layer's size) into screen px.
pub(crate) fn text_layer(id: &str, lines: &[Vec<(String, bool)>], font: Pixels, unit: f32) -> Div {
    let p = preset(id);
    let w = p.stroke;
    let black = |alpha: f32| hsla(0., 0., 0., alpha);
    // Back to front, as text-shadow layers paint: stroke ring, shadow, glow; then the fill.
    let mut copies: Vec<(f32, f32, Hsla)> = Vec::new();
    if let Some(glow) = p.glow {
        let glow = color(glow);
        for (radius, alpha) in [(20., 0.12), (8., 0.3)] {
            copies.extend(ring(radius * unit / 2., 12).map(|(x, y)| (x, y, glow.opacity(alpha))));
        }
    }
    match p.shadow {
        Shadow::Soft => copies.push((0., (w + 1.5) * unit, black(0.6))),
        Shadow::Hard => copies.push((0., (w + 2.5) * unit, black(0.9))),
        Shadow::Halo => {
            copies.extend(ring(3. * unit, 12).map(|(x, y)| (x, y + unit, black(0.25))));
            copies.extend(ring(1.5 * unit, 8).map(|(x, y)| (x, y, black(0.35))));
        }
        Shadow::None => {}
    }
    if w > 0. {
        copies.extend(ring(w * unit, 16).map(|(x, y)| (x, y, black(1.))));
    }
    let block = |fill: Option<Hsla>| {
        div()
            .flex()
            .flex_col()
            .items_center()
            .children(lines.iter().map(|line| {
                div().flex().children(line.iter().enumerate().map(|(i, (word, shown))| {
                    let text = if p.uppercase { word.to_uppercase() } else { word.clone() };
                    let text = if i > 0 { format!(" {text}") } else { text };
                    let colour = match (fill, shown) {
                        (_, false) => transparent_black(),
                        (Some(c), true) => c,
                        (None, true) => color(p.primary),
                    };
                    div().whitespace_nowrap().text_color(colour).child(text)
                }))
            }))
    };
    let text = div()
        .relative()
        .font_family(p.font)
        .font_weight(FontWeight(p.weight))
        .when(p.italic, |d| d.italic())
        .text_size(font)
        .line_height(relative(1.15))
        .children(copies.into_iter().map(|(x, y, c)| {
            block(Some(c)).absolute().top(px(y)).left(px(x)).w_full()
        }))
        .child(block(None));
    match p.plate {
        Some((hex, alpha)) => div()
            .rounded(px(12. * unit))
            .px(px(14. * unit))
            .py(px(4. * unit))
            .bg(color(hex).opacity(alpha))
            .child(text),
        None => text,
    }
}

// ---- style samples -----------------------------------------------------------------
// The caption looks as small live samples (the Create page's picker and the editor's
// captions panel). GPUI text has no text-shadow, so each effect is drawn as copies
// of the line behind the fill: the stroke as 16 copies in a ring, a drop shadow as
// an offset copy (a soft one as a few faint copies), a halo or glow as faint copies
// in widening rings.

/// Preset `size` and `stroke` are tuned for an 84px-tall preview; a sample's scene
/// is 60px tall.
const SAMPLE_SCALE: f32 = 60. / 84.;
/// The stand-in "frame" behind each sample.
pub(crate) const SCENE: u32 = 0x14161d;

/// How one copy of the line is painted.
#[derive(Clone, Copy)]
enum Paint {
    /// The visible text.
    Fill,
    /// A stroke or shadow copy: every word in this colour.
    Shade(Hsla),
    /// A bloom copy: only the active word, in this colour.
    Bloom(Hsla),
}

fn line(p: &Preset, paint: Paint) -> Div {
    let k = SAMPLE_SCALE;
    let case = |word: &str| {
        if p.uppercase {
            word.to_uppercase()
        } else {
            word.to_string()
        }
    };
    let [before, active, after] = p.words;
    let clear = transparent_black();
    let (before_color, after_color) = match paint {
        Paint::Fill => (
            color(if p.karaoke { p.highlight } else { p.primary }),
            match p.future {
                Future::Show => color(p.primary),
                Future::Dim => color(p.primary).opacity(0.6),
                Future::Hide => clear,
            },
        ),
        Paint::Shade(shade) => (
            shade,
            match p.future {
                Future::Show => shade,
                Future::Dim => shade.opacity(shade.a * 0.6),
                Future::Hide => clear,
            },
        ),
        Paint::Bloom(_) => (clear, clear),
    };
    let word = |text: String, c: Hsla| div().text_color(c).child(text);
    let active_word = {
        let active = case(active);
        match (paint, p.pill) {
            (paint, Some(pill)) => div()
                .rounded(u(4. * k))
                .px(u(3. * k))
                .py(u(1. * k))
                .when(matches!(paint, Paint::Fill), |d| d.bg(color(pill)))
                .text_color(if matches!(paint, Paint::Fill) {
                    color(p.highlight)
                } else {
                    clear
                })
                .child(active),
            (Paint::Fill, None) if p.karaoke => {
                // The sweep mid-word.
                let sung = (active.chars().count() as f32 * 0.6).ceil() as usize;
                let at = active
                    .char_indices()
                    .nth(sung)
                    .map_or(active.len(), |(i, _)| i);
                let (sung, rest) = active.split_at(at);
                div()
                    .flex()
                    .child(word(sung.to_string(), color(p.highlight)))
                    .child(word(rest.to_string(), color(p.primary)))
            }
            (Paint::Fill, None) => word(active, color(p.highlight)),
            (Paint::Shade(c) | Paint::Bloom(c), None) => word(active, c),
        }
    };
    div()
        .flex()
        .flex_wrap()
        .justify_center()
        .gap_x(u(p.size * k * 0.28))
        .when(!before.is_empty(), |d| {
            d.child(word(case(before), before_color))
        })
        .child(active_word)
        .when(!after.is_empty(), |d| {
            d.child(word(case(after), after_color))
        })
}


/// One preset's look on a short line, its active word highlighted.
pub(crate) fn sample(p: &Preset) -> Div {
    let k = SAMPLE_SCALE;
    let w = p.stroke * 0.28 * k;
    let black = |alpha: f32| hsla(0., 0., 0., alpha);
    // Back to front: drop shadow or halo, bloom, stroke; then the fill.
    let mut layers: Vec<(f32, f32, Paint)> = Vec::new();
    match p.shadow {
        Shadow::Soft => {
            layers.push((0., w + 1.5, Paint::Shade(black(0.3))));
            layers.extend(ring(1., 6).map(|(x, y)| (x, w + 1.5 + y, Paint::Shade(black(0.12)))));
        }
        Shadow::Hard => layers.push((0., w + 2.5, Paint::Shade(black(0.9)))),
        Shadow::Halo => {
            layers.extend(ring(1.5, 8).map(|(x, y)| (x, y + 1., Paint::Shade(black(0.22)))));
            layers.extend(ring(3., 12).map(|(x, y)| (x, y + 1., Paint::Shade(black(0.1)))));
        }
        Shadow::None => {}
    }
    if let Some(glow) = p.glow {
        let glow = color(glow);
        for (radius, alpha) in [(3.5, 0.12), (2.2, 0.25), (1.1, 0.45)] {
            layers.extend(ring(radius, 12).map(|(x, y)| (x, y, Paint::Bloom(glow.opacity(alpha)))));
        }
    }
    if w > 0. {
        layers.extend(ring(w, 16).map(|(x, y)| (x, y, Paint::Shade(black(1.)))));
    }
    let text = div()
        .relative()
        .font_family(p.font)
        .font_weight(FontWeight(p.weight))
        .when(p.italic, |d| d.italic())
        .text_size(u(p.size * k))
        .line_height(relative(1.1))
        .children(
            layers
                .into_iter()
                .map(|(x, y, paint)| line(p, paint).absolute().top(u(y)).left(u(x)).w_full()),
        )
        .child(line(p, Paint::Fill));
    match p.plate {
        Some((hex, alpha)) => div()
            .rounded(u(4.))
            .px(u(4.))
            .py(px(1.))
            .bg(color(hex).opacity(alpha))
            .child(text),
        None => text,
    }
}
