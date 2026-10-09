//! Caption Generator - short-form ASS captions with word-by-word highlighting,
//! plus SRT sidecars. Port of caption_generator.py and config.py's caption presets.
//!
//! Each caption event is drawn as a stack of layers that share the same text, so
//! the glyphs line up exactly and every effect is a real blurred bitmap:
//!
//! ```text
//! 0 plate   - rounded translucent box behind the whole line
//! 1 shadow  - soft (blurred) drop shadow
//! 2 glow    - blurred bloom around the text or the active word
//! 3 pill    - rounded box behind the active word
//! 4 face    - the crisp text with its stroke
//! ```
use super::fonts;
use super::pyfmt;
use crate::error::{EngineError, Result};
use crate::types::{TranscriptSegment, TranscriptWord};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

// ------------------------------------------------------------------
// Styles and presets (config.py)
// ------------------------------------------------------------------

/// Caption styling. Pixel fields are integers, as in the Python dataclass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptionStyle {
    pub font_name: String,
    pub font_size: i64,
    pub bold: bool,
    pub italic: bool,
    pub uppercase: bool,
    pub letter_spacing: i64,
    /// top | center | bottom
    pub position: String,
    /// left | center | right
    pub alignment: String,
    pub max_words_per_line: i64,
    pub word_by_word_highlight: bool,
    pub primary_color: String,
    pub highlight_color: String,
    pub outline_color: String,
    pub outline_width: i64,
    pub shadow_color: String,
    pub shadow_opacity: f64,
    pub shadow_blur: i64,
    pub shadow_offset: i64,
    pub shadow_spread: i64,
    /// show | dim | hide
    pub future_words: String,
    pub dim_opacity: f64,
    pub entrance_pop: bool,
    pub karaoke_fill: bool,
    pub color_transition: bool,
    pub highlight_box_color: Option<String>,
    pub highlight_box_padding: i64,
    pub glow_color: Option<String>,
    pub glow_opacity: f64,
    pub glow_radius: i64,
    pub glow_blur: i64,
    pub glow_active_only: bool,
    pub line_box_color: Option<String>,
    pub line_box_opacity: f64,
    pub line_box_padding: i64,
    pub emphasis_color: Option<String>,
}

impl Default for CaptionStyle {
    fn default() -> Self {
        Self {
            font_name: "Montserrat Black".into(),
            font_size: 84,
            bold: false,
            italic: false,
            uppercase: true,
            letter_spacing: 0,
            position: "bottom".into(),
            alignment: "center".into(),
            max_words_per_line: 3,
            word_by_word_highlight: true,
            primary_color: "#FFFFFF".into(),
            highlight_color: "#FFE234".into(),
            outline_color: "#000000".into(),
            outline_width: 6,
            shadow_color: "#000000".into(),
            shadow_opacity: 0.6,
            shadow_blur: 10,
            shadow_offset: 7,
            shadow_spread: 2,
            future_words: "show".into(),
            dim_opacity: 0.45,
            entrance_pop: true,
            karaoke_fill: false,
            color_transition: false,
            highlight_box_color: None,
            highlight_box_padding: 16,
            glow_color: None,
            glow_opacity: 0.8,
            glow_radius: 8,
            glow_blur: 14,
            glow_active_only: true,
            line_box_color: None,
            line_box_opacity: 0.6,
            line_box_padding: 22,
            emphasis_color: None,
        }
    }
}

fn opt(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

impl CaptionStyle {
    fn highlight_box(&self) -> Option<&str> {
        opt(&self.highlight_box_color)
    }
    fn glow(&self) -> Option<&str> {
        opt(&self.glow_color)
    }
    fn line_box(&self) -> Option<&str> {
        opt(&self.line_box_color)
    }
    fn emphasis(&self) -> Option<&str> {
        opt(&self.emphasis_color)
    }
    /// The active word sits on a pill (never with karaoke).
    fn pill(&self) -> bool {
        self.highlight_box().is_some() && !self.karaoke_fill
    }
}

pub const CAPTION_PRESETS: [&str; 13] = [
    "pop", "spotlight", "impact", "glow", "boxed", "sweep", "editorial", "hype", "punch", "neon", "headline", "paper",
    "subtle",
];
pub const DEFAULT_CAPTION_PRESET: &str = "pop";

/// A CaptionStyle for a preset ID (config.get_caption_preset).
pub fn get_caption_preset(preset_id: &str) -> Result<CaptionStyle> {
    let mut s = CaptionStyle::default();
    let some = |c: &str| Some(c.to_string());
    match preset_id {
        "pop" => {
            s.emphasis_color = some("#3DFF8B");
        }
        "spotlight" => {
            s.font_name = "Poppins Black".into();
            s.font_size = 80;
            s.highlight_color = "#FFFFFF".into();
            s.outline_width = 5;
            s.highlight_box_color = some("#7C5CFF");
            s.highlight_box_padding = 16;
            s.emphasis_color = some("#FFE234");
        }
        "impact" => {
            s.font_name = "Anton".into();
            s.font_size = 124;
            s.letter_spacing = 1;
            s.max_words_per_line = 2;
            s.highlight_color = "#FFD60A".into();
            s.outline_width = 7;
            s.shadow_opacity = 0.9;
            s.shadow_blur = 1;
            s.shadow_offset = 10;
            s.future_words = "hide".into();
            s.emphasis_color = some("#FF4D4D");
        }
        "glow" => {
            s.font_name = "Montserrat ExtraBold".into();
            s.font_size = 80;
            s.highlight_color = "#7DF9FF".into();
            s.outline_width = 0;
            s.shadow_opacity = 0.75;
            s.shadow_blur = 12;
            s.shadow_offset = 4;
            s.shadow_spread = 5;
            s.glow_color = some("#00C8FF");
            s.glow_opacity = 0.85;
            s.glow_radius = 8;
            s.glow_blur = 16;
            s.emphasis_color = some("#FF6BD6");
        }
        "boxed" => {
            s.font_name = "Archivo Black".into();
            s.font_size = 70;
            s.highlight_color = "#FFD23F".into();
            s.outline_width = 0;
            s.shadow_opacity = 0.0;
            s.line_box_color = some("#000000");
            s.line_box_opacity = 0.62;
            s.line_box_padding = 22;
            s.emphasis_color = some("#4ADE80");
        }
        "sweep" => {
            s.font_name = "Poppins ExtraBold".into();
            s.font_size = 76;
            s.max_words_per_line = 4;
            s.highlight_color = "#FF5FA2".into();
            s.outline_width = 5;
            s.karaoke_fill = true;
            s.entrance_pop = false;
            s.dim_opacity = 1.0;
            s.emphasis_color = some("#FFE234");
        }
        "editorial" => {
            s.font_name = "Instrument Serif Italic".into();
            s.font_size = 112;
            s.italic = true;
            s.uppercase = false;
            s.max_words_per_line = 4;
            s.highlight_color = "#FFE6B8".into();
            s.outline_width = 0;
            s.shadow_opacity = 0.9;
            s.shadow_blur = 20;
            s.shadow_offset = 3;
            s.shadow_spread = 8;
            s.future_words = "dim".into();
            s.dim_opacity = 0.6;
            s.entrance_pop = false;
        }
        "hype" => {
            s.font_size = 88;
            s.highlight_color = "#39FF6A".into();
            s.outline_width = 8;
            s.shadow_opacity = 0.95;
            s.shadow_blur = 1;
            s.shadow_offset = 9;
            s.color_transition = true;
            s.emphasis_color = some("#FFE234");
        }
        "punch" => {
            s.font_name = "Anton".into();
            s.font_size = 150;
            s.letter_spacing = 1;
            s.max_words_per_line = 1;
            s.highlight_color = "#FFFFFF".into();
            s.outline_width = 8;
            s.shadow_opacity = 0.9;
            s.shadow_blur = 1;
            s.shadow_offset = 12;
            s.emphasis_color = some("#FF3B30");
        }
        "neon" => {
            s.font_name = "Poppins ExtraBold".into();
            s.font_size = 78;
            s.highlight_color = "#FF9CEB".into();
            s.outline_width = 0;
            s.shadow_opacity = 0.75;
            s.shadow_blur = 12;
            s.shadow_offset = 4;
            s.shadow_spread = 5;
            s.glow_color = some("#FF2EC4");
            s.glow_opacity = 0.9;
            s.glow_radius = 9;
            s.glow_blur = 18;
            s.emphasis_color = some("#7DF9FF");
        }
        "headline" => {
            s.font_name = "Archivo Black".into();
            s.font_size = 72;
            s.highlight_color = "#FFFFFF".into();
            s.outline_width = 4;
            s.highlight_box_color = some("#E5202E");
            s.highlight_box_padding = 14;
            s.emphasis_color = some("#FFD23F");
        }
        "paper" => {
            s.font_name = "Poppins ExtraBold".into();
            s.font_size = 68;
            s.uppercase = false;
            s.max_words_per_line = 4;
            s.primary_color = "#111111".into();
            s.highlight_color = "#6D28D9".into();
            s.outline_width = 0;
            s.shadow_opacity = 0.0;
            s.line_box_color = some("#FFFFFF");
            s.line_box_opacity = 0.94;
            s.line_box_padding = 22;
            s.entrance_pop = false;
            s.emphasis_color = some("#DB2777");
        }
        "subtle" => {
            s.font_name = "Montserrat ExtraBold".into();
            s.font_size = 70;
            s.uppercase = false;
            s.max_words_per_line = 4;
            s.highlight_color = "#C4F1FF".into();
            s.outline_width = 0;
            s.shadow_opacity = 0.85;
            s.shadow_blur = 16;
            s.shadow_offset = 3;
            s.shadow_spread = 6;
            s.future_words = "dim".into();
            s.dim_opacity = 0.55;
            s.entrance_pop = false;
            s.color_transition = true;
        }
        other => {
            return Err(EngineError::msg(format!(
                "Unknown caption preset: {other}. Valid presets: {CAPTION_PRESETS:?}"
            )))
        }
    }
    Ok(s)
}

/// The job's caption style: the named preset, falling back to "pop" (bridge_runner behavior).
pub fn caption_style_for(preset: &str) -> CaptionStyle {
    get_caption_preset(preset).unwrap_or_else(|_| get_caption_preset(DEFAULT_CAPTION_PRESET).expect("pop preset"))
}

// ------------------------------------------------------------------
// Generator
// ------------------------------------------------------------------

pub const SENTENCE_END_CHARS: [char; 3] = ['.', '!', '?'];
/// A group stays up at most this long after its last word ends.
pub const MAX_LINGER_MS: i64 = 700;
/// Group entrance: scale overshoot, then settle.
/// Text layer entrance and exit lengths, and how long a typewriter takes to type, ms.
const TEXT_IN_MS: i64 = 300;
const TEXT_OUT_MS: i64 = 250;
const TYPEWRITER_MS: i64 = 1_200;

pub const POP_TAGS: &str = r"\fscx82\fscy82\t(0,90,\fscx106\fscy106)\t(90,170,\fscx100\fscy100)";

pub const LAYER_PLATE: i64 = 0;
pub const LAYER_SHADOW: i64 = 1;
pub const LAYER_GLOW: i64 = 2;
pub const LAYER_PILL: i64 = 3;
pub const LAYER_FACE: i64 = 4;

pub const SRT_LINE_CHARS: usize = 42;
pub const SRT_MAX_CUE_MS: i64 = 6000;
pub const SRT_PAUSE_BREAK_MS: i64 = 700;
pub const SRT_LINGER_MS: i64 = 400;

/// Side margins of the base style; libass wraps lines wider than the rest.
pub const SIDE_MARGIN: i64 = 60;
/// Advance width per character (share of the font size) when the font can't be measured.
pub const FALLBACK_CHAR_EM: f64 = 0.75;

/// Comparison key for emphasis matching ("$14,500" -> "14500").
fn norm(word: &str) -> String {
    word.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect()
}

fn alignment_code(alignment: &str, position: &str) -> i64 {
    let col = match alignment {
        "left" => 0,
        "right" => 2,
        _ => 1,
    };
    let row = match position {
        "top" => [7, 8, 9],
        "center" => [4, 5, 6],
        _ => [1, 2, 3],
    };
    row[col]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Past,
    Active,
    Future,
}

#[derive(Debug, Clone)]
struct Token {
    text: String,
    state: State,
    emphasized: bool,
    karaoke_cs: i64,
}

/// Pins one group's events to a single position.
type PlaceEvents<'p> = &'p mut dyn FnMut(Vec<String>, &[String]) -> Vec<String>;

/// Pixel-placed caption blocks: (start_ms, end_ms, block_w, block_h) -> (alignment, y).
pub type Placer<'a> = &'a mut dyn FnMut(i64, i64, i64, i64) -> (i64, i64);

/// Everything `generate_captions` takes besides the transcript.
pub struct CaptionRequest<'a> {
    pub clip_start_ms: i64,
    pub clip_end_ms: i64,
    pub style: &'a CaptionStyle,
    pub output_width: i64,
    pub output_height: i64,
    pub video_region: Option<(i64, i64)>,
    /// (until_ms, alignment, y) in clip time, sorted.
    pub anchors: Option<&'a [(i64, i64, i64)]>,
    pub emphasis_words: &'a [String],
    /// Bundled fonts, for measuring caption blocks.
    pub fonts_dir: Option<&'a Path>,
}

/// ASS caption generator. One per caption file (holds the emphasis set).
pub struct CaptionGenerator<'a> {
    style: &'a CaptionStyle,
    emphasis: HashSet<String>,
    fonts_dir: Option<&'a Path>,
}

impl<'a> CaptionGenerator<'a> {
    pub fn new(style: &'a CaptionStyle, emphasis_words: &[String], fonts_dir: Option<&'a Path>) -> Self {
        let emphasis = if style.emphasis().is_some() {
            emphasis_words
                .iter()
                .flat_map(|w| w.split_whitespace())
                .map(norm)
                .filter(|k| k.chars().count() >= 2)
                .collect()
        } else {
            HashSet::new()
        };
        Self { style, emphasis, fonts_dir }
    }

    fn is_emphasis(&self, word: &str) -> bool {
        let key = norm(word);
        !key.is_empty() && (self.emphasis.contains(&key) || self.emphasis.contains(key.trim_end_matches('s')))
    }

    /// Caption text for a word: trailing commas/periods dropped, ASS braces removed.
    pub fn display_word(word: &str, style: &CaptionStyle) -> String {
        let mut text: String = word.trim().chars().filter(|c| !matches!(c, '{' | '}' | '\\')).collect();
        if text.chars().count() > 1 && text.ends_with([',', '.', ';', ':']) {
            text = text.trim_end_matches([',', '.', ';', ':']).to_string();
        }
        if style.uppercase {
            text.to_uppercase()
        } else {
            text
        }
    }

    fn hex_to_ass(hex: &str) -> String {
        let clean = hex.trim_start_matches('#');
        let byte = |i: usize| clean.get(i..i + 2).and_then(|s| u8::from_str_radix(s, 16).ok()).unwrap_or(0);
        format!("&H00{:02X}{:02X}{:02X}", byte(4), byte(2), byte(0))
    }

    /// ASS alpha tag value for an opacity in 0..1 (ASS alpha is inverted).
    fn alpha(opacity: f64) -> String {
        let o = opacity.clamp(0.0, 1.0);
        format!("&H{:02X}&", pyfmt::round((1.0 - o) * 255.0))
    }

    /// H:MM:SS.CC
    pub fn format_ass_time(ms: i64) -> String {
        let mut total_seconds = ms.div_euclid(1000);
        let mut centis = pyfmt::round(ms.rem_euclid(1000) as f64 / 10.0);
        if centis >= 100 {
            centis = 0;
            total_seconds += 1;
        }
        format!(
            "{}:{:02}:{:02}.{:02}",
            total_seconds.div_euclid(3600),
            total_seconds.div_euclid(60).rem_euclid(60),
            total_seconds.rem_euclid(60),
            centis
        )
    }

    pub fn parse_ass_time(value: &str) -> Option<i64> {
        let mut it = value.split(':');
        let (h, m, rest) = (it.next()?, it.next()?, it.next()?);
        let (s, c) = rest.split_once('.')?;
        Some(((h.trim().parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?) * 60 + s.parse::<i64>().ok()?) * 1000
            + c.parse::<i64>().ok()? * 10)
    }

    fn header(&self, width: i64, height: i64, video_region: Option<(i64, i64)>) -> String {
        let s = self.style;
        let alignment = alignment_code(&s.alignment, &s.position);
        let margin_v = match video_region {
            Some((y, h)) => (pyfmt::int((height - (y + h)) as f64 * 0.55)).max(30),
            None => match s.position.as_str() {
                "center" => 450,
                _ => 100,
            },
        };
        let default_style = format!(
            "Style: Default,{},{},{},{},{},{},{},{},0,0,100,100,{},0,1,{},0,{alignment},60,60,{margin_v},1",
            s.font_name,
            s.font_size,
            Self::hex_to_ass(&s.primary_color),
            Self::hex_to_ass(&s.highlight_color),
            Self::hex_to_ass(&s.outline_color),
            Self::hex_to_ass(&s.shadow_color),
            if s.bold { -1 } else { 0 },
            if s.italic { -1 } else { 0 },
            s.letter_spacing,
            s.outline_width,
        );
        format!(
            "[Script Info]\nTitle: Clipward Captions\nScriptType: v4.00+\nWrapStyle: 0\nScaledBorderAndShadow: yes\n\
             YCbCr Matrix: TV.709\nPlayResX: {width}\nPlayResY: {height}\n\n[V4+ Styles]\n\
             Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, \
             Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, \
             MarginR, MarginV, Encoding\n{default_style}\n\n"
        )
    }

    const EVENTS_HEADER: &'static str =
        "[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";

    /// Build the ASS document, or None when no segment overlaps the clip.
    #[allow(clippy::too_many_arguments)]
    pub fn build_ass(
        &self,
        segments: &[TranscriptSegment],
        clip_start_ms: i64,
        clip_end_ms: i64,
        width: i64,
        height: i64,
        video_region: Option<(i64, i64)>,
        anchors: Option<&[(i64, i64, i64)]>,
        mut placer: Option<Placer>,
    ) -> Option<String> {
        let relevant: Vec<&TranscriptSegment> = segments
            .iter()
            .filter(|s| s.end_time_ms > clip_start_ms && s.start_time_ms < clip_end_ms)
            .collect();
        if relevant.is_empty() {
            return None;
        }
        let has_word_timing = relevant.iter().any(|s| !s.words.is_empty());
        let header = self.header(width, height, video_region);
        let has_placer = placer.is_some();

        let mut place = |events: Vec<String>, words: &[String]| -> Vec<String> {
            let Some(placer) = placer.as_mut() else { return events };
            if events.is_empty() {
                return events;
            }
            let times: Vec<(i64, i64)> = events
                .iter()
                .map(|line| {
                    let fields: Vec<&str> = line["Dialogue: ".len()..].splitn(4, ',').collect();
                    (
                        Self::parse_ass_time(fields.get(1).copied().unwrap_or("")).unwrap_or(0),
                        Self::parse_ass_time(fields.get(2).copied().unwrap_or("")).unwrap_or(0),
                    )
                })
                .collect();
            let start = times.iter().map(|t| t.0).min().unwrap_or(0);
            let end = times.iter().map(|t| t.1).max().unwrap_or(0);
            let (w, h) = self.block_size(words, width);
            let (alignment, y) = placer(start, end, w, h);
            events.iter().map(|line| pin(line, alignment, width.div_euclid(2), y)).collect()
        };

        let events = if has_word_timing && self.style.word_by_word_highlight {
            self.word_by_word_events(&relevant, clip_start_ms, clip_end_ms, &mut place)
        } else {
            self.segment_events(&relevant, clip_start_ms, clip_end_ms, &mut place)
        };
        let mut content = header + Self::EVENTS_HEADER + &events.join("\n");
        if let Some(anchors) = anchors.filter(|a| !a.is_empty()) {
            if !has_placer {
                content = apply_anchors(&content, anchors, width);
            }
        }
        Some(content)
    }

    fn word_by_word_events(
        &self,
        segments: &[&TranscriptSegment],
        clip_start_ms: i64,
        clip_end_ms: i64,
        place: PlaceEvents,
    ) -> Vec<String> {
        let mut all_words: Vec<TranscriptWord> = Vec::new();
        for seg in segments {
            if seg.words.is_empty() {
                all_words.extend(split_segment_into_words(seg));
            } else {
                all_words.extend(seg.words.iter().cloned());
            }
        }
        let mut clip_words: Vec<TranscriptWord> = all_words
            .into_iter()
            .filter(|w| w.end_time_ms > clip_start_ms && w.start_time_ms < clip_end_ms)
            .collect();
        // In time order: a word the user retimed into another segment's stretch joins
        // that line instead of drawing a second one on top of it.
        clip_words.sort_by_key(|w| w.start_time_ms);
        let groups = group_words(&clip_words, self.style.max_words_per_line);
        let mut events = Vec::new();
        for (idx, group) in groups.iter().enumerate() {
            let next_start = groups.get(idx + 1).map(|g| g[0].start_time_ms);
            let group_events = self.word_group_events(group, clip_start_ms, next_start);
            let words: Vec<String> = group.iter().map(|w| w.word.clone()).collect();
            events.extend(place(group_events, &words));
        }
        events
    }

    fn segment_events(
        &self,
        segments: &[&TranscriptSegment],
        clip_start_ms: i64,
        clip_end_ms: i64,
        place: PlaceEvents,
    ) -> Vec<String> {
        let mut events = Vec::new();
        let mut segments = segments.to_vec();
        segments.sort_by_key(|s| s.start_time_ms);
        for (i, seg) in segments.iter().enumerate() {
            let start = (seg.start_time_ms - clip_start_ms).max(0);
            // A segment ends where the next begins, so two never show at once.
            let next = segments.get(i + 1).map_or(i64::MAX, |n| n.start_time_ms);
            let end = (clip_end_ms - clip_start_ms).min(seg.end_time_ms.min(next) - clip_start_ms);
            if end <= start {
                continue;
            }
            let words: Vec<String> = seg.text.split_whitespace().map(str::to_string).collect();
            let tokens = words
                .iter()
                .map(|w| Token {
                    text: Self::display_word(w, self.style),
                    state: State::Past,
                    emphasized: self.is_emphasis(w),
                    karaoke_cs: 0,
                })
                .collect::<Vec<_>>();
            let seg_events = self.layered_events(&tokens, start, end, false, 0);
            events.extend(place(seg_events, &words));
        }
        events
    }

    fn word_group_events(&self, words: &[TranscriptWord], clip_start_ms: i64, next_group_start_ms: Option<i64>) -> Vec<String> {
        if words.is_empty() {
            return Vec::new();
        }
        let style = self.style;
        let last_end = words[words.len() - 1].end_time_ms + MAX_LINGER_MS;
        let group_end = next_group_start_ms.map(|n| n.min(last_end)).unwrap_or(last_end);

        if style.karaoke_fill {
            let start = (words[0].start_time_ms - clip_start_ms).max(0);
            let end = (group_end - clip_start_ms).max(0);
            if end <= start {
                return Vec::new();
            }
            // Each sweep lasts until the next word starts, so gaps don't make it drift.
            let tokens: Vec<Token> = words
                .iter()
                .enumerate()
                .map(|(k, w)| {
                    let until = words.get(k + 1).map(|n| n.start_time_ms).unwrap_or(w.end_time_ms);
                    Token {
                        text: Self::display_word(&w.word, style),
                        state: State::Future,
                        emphasized: self.is_emphasis(&w.word),
                        karaoke_cs: 1.max(pyfmt::round((until - w.start_time_ms) as f64 / 10.0)),
                    }
                })
                .collect();
            return self.layered_events(&tokens, start, end, style.entrance_pop, 0);
        }

        let mut events: Vec<String> = Vec::new();
        for (i, current) in words.iter().enumerate() {
            let word_start = (current.start_time_ms - clip_start_ms).max(0);
            let word_end = match words.get(i + 1) {
                Some(next) => (next.start_time_ms - clip_start_ms).max(0),
                None => (group_end - clip_start_ms).max(0),
            };
            if word_end <= word_start {
                continue;
            }
            let tokens: Vec<Token> = words
                .iter()
                .enumerate()
                .map(|(j, w)| Token {
                    text: Self::display_word(&w.word, style),
                    state: if j == i {
                        State::Active
                    } else if j < i {
                        State::Past
                    } else {
                        State::Future
                    },
                    emphasized: self.is_emphasis(&w.word),
                    karaoke_cs: 0,
                })
                .collect();
            let entrance = style.entrance_pop && events.is_empty();
            events.extend(self.layered_events(&tokens, word_start, word_end, entrance, word_end - word_start));
        }
        events
    }

    /// One Dialogue line per enabled layer, all sharing the same words.
    fn layered_events(&self, tokens: &[Token], start_ms: i64, end_ms: i64, entrance: bool, active_ms: i64) -> Vec<String> {
        self.layered_lines(tokens, start_ms, end_ms, entrance, active_ms, self.style.max_words_per_line.max(1) as usize)
    }

    /// Free text in this preset's look (the editor's text layers): one line per `text`
    /// line, centred on (x, y) px at `font_size`, shown over [start_ms, end_ms).
    pub fn text_events(&self, text: &str, (start_ms, end_ms): (i64, i64), (x, y): (i64, i64), font_size: i64, animation: (&str, &str)) -> Vec<String> {
        let s = self.style;
        let tokens: Vec<Token> = text
            .lines()
            .map(|line| line.trim().chars().filter(|c| !matches!(c, '{' | '}' | '\\')).collect::<String>())
            .filter(|line| !line.is_empty())
            .map(|line| Token {
                text: if s.uppercase { line.to_uppercase() } else { line },
                state: State::Past,
                emphasized: false,
                karaoke_cs: 0,
            })
            .collect();
        if tokens.is_empty() || end_ms <= start_ms {
            return Vec::new();
        }
        let (anim_in, anim_out) = animation;
        let len = end_ms - start_ms;
        let (in_ms, out_ms) = (TEXT_IN_MS.min(len / 3), TEXT_OUT_MS.min(len / 3));
        // Slide in: rise into place from a line lower.
        let place = if anim_in == "slide" { format!("\\move({x},{},{x},{y},0,{in_ms})", y + font_size) } else { format!("\\pos({x},{y})") };
        let font = format!("\\an5{place}\\fn{}\\fs{font_size}\\fsp{}\\b{}\\i{}", s.font_name, s.letter_spacing, u8::from(s.bold), u8::from(s.italic));
        // Tags for one event [from, to): the entrance on the first, the exit on the last.
        let tags = |first: bool, last: bool, event_len: i64| {
            let fade_in = if !first { 0 } else {
                match anim_in {
                    "fade" | "slide" => in_ms,
                    "pop" => in_ms / 2,
                    _ => 0,
                }
            };
            let fade_out = if last && matches!(anim_out, "fade" | "pop") { out_ms } else { 0 };
            let mut t = String::new();
            if fade_in > 0 || fade_out > 0 {
                t += &format!("\\fad({fade_in},{fade_out})");
            }
            if first && anim_in == "pop" {
                t += &format!("\\fscx30\\fscy30\\t(0,{in_ms},\\fscx100\\fscy100)");
            }
            if last && anim_out == "pop" {
                t += &format!("\\t({},{event_len},\\fscx30\\fscy30)", event_len - out_ms);
            }
            t
        };
        let event = |tokens: &[Token], from: i64, to: i64, first: bool, last: bool| {
            let head = format!(",,{{{font}{}", tags(first, last, to - from));
            self.layered_lines(tokens, from, to, false, 0, 1).into_iter().map(move |line| line.replacen(",,{", &head, 1))
        };
        if anim_in != "typewriter" {
            return event(&tokens, start_ms, end_ms, true, true).collect();
        }
        // Typewriter: one event per word shown so far; the words still to come are there
        // but transparent, so the text never shifts as it appears.
        let words: usize = tokens.iter().map(|t| t.text.split_whitespace().count()).sum();
        let step = (TYPEWRITER_MS.min(len * 2 / 3) / words.max(1) as i64).max(1);
        let mut out = Vec::new();
        for shown in 1..=words {
            let (from, to) = (start_ms + (shown as i64 - 1) * step, if shown == words { end_ms } else { start_ms + shown as i64 * step });
            let mut seen = 0;
            let partial: Vec<Token> = tokens
                .iter()
                .map(|t| {
                    let mut text = String::new();
                    for (i, word) in t.text.split_whitespace().enumerate() {
                        if i > 0 {
                            text.push(' ');
                        }
                        if seen == shown {
                            text += r"{\alpha&HFF&}";
                        }
                        seen += 1;
                        text += word;
                    }
                    Token { text, ..t.clone() }
                })
                .collect();
            out.extend(event(&partial, from, to, shown == 1, shown == words));
        }
        out
    }

    /// An empty caption document (header and event list), for text layers on a clip without captions.
    pub fn empty_document(&self, width: i64, height: i64) -> String {
        self.header(width, height, None) + Self::EVENTS_HEADER
    }

    fn layered_lines(&self, tokens: &[Token], start_ms: i64, end_ms: i64, entrance: bool, active_ms: i64, per_line: usize) -> Vec<String> {
        let style = self.style;
        let mut layers: Vec<i64> = Vec::new();
        if style.line_box().is_some() {
            layers.push(LAYER_PLATE);
        }
        if style.shadow_opacity > 0.0 {
            layers.push(LAYER_SHADOW);
        }
        if style.glow().is_some() {
            layers.push(LAYER_GLOW);
        }
        if style.pill() {
            layers.push(LAYER_PILL);
        }
        layers.push(LAYER_FACE);

        let lead = if entrance { POP_TAGS } else { "" };
        let gap = if style.pill() { " \\h" } else { " " };
        let (start, end) = (Self::format_ass_time(start_ms), Self::format_ass_time(end_ms));
        layers
            .into_iter()
            .map(|layer| {
                let parts: Vec<String> = tokens
                    .iter()
                    .map(|t| {
                        let karaoke = if t.karaoke_cs != 0 { format!("\\kf{}", t.karaoke_cs) } else { String::new() };
                        let tags = match layer {
                            LAYER_PLATE => self.plate_tags(),
                            LAYER_SHADOW => self.shadow_tags(t),
                            LAYER_GLOW => self.glow_tags(t),
                            LAYER_PILL => self.pill_tags(t),
                            _ => self.face_tags(t, active_ms),
                        };
                        format!("{{{karaoke}{tags}}}{}", t.text)
                    })
                    .collect();
                let text = parts.chunks(per_line).map(|c| c.join(gap)).collect::<Vec<_>>().join("\\N");
                format!("Dialogue: {layer},{start},{end},Default,,0,0,0,,{{{lead}}}{text}")
            })
            .collect()
    }

    fn visibility(&self, t: &Token) -> f64 {
        let s = self.style;
        if t.state != State::Future || s.karaoke_fill {
            return 1.0;
        }
        match s.future_words.as_str() {
            "hide" => 0.0,
            "dim" => s.dim_opacity,
            _ => 1.0,
        }
    }

    fn face_tags(&self, t: &Token, active_ms: i64) -> String {
        let s = self.style;
        let emphasis = if t.emphasized { s.emphasis().map(Self::hex_to_ass) } else { None };
        let primary = Self::hex_to_ass(&s.primary_color);
        let highlight = Self::hex_to_ass(&s.highlight_color);
        let alpha = Self::alpha(self.visibility(t));
        let mut border = format!("\\bord{}\\3c{}", s.outline_width, Self::hex_to_ass(&s.outline_color));

        if s.karaoke_fill {
            let spoken = emphasis.unwrap_or(highlight);
            return format!("\\1c{spoken}\\2c{primary}\\1a&H00&\\2a{}\\3a&H00&{border}", Self::alpha(s.dim_opacity));
        }
        if t.state == State::Active {
            let color = emphasis.unwrap_or(highlight);
            if s.highlight_box().is_some() {
                border = "\\bord0".into();
            }
            if s.color_transition && active_ms >= 150 {
                return format!(
                    "\\alpha&H00&{border}\\1c{primary}\\t(0,{},\\1c{color})",
                    pyfmt::int(active_ms as f64 * 0.3)
                );
            }
            return format!("\\alpha&H00&{border}\\1c{color}");
        }
        format!("\\alpha{alpha}{border}\\1c{}", emphasis.unwrap_or(primary))
    }

    fn shadow_tags(&self, t: &Token) -> String {
        let s = self.style;
        let mut visibility = self.visibility(t);
        if s.highlight_box().is_some() && t.state == State::Active {
            visibility *= 0.5;
        }
        format!(
            "\\1a&HFF&\\3a&HFF&\\4a{}\\4c{}\\bord{}\\xshad0\\yshad{}\\blur{}",
            Self::alpha(s.shadow_opacity * visibility),
            Self::hex_to_ass(&s.shadow_color),
            s.outline_width + s.shadow_spread,
            s.shadow_offset,
            s.shadow_blur,
        )
    }

    fn glow_tags(&self, t: &Token) -> String {
        let s = self.style;
        let lit = t.state == State::Active || !s.glow_active_only || s.karaoke_fill;
        let opacity = if lit { s.glow_opacity * self.visibility(t) } else { 0.0 };
        format!(
            "\\1a&HFF&\\3a{}\\3c{}\\bord{}\\shad0\\blur{}",
            Self::alpha(opacity),
            Self::hex_to_ass(s.glow().unwrap_or("#000000")),
            s.glow_radius,
            s.glow_blur,
        )
    }

    fn pill_tags(&self, t: &Token) -> String {
        let s = self.style;
        if t.state != State::Active {
            return "\\alpha&HFF&\\bord0\\shad0".into();
        }
        let color = Self::hex_to_ass(s.highlight_box().unwrap_or("#000000"));
        let pad = s.highlight_box_padding;
        format!(
            "\\alpha&H00&\\1c{color}\\3c{color}\\shad0\\blur0.8\\bord{}\\t(0,80,\\bord{pad})",
            1.max(pad.div_euclid(2))
        )
    }

    fn plate_tags(&self) -> String {
        let s = self.style;
        let color = Self::hex_to_ass(s.line_box().unwrap_or("#000000"));
        format!(
            "\\1a&HFF&\\3a{}\\3c{color}\\bord{}\\shad0\\blur1",
            Self::alpha(s.line_box_opacity),
            s.line_box_padding
        )
    }

    /// Approximate drawn (width, height) of one caption group, in output px.
    pub fn block_size(&self, words: &[String], output_width: i64) -> (i64, i64) {
        let style = self.style;
        let wrap_w = (output_width - 2 * SIDE_MARGIN).max(1) as f64;
        let pill = style.pill();
        let gap = if pill { "  " } else { " " };
        let per_line = style.max_words_per_line.max(1) as usize;
        let mut shown: Vec<String> =
            words.iter().filter(|w| !w.trim().is_empty()).map(|w| Self::display_word(w, style)).collect();
        if shown.is_empty() {
            shown.push(String::new());
        }
        let texts: Vec<String> = shown.chunks(per_line).map(|c| c.join(gap)).collect();
        let size = style.font_size as f64;
        let spacing = style.letter_spacing as f64;

        let (widths, baseline, ink_top, ink_bottom): (Vec<f64>, f64, f64, f64) =
            match fonts::caption_font(self.fonts_dir, &style.font_name) {
                Some(m) => {
                    let scale = size / m.height;
                    let widths =
                        texts.iter().map(|t| m.get_length(t) * scale + spacing * t.chars().count() as f64).collect();
                    let (top, bottom) = m.ink_top_bottom(&texts.join(" "));
                    (widths, m.ascent * scale, top * scale, bottom * scale)
                }
                None => {
                    let widths =
                        texts.iter().map(|t| t.chars().count() as f64 * (size * FALLBACK_CHAR_EM + spacing)).collect();
                    (widths, size, -size, 0.0)
                }
            };
        let lines: f64 = widths.iter().map(|w| ((w / wrap_w).ceil()).max(1.0)).sum();

        let block_top = -lines * size / 2.0;
        let top = block_top + baseline + ink_top;
        let bottom = block_top + (lines - 1.0) * size + baseline + ink_bottom;
        let ow = style.outline_width as f64;
        let pad = [
            ow,
            if style.shadow_opacity > 0.0 { ow + (style.shadow_spread + style.shadow_blur) as f64 } else { 0.0 },
            if style.line_box().is_some() { (style.line_box_padding + 1) as f64 } else { 0.0 },
            if style.glow().is_some() { (style.glow_radius + style.glow_blur) as f64 } else { 0.0 },
            if pill { (style.highlight_box_padding + 1) as f64 } else { 0.0 },
        ]
        .into_iter()
        .fold(f64::MIN, f64::max);
        let shadow = if style.shadow_opacity > 0.0 { style.shadow_offset as f64 } else { 0.0 };
        let grow = if style.entrance_pop { 1.06 } else { 1.0 };
        let half = (pad - top).max(bottom + pad + shadow) * grow;
        let max_width = widths.iter().copied().fold(f64::MIN, f64::max);
        (pyfmt::round((max_width.min(wrap_w) + 2.0 * pad) * grow), pyfmt::round(2.0 * half))
    }
}

/// A Dialogue line with an \an/\pos override in front of its text.
pub fn pin(line: &str, alignment: i64, x: i64, y: i64) -> String {
    let prefix = "Dialogue: ";
    let body = line.strip_prefix(prefix).unwrap_or(line);
    let mut fields: Vec<String> = body.splitn(10, ',').map(str::to_string).collect();
    if fields.len() == 10 {
        fields[9] = format!("{{\\an{alignment}\\pos({x},{y})}}{}", fields[9]);
    }
    format!("{prefix}{}", fields.join(","))
}

/// Prefix each Dialogue with the \an/\pos of the layout active at its start.
pub fn apply_anchors(content: &str, anchors: &[(i64, i64, i64)], width: i64) -> String {
    let prefix = "Dialogue: ";
    content
        .split('\n')
        .map(|line| {
            let Some(body) = line.strip_prefix(prefix) else { return line.to_string() };
            let start = body
                .split(',')
                .nth(1)
                .and_then(CaptionGenerator::parse_ass_time)
                .unwrap_or(0);
            let (_, mut alignment, mut y) = anchors[anchors.len() - 1];
            if let Some(&(_, a, ay)) = anchors.iter().find(|(until, _, _)| start < *until) {
                alignment = a;
                y = ay;
            }
            pin(line, alignment, width.div_euclid(2), y)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn split_segment_into_words(seg: &TranscriptSegment) -> Vec<TranscriptWord> {
    let words: Vec<&str> = seg.text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let duration = (seg.end_time_ms - seg.start_time_ms) as f64;
    let per = duration / words.len() as f64;
    words
        .iter()
        .enumerate()
        .map(|(i, w)| TranscriptWord {
            word: w.to_string(),
            start_time_ms: pyfmt::int(seg.start_time_ms as f64 + i as f64 * per),
            end_time_ms: pyfmt::int(seg.start_time_ms as f64 + (i + 1) as f64 * per),
        })
        .collect()
}

/// Group words into display groups, breaking at natural pauses.
fn group_words(words: &[TranscriptWord], max_per_group: i64) -> Vec<Vec<TranscriptWord>> {
    let mut groups = Vec::new();
    let mut current: Vec<TranscriptWord> = Vec::new();
    for word in words {
        current.push(word.clone());
        let at_limit = current.len() as i64 >= max_per_group;
        let last = word.word.trim().chars().last();
        let at_sentence_end = last.map(|c| SENTENCE_END_CHARS.contains(&c)).unwrap_or(false);
        let at_comma = last == Some(',') && current.len() >= 2;
        if at_limit || at_sentence_end || at_comma {
            groups.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

/// Write ASS captions for a clip; returns the path, or None when there is no speech.
pub fn generate_captions(
    segments: &[TranscriptSegment],
    req: &CaptionRequest,
    output_path: &Path,
    placer: Option<Placer>,
) -> std::io::Result<Option<std::path::PathBuf>> {
    let generator = CaptionGenerator::new(req.style, req.emphasis_words, req.fonts_dir);
    let Some(content) = generator.build_ass(
        segments,
        req.clip_start_ms,
        req.clip_end_ms,
        req.output_width,
        req.output_height,
        req.video_region,
        req.anchors,
        placer,
    ) else {
        return Ok(None);
    };
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, content)?;
    Ok(Some(output_path.to_path_buf()))
}

// ------------------------------------------------------------------
// SRT sidecar
// ------------------------------------------------------------------

/// Split a cue into at most two balanced lines.
fn wrap_subtitle(text: &str) -> String {
    if text.chars().count() <= SRT_LINE_CHARS {
        return text.to_string();
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut best = text.to_string();
    let mut best_diff: Option<usize> = None;
    for i in 1..words.len() {
        let (top, bottom) = (words[..i].join(" "), words[i..].join(" "));
        let diff = top.chars().count().abs_diff(bottom.chars().count());
        if best_diff.map(|b| diff < b).unwrap_or(true) {
            best = format!("{top}\n{bottom}");
            best_diff = Some(diff);
        }
    }
    best
}

/// HH:MM:SS,mmm
fn format_srt_time(ms: i64) -> String {
    let ms = ms.max(0);
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// SRT text for the clip (broadcast conventions), or None when there is no speech.
pub fn build_srt(segments: &[TranscriptSegment], clip_start_ms: i64, clip_end_ms: i64) -> Option<String> {
    let mut words: Vec<TranscriptWord> = Vec::new();
    for seg in segments {
        if seg.words.is_empty() {
            words.extend(split_segment_into_words(seg));
        } else {
            words.extend(seg.words.iter().cloned());
        }
    }
    words.sort_by_key(|w| w.start_time_ms);
    let words: Vec<TranscriptWord> = words
        .into_iter()
        .filter(|w| w.end_time_ms > clip_start_ms && w.start_time_ms < clip_end_ms && !w.word.trim().is_empty())
        .collect();
    if words.is_empty() {
        return None;
    }

    let joined = |ws: &[TranscriptWord]| ws.iter().map(|w| w.word.trim()).collect::<Vec<_>>().join(" ");
    let mut cues: Vec<Vec<TranscriptWord>> = Vec::new();
    let mut current: Vec<TranscriptWord> = Vec::new();
    for word in words {
        if !current.is_empty() {
            let mut candidate = current.clone();
            candidate.push(word.clone());
            let too_long = joined(&candidate).chars().count() > 2 * SRT_LINE_CHARS;
            let too_slow = word.end_time_ms - current[0].start_time_ms > SRT_MAX_CUE_MS;
            let paused = word.start_time_ms - current[current.len() - 1].end_time_ms > SRT_PAUSE_BREAK_MS;
            if too_long || too_slow || paused {
                cues.push(std::mem::take(&mut current));
            }
        }
        let ends_sentence = word.word.trim().chars().last().map(|c| SENTENCE_END_CHARS.contains(&c)).unwrap_or(false);
        current.push(word);
        if ends_sentence && current.len() >= 3 {
            cues.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        cues.push(current);
    }

    let mut blocks: Vec<String> = Vec::new();
    for (index, cue) in cues.iter().enumerate() {
        let start = (cue[0].start_time_ms - clip_start_ms).max(0);
        let mut end = cue[cue.len() - 1].end_time_ms - clip_start_ms + SRT_LINGER_MS;
        if let Some(next) = cues.get(index + 1) {
            end = end.min(next[0].start_time_ms - clip_start_ms);
        }
        end = end.min(clip_end_ms - clip_start_ms);
        if end <= start {
            continue;
        }
        let text = cue.iter().map(|w| w.word.trim().replace(['{', '}'], "")).collect::<Vec<_>>().join(" ");
        blocks.push(format!(
            "{}\n{} --> {}\n{}\n",
            blocks.len() + 1,
            format_srt_time(start),
            format_srt_time(end),
            wrap_subtitle(&text)
        ));
    }
    if blocks.is_empty() {
        return None;
    }
    Some(blocks.join("\n"))
}

/// Write a standard SRT subtitle file for the clip; returns the path or None.
pub fn generate_srt(
    segments: &[TranscriptSegment],
    clip_start_ms: i64,
    clip_end_ms: i64,
    output_path: &Path,
) -> std::io::Result<Option<std::path::PathBuf>> {
    let Some(content) = build_srt(segments, clip_start_ms, clip_end_ms) else { return Ok(None) };
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, content)?;
    Ok(Some(output_path.to_path_buf()))
}
