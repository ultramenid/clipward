//! Main-track edits for the clip editor (ported from the React renderer's `timeline.ts`;
//! see docs/GPUI_PORT.md). The track is magnetic, like CapCut's: items play back to
//! back, so an item's place on the timeline is the sum of the ones before it (less any
//! cross-dissolve, where two pieces overlap). Every function returns a new list and
//! leaves its input alone.
//!
//! Where the TS returned the very same array to say "nothing changed" (`splitAt`,
//! `fitTransitions`, `fitKeyframes`, `moveItem`, ...), these return an equal copy:
//! compare with `==` (VideoItem is PartialEq) to tell an edit from a no-op.
//!
//! Rounding follows JavaScript's `Math.round` ([`js_round`]), not `f64::round`, so
//! every edit lands on the same millisecond as before.
use clip_engine::render::project::{
    AudioItem, Crop, CropKey, PipItem, TextLayer, VideoItem, MAX_CROP_ZOOM, MAX_TRANSITION_MS,
};
use clip_engine::{TranscriptSegment, TranscriptWord};
use std::collections::{HashMap, HashSet};

/// Shortest piece a trim or split may leave.
pub const MIN_ITEM_MS: i64 = 100;
/// Keyframes within this of the playhead are the one there (a frame at 30 fps).
pub const KEY_SNAP_MS: i64 = 34;
/// Shortest a caption word may be.
pub const MIN_WORD_MS: i64 = 50;
/// Shortest a text layer may be.
pub const MIN_TEXT_MS: i64 = 200;
/// Text layer animation lengths (private TEXT_IN_MS, TEXT_OUT_MS, TYPEWRITER_MS in the
/// engine's captions.rs).
const TEXT_IN_MS: i64 = 300;
const TEXT_OUT_MS: i64 = 250;
const TYPEWRITER_MS: i64 = 1_200;

pub use crate::format::js_round;

/// `js_round` as whole milliseconds.
fn round_ms(x: f64) -> i64 {
    js_round(x) as i64
}

/// JavaScript's `\s`: Unicode white space plus the BOM (not U+0085, which Rust counts).
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// `s.split(/\s+/)`: like JavaScript, white space at either end leaves an empty part there.
fn split_js_space(s: &str) -> Vec<&str> {
    let parts: Vec<&str> = s.split(js_space).collect();
    let last = parts.len() - 1;
    parts
        .into_iter()
        .enumerate()
        .filter(|&(i, p)| !p.is_empty() || i == 0 || i == last)
        .map(|(_, p)| p)
        .collect()
}

/// The words of `s` (`s.split(/\s+/).filter(Boolean)`).
fn js_words(s: &str) -> impl Iterator<Item = &str> {
    s.split(js_space).filter(|p| !p.is_empty())
}

/// Where a piece sits on the edited timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement<'a> {
    pub item: &'a VideoItem,
    pub index: usize,
    /// Edited time, ms.
    pub start: i64,
    pub end: i64,
    /// Where it is the piece on screen: dissolves hand over half way (Project::timeline).
    pub from: f64,
    pub to: f64,
}

/// Length on the edited timeline, after the piece's speed (VideoItem::out_ms).
pub fn item_length(item: &VideoItem) -> i64 {
    round_ms((item.src_out_ms - item.src_in_ms) as f64 / item.speed)
}

/// Edited time `t` into a piece, as source time.
fn src_at(item: &VideoItem, t: f64) -> i64 {
    (item.src_in_ms + round_ms(t * item.speed)).min(item.src_out_ms)
}

/// How long piece `index` overlaps the one before it (its cross-dissolve; Project::overlap).
pub fn overlap_of(video: &[VideoItem], index: usize) -> i64 {
    match video.get(index) {
        Some(item) if index > 0 && item.dissolve => item.transition_ms,
        _ => 0,
    }
}

/// Every piece's place on the edited timeline, in track order.
pub fn placements(video: &[VideoItem]) -> Vec<Placement<'_>> {
    let mut at = 0;
    video
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let start = at - overlap_of(video, index);
            at = start + item_length(item);
            Placement {
                item,
                index,
                start,
                end: at,
                from: start as f64 + overlap_of(video, index) as f64 / 2.0,
                to: at as f64 - overlap_of(video, index + 1) as f64 / 2.0,
            }
        })
        .collect()
}

/// Edited length of the whole track, ms.
pub fn total_length(video: &[VideoItem]) -> i64 {
    placements(video).last().map_or(0, |p| p.end)
}

/// Shortens transitions that no longer fit (after a trim, split, speed change or move):
/// each at most its piece and MAX_TRANSITION_MS, and dissolves in and out of a piece
/// together no longer than it. Equal to the input when nothing changes.
pub fn fit_transitions(video: &[VideoItem]) -> Vec<VideoItem> {
    let mut into = 0;
    video
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut max = item_length(item).min(MAX_TRANSITION_MS);
            // A dissolve takes its overlap from the previous piece too.
            if i > 0 && item.dissolve {
                max = max.min(item_length(&video[i - 1]) - into);
            }
            let transition_ms = item.transition_ms.min(max).max(0);
            into = if i > 0 && item.dissolve {
                transition_ms
            } else {
                0
            };
            VideoItem {
                transition_ms,
                ..item.clone()
            }
        })
        .collect()
}

/// How dark the instant preview is at `t` for dips to black (0 none, 1 black): half of a
/// dip fades the piece before out, half fades the next one in.
pub fn dip_at(video: &[VideoItem], t: f64) -> f64 {
    let mut dark: f64 = 0.0;
    for p in placements(video) {
        let dip_in = if p.item.dissolve && p.index > 0 {
            0.0
        } else {
            p.item.transition_ms as f64 / 2.0
        };
        let dip_out = match video.get(p.index + 1) {
            Some(next) if !next.dissolve => next.transition_ms as f64 / 2.0,
            _ => 0.0,
        };
        let (start, end) = (p.start as f64, p.end as f64);
        if dip_in > 0.0 && t >= start && t < start + dip_in {
            dark = dark.max(1.0 - (t - start) / dip_in);
        }
        if dip_out > 0.0 && t < end && t >= end - dip_out {
            dark = dark.max(1.0 - (end - t) / dip_out);
        }
    }
    dark
}

/// JavaScript's `Number(s)` for a decimal string: trimmed, "" is 0, junk is NaN (hex and
/// octal literals are not read; a frame rate never is one).
fn js_number(s: &str) -> f64 {
    let s = s.trim_matches(js_space);
    if s.is_empty() {
        0.0
    } else {
        s.parse().unwrap_or(f64::NAN)
    }
}

/// Nearest frame boundary, so every edit lands on a real frame. `fps` is "30" or
/// "30000/1001"; an unreadable rate only rounds to the millisecond.
pub fn snap_to_frame(ms: f64, fps: &str) -> i64 {
    let mut parts = fps.split('/');
    let num = parts.next().unwrap_or("");
    let den = parts.next().unwrap_or("1");
    let rate = js_number(num) / js_number(den);
    if !rate.is_finite() || rate <= 0.0 {
        return round_ms(ms);
    }
    let frame = 1000.0 / rate;
    round_ms(js_round(ms / frame) * frame)
}

/// The item on screen at edited time `t` (the last one at the very end).
pub fn placement_at(video: &[VideoItem], t: f64) -> Option<Placement<'_>> {
    let all = placements(video);
    all.iter()
        .find(|p| t >= p.from && t < p.to)
        .or(all.last())
        .copied()
}

/// Source time shown at edited time `t`.
pub fn source_time_at(video: &[VideoItem], t: f64) -> Option<i64> {
    let p = placement_at(video, t)?;
    Some(src_at(
        p.item,
        (t - p.start as f64)
            .max(0.0)
            .min(item_length(p.item) as f64),
    ))
}

/// `prefix` + the first number, from the list's length up, that no id uses.
fn next_id(prefix: &str, ids: &[&str]) -> String {
    let used: HashSet<&str> = ids.iter().copied().collect();
    let mut n = ids.len();
    while used.contains(format!("{prefix}{n}").as_str()) {
        n += 1;
    }
    format!("{prefix}{n}")
}

/// A fresh main-track item id ("v<n>").
pub fn next_item_id(video: &[VideoItem]) -> String {
    next_id(
        "v",
        &video.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
    )
}

/// Splits the item under edited time `t`; unchanged when a part would be too short.
pub fn split_at(video: &[VideoItem], t: i64) -> Vec<VideoItem> {
    let Some(p) = placements(video)
        .into_iter()
        .find(|q| t > q.start && t < q.end)
    else {
        return video.to_vec();
    };
    if t - p.start < MIN_ITEM_MS || p.end - t < MIN_ITEM_MS {
        return video.to_vec();
    }
    let at = t - p.start;
    let cut = src_at(p.item, at as f64);
    // The piece's own transition and fades stay at its ends; keyframes stay on their frames,
    // with the crop at the cut added to both sides so neither jumps.
    let c = crop_at(p.item, at as f64);
    let here = CropKey {
        t_ms: 0,
        x: c.x,
        y: c.y,
        zoom: c.zoom,
    };
    let keys = &p.item.keyframes;
    let left = VideoItem {
        src_out_ms: cut,
        fade_out_ms: 0,
        keyframes: if keys.is_empty() {
            Vec::new()
        } else {
            keys.iter()
                .filter(|k| k.t_ms < at)
                .copied()
                .chain([CropKey { t_ms: at, ..here }])
                .collect()
        },
        ..p.item.clone()
    };
    let right = VideoItem {
        id: next_item_id(video),
        src_in_ms: cut,
        fade_in_ms: 0,
        transition_ms: 0,
        dissolve: false,
        keyframes: if keys.is_empty() {
            Vec::new()
        } else {
            [here]
                .into_iter()
                .chain(keys.iter().filter(|k| k.t_ms > at).map(|k| CropKey {
                    t_ms: k.t_ms - at,
                    ..*k
                }))
                .collect()
        },
        ..p.item.clone()
    };
    let mut out = video[..p.index].to_vec();
    out.extend([left, right]);
    out.extend_from_slice(&video[p.index + 1..]);
    out
}

/// Inserts `item` (given a fresh id; its own is ignored) at edited time `t`: between
/// pieces, or splitting the piece there. Returns the new track and the item's index.
pub fn insert_at(video: &[VideoItem], t: i64, item: VideoItem) -> (Vec<VideoItem>, usize) {
    let mut split = split_at(video, t);
    let index = placements(&split)
        .iter()
        .position(|p| p.start >= t - 1)
        .unwrap_or(split.len());
    let id = next_item_id(&split);
    split.insert(index, VideoItem { id, ..item });
    (split, index)
}

/// Which end of an item a trim moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    In,
    Out,
}

/// Moves an item's in or out point to source time `src_ms`, within the source and the
/// minimum length.
pub fn trim_item(
    video: &[VideoItem],
    index: usize,
    edge: Edge,
    src_ms: f64,
    source_duration_ms: i64,
) -> Vec<VideoItem> {
    let Some(item) = video.get(index) else {
        return video.to_vec();
    };
    let next = match edge {
        Edge::In => {
            let src_in_ms = round_ms(src_ms.max(0.0).min((item.src_out_ms - MIN_ITEM_MS) as f64));
            // Keyframes stay on their frames: they move by the trim (fit_keyframes drops any cut off).
            let shift = round_ms((src_in_ms - item.src_in_ms) as f64 / item.speed);
            let keyframes = item
                .keyframes
                .iter()
                .map(|k| CropKey {
                    t_ms: k.t_ms - shift,
                    ..*k
                })
                .filter(|k| k.t_ms >= 0)
                .collect();
            VideoItem {
                src_in_ms,
                keyframes,
                ..item.clone()
            }
        }
        Edge::Out => VideoItem {
            src_out_ms: round_ms(
                src_ms
                    .min(source_duration_ms as f64)
                    .max((item.src_in_ms + MIN_ITEM_MS) as f64),
            ),
            ..item.clone()
        },
    };
    let mut out = video.to_vec();
    out[index] = next;
    out
}

/// Drops keyframes past the end of their piece (after a trim or speed-up). Equal to the
/// input when none are.
pub fn fit_keyframes(video: &[VideoItem]) -> Vec<VideoItem> {
    video
        .iter()
        .map(|item| {
            let len = item_length(item);
            VideoItem {
                keyframes: item
                    .keyframes
                    .iter()
                    .filter(|k| k.t_ms <= len)
                    .copied()
                    .collect(),
                ..item.clone()
            }
        })
        .collect()
}

/// The crop at edited ms `t` into a piece: its keyframes eased (Project::moving_crop), or
/// its fixed crop (the centre, unzoomed, without one).
pub fn crop_at(item: &VideoItem, t: f64) -> Crop {
    let keys = &item.keyframes;
    let crop = |k: &CropKey| Crop {
        x: k.x,
        y: k.y,
        zoom: k.zoom,
    };
    let Some(first) = keys.first() else {
        return item.crop.unwrap_or(Crop {
            x: 0.5,
            y: 0.5,
            zoom: 1.0,
        });
    };
    if t <= first.t_ms as f64 {
        return crop(first);
    }
    for pair in keys.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if t < b.t_ms as f64 {
            let p = ((t - a.t_ms as f64) / (b.t_ms - a.t_ms).max(1) as f64).clamp(0.0, 1.0);
            let e = p * p * (3.0 - 2.0 * p);
            return Crop {
                x: a.x + (b.x - a.x) * e,
                y: a.y + (b.y - a.y) * e,
                zoom: a.zoom + (b.zoom - a.zoom) * e,
            };
        }
    }
    crop(&keys[keys.len() - 1])
}

/// Sets the crop at `t` into a piece: the keyframe there changes, or a new one goes in.
pub fn set_crop_key(keys: &[CropKey], t: f64, crop: Crop) -> Vec<CropKey> {
    let at = round_ms(t);
    let key = |t_ms: i64| CropKey {
        t_ms,
        x: crop.x,
        y: crop.y,
        zoom: crop.zoom,
    };
    let mut out = keys.to_vec();
    if let Some(near) = keys.iter().position(|k| (k.t_ms - at).abs() < KEY_SNAP_MS) {
        out[near] = key(keys[near].t_ms);
        return out;
    }
    out.push(key(at));
    out.sort_by_key(|k| k.t_ms);
    out
}

/// Moves item `from` to index `to`; unchanged when either is off the track.
pub fn move_item(video: &[VideoItem], from: usize, to: usize) -> Vec<VideoItem> {
    let mut next = video.to_vec();
    if from == to || from >= video.len() || to >= video.len() {
        return next;
    }
    let item = next.remove(from);
    next.insert(to, item);
    next
}

/// Removes an item; the last one stays, since an edit needs video.
pub fn remove_item(video: &[VideoItem], index: usize) -> Vec<VideoItem> {
    if video.len() <= 1 {
        return video.to_vec();
    }
    video
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != index)
        .map(|(_, v)| v.clone())
        .collect()
}

/// The track with item `index` changed by `patch` (the TS took a partial item without
/// `id`, `src_in_ms` and `src_out_ms`; trims go through [`trim_item`]).
pub fn update_item(
    video: &[VideoItem],
    index: usize,
    patch: impl FnOnce(&mut VideoItem),
) -> Vec<VideoItem> {
    let mut out = video.to_vec();
    if let Some(item) = out.get_mut(index) {
        patch(item);
    }
    out
}

/// A caption word as it plays in the edit, pointing at its word in the transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct EditedWord {
    pub seg: usize,
    pub word: usize,
    pub text: String,
    /// Edited time. The end may fall on a half millisecond: a dissolve's handover.
    pub start: i64,
    pub end: f64,
    /// The piece it plays in.
    pub piece: usize,
}

/// JavaScript truthiness of a piece's `media`: an empty name reads the source, like none.
fn added_media(item: &VideoItem) -> Option<&String> {
    item.media.as_ref().filter(|m| !m.is_empty())
}

/// Caption words in edit order. A word starting inside a piece plays with it (as the
/// engine's captions do).
pub fn edited_words(video: &[VideoItem], transcript: &[TranscriptSegment]) -> Vec<EditedWord> {
    let mut out = Vec::new();
    for p in placements(video) {
        // Captions are the source's words; an added video has none.
        if added_media(p.item).is_some() {
            continue;
        }
        let (from, to) = (p.item.src_in_ms, p.item.src_out_ms);
        for (s, seg) in transcript.iter().enumerate() {
            if seg.end_time_ms <= from || seg.start_time_ms >= to {
                continue;
            }
            for (i, w) in seg.words.iter().enumerate() {
                if w.start_time_ms < from || w.start_time_ms >= to {
                    continue;
                }
                let at = |src: i64| p.start + round_ms((src - from) as f64 / p.item.speed);
                let start = at(w.start_time_ms);
                // Where a dissolve overlaps two pieces, a word plays with the one on screen.
                if (start as f64) < p.from || start as f64 >= p.to {
                    continue;
                }
                let end = p.to.min(at(w.end_time_ms) as f64);
                out.push(EditedWord {
                    seg: s,
                    word: i,
                    text: w.word.clone(),
                    start,
                    end,
                    piece: p.index,
                });
            }
        }
    }
    out.sort_by_key(|w| w.start);
    out
}

/// Rewrites one transcript word; empty text deletes it, and a segment left with no words.
pub fn set_word(
    transcript: &[TranscriptSegment],
    seg: usize,
    word: usize,
    text: &str,
) -> Vec<TranscriptSegment> {
    let next = js_words(text).collect::<Vec<_>>().join(" ");
    transcript
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            if i != seg {
                return Some(s.clone());
            }
            let words: Vec<TranscriptWord> = if next.is_empty() {
                s.words
                    .iter()
                    .enumerate()
                    .filter(|&(j, _)| j != word)
                    .map(|(_, w)| w.clone())
                    .collect()
            } else {
                s.words
                    .iter()
                    .enumerate()
                    .map(|(j, w)| {
                        if j == word {
                            TranscriptWord {
                                word: next.clone(),
                                ..w.clone()
                            }
                        } else {
                            w.clone()
                        }
                    })
                    .collect()
            };
            if words.is_empty() {
                return None;
            }
            let text = words
                .iter()
                .map(|w| w.word.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            Some(TranscriptSegment {
                words,
                text,
                ..s.clone()
            })
        })
        .collect()
}

/// Retimes a caption word to edited [start, end) in its piece: stays inside the piece
/// and between its neighbours in the segment, which grows to hold it.
pub fn set_word_time(
    video: &[VideoItem],
    transcript: &[TranscriptSegment],
    w: &EditedWord,
    start: f64,
    end: f64,
) -> Vec<TranscriptSegment> {
    let all = placements(video);
    let (Some(p), Some(seg)) = (all.get(w.piece), transcript.get(w.seg)) else {
        return transcript.to_vec();
    };
    if w.word >= seg.words.len() {
        return transcript.to_vec();
    }
    let src = |t: f64| p.item.src_in_ms + round_ms((t - p.start as f64) * p.item.speed);
    // The word stays between its neighbours, in other segments too: two captions must
    // never show at once.
    let before = w
        .word
        .checked_sub(1)
        .and_then(|i| seg.words.get(i))
        .or_else(|| {
            transcript[..w.seg]
                .iter()
                .rev()
                .find(|x| !x.words.is_empty())
                .and_then(|x| x.words.last())
        });
    let after = seg.words.get(w.word + 1).or_else(|| {
        transcript[w.seg + 1..]
            .iter()
            .find(|x| !x.words.is_empty())
            .map(|x| &x.words[0])
    });
    let lo = before.map_or(p.item.src_in_ms, |b| p.item.src_in_ms.max(b.end_time_ms));
    let hi = after.map_or(p.item.src_out_ms, |a| {
        p.item.src_out_ms.min(a.start_time_ms)
    });
    if hi - lo < MIN_WORD_MS {
        return transcript.to_vec();
    }
    let s = src(start).max(lo).min(hi - MIN_WORD_MS);
    let e = src(end).min(hi).max(s + MIN_WORD_MS);
    let words: Vec<TranscriptWord> = seg
        .words
        .iter()
        .enumerate()
        .map(|(i, x)| {
            if i == w.word {
                TranscriptWord {
                    start_time_ms: s,
                    end_time_ms: e,
                    ..x.clone()
                }
            } else {
                x.clone()
            }
        })
        .collect();
    // The segment spans its words: an end word moved in pulls the edge in with it.
    let first = w.word == 0;
    let last = w.word == seg.words.len() - 1;
    let mut out = transcript.to_vec();
    out[w.seg] = TranscriptSegment {
        start_time_ms: if first { s } else { seg.start_time_ms.min(s) },
        end_time_ms: if last { e } else { seg.end_time_ms.max(e) },
        words,
        ..seg.clone()
    };
    out
}

/// The engine's emphasis key for a word (norm() in captions.rs): lowercase ASCII letters
/// and digits only.
pub fn emphasis_key(word: &str) -> String {
    word.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

/// Whether `word` is one of the key words (each entry may hold several, space separated).
pub fn is_emphasized(emphasis: &[String], word: &str) -> bool {
    let key = emphasis_key(word);
    key.len() >= 2
        && emphasis.iter().any(|e| {
            split_js_space(e)
                .into_iter()
                .any(|part| emphasis_key(part) == key)
        })
}

/// Adds or removes a word from the key words the caption preset colours.
pub fn toggle_emphasis(emphasis: &[String], word: &str) -> Vec<String> {
    let key = emphasis_key(word);
    if key.len() < 2 {
        return emphasis.to_vec();
    }
    if !is_emphasized(emphasis, word) {
        return emphasis.iter().cloned().chain([key]).collect();
    }
    emphasis
        .iter()
        .filter_map(|e| {
            let kept: Vec<&str> = split_js_space(e)
                .into_iter()
                .filter(|part| emphasis_key(part) != key)
                .collect();
            (!kept.is_empty()).then(|| kept.join(" "))
        })
        .collect()
}

/// How a text layer looks at one moment (see [`text_motion`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMotion {
    pub opacity: f64,
    pub scale: f64,
    /// How far below its place it is, in font sizes.
    pub drop: f64,
    /// How many words are typed so far.
    pub words: usize,
}

/// How a text layer looks at edited time `t` in the instant preview, following its
/// animation as the engine draws it (captions.rs text_events): opacity, scale, how far
/// below its place it is (in font sizes), and how many words are typed so far.
pub fn text_motion(layer: &TextLayer, t: f64) -> TextMotion {
    let len = layer.end_ms - layer.start_ms;
    // div_euclid by a positive number is Math.floor's division, negative lengths too.
    let in_ms = TEXT_IN_MS.min(len.div_euclid(3));
    let out_ms = TEXT_OUT_MS.min(len.div_euclid(3));
    let since = t - layer.start_ms as f64;
    let left = layer.end_ms as f64 - t;
    let rise = if in_ms > 0 {
        (since / in_ms as f64).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let fall = if out_ms > 0 {
        (left / out_ms as f64).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let (a, b) = (layer.animation_in.as_str(), layer.animation_out.as_str());
    let mut opacity = 1.0;
    let mut scale = 1.0;
    if a == "fade" || a == "slide" {
        opacity *= rise;
    }
    if a == "pop" {
        opacity *= (since / (in_ms as f64 / 2.0).max(1.0)).min(1.0);
        scale = 0.3 + 0.7 * rise;
    }
    if b == "fade" || b == "pop" {
        opacity *= fall;
    }
    if b == "pop" {
        scale *= 0.3 + 0.7 * fall;
    }
    let total = js_words(&layer.text).count();
    let step = TYPEWRITER_MS
        .min((len * 2).div_euclid(3))
        .div_euclid(total.max(1) as i64)
        .max(1);
    let words = if a == "typewriter" {
        total.min((since.max(0.0) / step as f64).floor() as usize + 1)
    } else {
        total
    };
    TextMotion {
        opacity: opacity.clamp(0.0, 1.0),
        scale,
        drop: if a == "slide" { 1.0 - rise } else { 0.0 },
        words,
    }
}

/// Timeline rows for (id, start, end) items: each goes in the first row where it
/// overlaps nothing, in start order. Row by id.
pub fn lanes_of<'a>(
    items: impl IntoIterator<Item = (&'a str, i64, i64)>,
) -> HashMap<String, usize> {
    let mut sorted: Vec<_> = items.into_iter().collect();
    sorted.sort_by_key(|&(_, start, _)| start);
    let mut ends: Vec<i64> = Vec::new();
    let mut lanes = HashMap::new();
    for (id, start, end) in sorted {
        let lane = ends.iter().position(|&e| e <= start).unwrap_or(ends.len());
        if lane == ends.len() {
            ends.push(end);
        } else {
            ends[lane] = end;
        }
        lanes.insert(id.to_string(), lane);
    }
    lanes
}

/// Rows for the text layers.
pub fn text_lanes(texts: &[TextLayer]) -> HashMap<String, usize> {
    lanes_of(texts.iter().map(|t| (t.id.as_str(), t.start_ms, t.end_ms)))
}

/// Anything on the timeline that plays a stretch of a file: a sound or a
/// picture-in-picture video (which has no speed or fades).
pub trait FileClip: Clone {
    fn id(&self) -> &str;
    fn start_ms(&self) -> i64;
    fn src_in_ms(&self) -> i64;
    fn src_out_ms(&self) -> i64;
    /// Playback speed; 1 for a clip without one.
    fn speed(&self) -> f64 {
        1.0
    }
    /// Sound fades; None for a clip without them.
    fn fade_in_ms(&self) -> Option<i64> {
        None
    }
    fn fade_out_ms(&self) -> Option<i64> {
        None
    }
    fn set_start_ms(&mut self, ms: i64);
    fn set_src_in_ms(&mut self, ms: i64);
    fn set_src_out_ms(&mut self, ms: i64);
    /// Sets a fade; does nothing on a clip without fades.
    fn set_fade_in_ms(&mut self, _ms: i64) {}
    fn set_fade_out_ms(&mut self, _ms: i64) {}
}

impl FileClip for AudioItem {
    fn id(&self) -> &str {
        &self.id
    }
    fn start_ms(&self) -> i64 {
        self.start_ms
    }
    fn src_in_ms(&self) -> i64 {
        self.src_in_ms
    }
    fn src_out_ms(&self) -> i64 {
        self.src_out_ms
    }
    fn speed(&self) -> f64 {
        self.speed
    }
    fn fade_in_ms(&self) -> Option<i64> {
        Some(self.fade_in_ms)
    }
    fn fade_out_ms(&self) -> Option<i64> {
        Some(self.fade_out_ms)
    }
    fn set_start_ms(&mut self, ms: i64) {
        self.start_ms = ms;
    }
    fn set_src_in_ms(&mut self, ms: i64) {
        self.src_in_ms = ms;
    }
    fn set_src_out_ms(&mut self, ms: i64) {
        self.src_out_ms = ms;
    }
    fn set_fade_in_ms(&mut self, ms: i64) {
        self.fade_in_ms = ms;
    }
    fn set_fade_out_ms(&mut self, ms: i64) {
        self.fade_out_ms = ms;
    }
}

impl FileClip for PipItem {
    fn id(&self) -> &str {
        &self.id
    }
    fn start_ms(&self) -> i64 {
        self.start_ms
    }
    fn src_in_ms(&self) -> i64 {
        self.src_in_ms
    }
    fn src_out_ms(&self) -> i64 {
        self.src_out_ms
    }
    fn set_start_ms(&mut self, ms: i64) {
        self.start_ms = ms;
    }
    fn set_src_in_ms(&mut self, ms: i64) {
        self.src_in_ms = ms;
    }
    fn set_src_out_ms(&mut self, ms: i64) {
        self.src_out_ms = ms;
    }
}

/// A sound's (or picture-in-picture's) length on the timeline, after its speed
/// (AudioItem::out_ms).
pub fn sound_length(a: &impl FileClip) -> i64 {
    round_ms((a.src_out_ms() - a.src_in_ms()) as f64 / a.speed())
}

/// Rows for the picture-in-picture videos.
pub fn pip_lanes(pip: &[PipItem]) -> HashMap<String, usize> {
    lanes_of(
        pip.iter()
            .map(|p| (p.id.as_str(), p.start_ms, p.start_ms + sound_length(p))),
    )
}

/// A fresh picture-in-picture id ("p<n>").
pub fn next_pip_id(pip: &[PipItem]) -> String {
    next_id("p", &pip.iter().map(|p| p.id.as_str()).collect::<Vec<_>>())
}

/// Rows for the sounds.
pub fn sound_lanes(audio: &[AudioItem]) -> HashMap<String, usize> {
    lanes_of(
        audio
            .iter()
            .map(|a| (a.id.as_str(), a.start_ms, a.start_ms + sound_length(a))),
    )
}

/// A fresh text layer id ("t<n>").
pub fn next_text_id(texts: &[TextLayer]) -> String {
    next_id(
        "t",
        &texts.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
    )
}

/// Moves a layer to start at `start`, keeping its length and staying inside [0, total].
pub fn move_text(texts: &[TextLayer], id: &str, start: f64, total: i64) -> Vec<TextLayer> {
    texts
        .iter()
        .map(|t| {
            if t.id != id {
                return t.clone();
            }
            let len = t.end_ms - t.start_ms;
            let s = round_ms(start.max(0.0).min(0.max(total - len) as f64));
            TextLayer {
                start_ms: s,
                end_ms: s + len,
                ..t.clone()
            }
        })
        .collect()
}

/// Moves a layer's start or end, within [0, total] and the minimum length.
pub fn trim_text(texts: &[TextLayer], id: &str, edge: Edge, ms: f64, total: i64) -> Vec<TextLayer> {
    texts
        .iter()
        .map(|t| {
            if t.id != id {
                return t.clone();
            }
            match edge {
                Edge::In => TextLayer {
                    start_ms: round_ms(ms.max(0.0).min((t.end_ms - MIN_TEXT_MS) as f64)),
                    ..t.clone()
                },
                Edge::Out => TextLayer {
                    end_ms: round_ms(ms.min(total as f64).max((t.start_ms + MIN_TEXT_MS) as f64)),
                    ..t.clone()
                },
            }
        })
        .collect()
}

/// A fresh sound id ("a<n>").
pub fn next_sound_id(audio: &[AudioItem]) -> String {
    next_id(
        "a",
        &audio.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
    )
}

/// Detaches a piece's sound: the piece goes silent and the same sound, with its volume
/// and fades, becomes an item on the audio track that can move and trim on its own.
/// Both unchanged when the piece is missing or already silent.
pub fn detach_audio(
    video: &[VideoItem],
    audio: &[AudioItem],
    index: usize,
) -> (Vec<VideoItem>, Vec<AudioItem>) {
    let all = placements(video);
    let Some(p) = all.get(index).filter(|p| p.item.volume != 0.0) else {
        return (video.to_vec(), audio.to_vec());
    };
    let item = p.item;
    let media = added_media(item);
    let sound = AudioItem {
        id: next_sound_id(audio),
        // An added video's sound is its own file.
        name: if media.is_some() {
            "Clip sound"
        } else {
            "Original sound"
        }
        .into(),
        file: item.media.clone(),
        src_in_ms: item.src_in_ms,
        src_out_ms: item.src_out_ms,
        start_ms: p.start,
        volume: item.volume,
        fade_in_ms: item.fade_in_ms,
        fade_out_ms: item.fade_out_ms,
        duck: false,
        voice: false,
        speed: item.speed,
    };
    let video = update_item(video, index, |v| {
        v.volume = 0.0;
        v.fade_in_ms = 0;
        v.fade_out_ms = 0;
    });
    (video, audio.iter().cloned().chain([sound]).collect())
}

/// Adds a voiceover take, cutting the part of earlier takes it records over (punch-in):
/// a take it covers goes, one it covers partly is trimmed or split around it.
pub fn punch_in(audio: &[AudioItem], take: AudioItem) -> Vec<AudioItem> {
    let from = take.start_ms;
    let to = take.start_ms + sound_length(&take);
    let mut out: Vec<AudioItem> = Vec::new();
    for a in audio {
        let end = a.start_ms + sound_length(a);
        if !a.voice || end <= from || a.start_ms >= to {
            out.push(a.clone());
            continue;
        }
        // Timeline time into the take, as time in its file.
        let file = |t: i64| a.src_in_ms + round_ms((t - a.start_ms) as f64 * a.speed);
        if from - a.start_ms >= MIN_ITEM_MS {
            let len = from - a.start_ms;
            out.push(AudioItem {
                src_out_ms: file(from),
                fade_out_ms: a.fade_out_ms.min(len),
                ..a.clone()
            });
        }
        if end - to >= MIN_ITEM_MS {
            let id = if from - a.start_ms >= MIN_ITEM_MS {
                let ids: Vec<&str> = audio
                    .iter()
                    .chain(&out)
                    .chain([&take])
                    .map(|x| x.id.as_str())
                    .collect();
                next_id("a", &ids)
            } else {
                a.id.clone()
            };
            out.push(AudioItem {
                id,
                start_ms: to,
                src_in_ms: file(to),
                fade_in_ms: a.fade_in_ms.min(end - to),
                ..a.clone()
            });
        }
    }
    out.push(take);
    out
}

/// Moves a sound to start at `start` (edited time, not before 0). A sound may run past
/// the end of the video; the export stops there.
pub fn move_sound<T: FileClip>(audio: &[T], id: &str, start: f64) -> Vec<T> {
    audio
        .iter()
        .map(|a| {
            let mut a = a.clone();
            if a.id() == id {
                a.set_start_ms(round_ms(start.max(0.0)));
            }
            a
        })
        .collect()
}

/// Trims a sound's start or end to edited time `ms`: the start edge moves the timeline
/// start and the file's in point together. Stays within the file (`file_ms`) and
/// MIN_ITEM_MS.
pub fn trim_sound<T: FileClip>(audio: &[T], id: &str, edge: Edge, ms: f64, file_ms: i64) -> Vec<T> {
    audio
        .iter()
        .map(|a| {
            let mut next = a.clone();
            if a.id() != id {
                return next;
            }
            let speed = a.speed();
            let (start, src_in) = (a.start_ms() as f64, a.src_in_ms() as f64);
            let end = a.start_ms() + sound_length(a);
            if edge == Edge::Out {
                // Timeline length, up to the end of the file.
                let len = (ms - start)
                    .max(MIN_ITEM_MS as f64)
                    .min((file_ms as f64 - src_in) / speed);
                if let Some(fade) = a.fade_out_ms() {
                    next.set_fade_out_ms(fade.min(round_ms(len)));
                }
                next.set_src_out_ms(round_ms(src_in + len * speed));
                return next;
            }
            // Earliest: the file's start (and time 0); latest: MIN_ITEM_MS before the end.
            let shift = (ms - start)
                .max(-src_in / speed)
                .max(-start)
                .min((end - MIN_ITEM_MS) as f64 - start);
            let src_in_ms = round_ms(src_in + shift * speed);
            let len = round_ms((a.src_out_ms() - src_in_ms) as f64 / speed);
            if let Some(fade) = a.fade_in_ms() {
                next.set_fade_in_ms(fade.min(len));
            }
            next.set_start_ms(round_ms(start + shift));
            next.set_src_in_ms(src_in_ms);
            next
        })
        .collect()
}

/// A rectangle as fractions of a frame: top-left corner and size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A crop's window as fractions of the source (Project::crop_rect, without the pixel
/// rounding): the canvas's shape, the largest that fits divided by zoom, kept inside.
/// `source` and `canvas` are (width, height) px.
pub fn crop_window(source: (f64, f64), canvas: (f64, f64), crop: Crop) -> Rect {
    let (sw, sh) = source;
    let aspect = canvas.0 / canvas.1;
    let fit_w = if sw / sh > aspect { sh * aspect } else { sw };
    let fit_h = fit_w / aspect;
    let zoom = crop.zoom.clamp(1.0, MAX_CROP_ZOOM);
    let w = fit_w / zoom / sw;
    let h = fit_h / zoom / sh;
    Rect {
        x: (crop.x - w / 2.0).max(0.0).min(1.0 - w),
        y: (crop.y - h / 2.0).max(0.0).min(1.0 - h),
        w,
        h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, a: i64, b: i64) -> VideoItem {
        VideoItem {
            id: id.into(),
            src_in_ms: a,
            src_out_ms: b,
            ..Default::default()
        }
    }

    fn track() -> Vec<VideoItem> {
        vec![item("v0", 1000, 3000), item("v1", 5000, 6000)]
    }

    fn spans(video: &[VideoItem]) -> Vec<(i64, i64)> {
        placements(video).iter().map(|p| (p.start, p.end)).collect()
    }

    fn word(w: &str, a: i64, b: i64) -> TranscriptWord {
        TranscriptWord {
            word: w.into(),
            start_time_ms: a,
            end_time_ms: b,
        }
    }

    fn segment(a: i64, b: i64, text: &str, words: Vec<TranscriptWord>) -> TranscriptSegment {
        TranscriptSegment {
            start_time_ms: a,
            end_time_ms: b,
            text: text.into(),
            speaker_label: None,
            words,
            audio_events: Vec::new(),
        }
    }

    fn transcript() -> Vec<TranscriptSegment> {
        vec![
            segment(
                1000,
                3000,
                "hello big world",
                vec![
                    word("hello", 1100, 1400),
                    word("big", 1500, 1800),
                    word("world", 2500, 2900),
                ],
            ),
            segment(5000, 6000, "bye", vec![word("bye", 5200, 5600)]),
        ]
    }

    fn find<'a>(words: &'a [EditedWord], text: &str) -> &'a EditedWord {
        words.iter().find(|w| w.text == text).unwrap()
    }

    #[test]
    fn js_round_rounds_halves_up() {
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.6), -3.0);
        assert_eq!(js_round(-0.5), 0.0);
        assert_eq!(js_round(0.49999999999999994), 0.0);
        assert_eq!(js_round(1033.3333), 1033.0);
    }

    #[test]
    fn items_play_back_to_back_and_edited_time_maps_to_source_time() {
        let track = track();
        assert_eq!(spans(&track), [(0, 2000), (2000, 3000)]);
        assert_eq!(source_time_at(&track, 500.0), Some(1500));
        assert_eq!(source_time_at(&track, 2500.0), Some(5500));
        assert_eq!(source_time_at(&track, 3000.0), Some(6000));
    }

    #[test]
    fn split_cuts_the_item_under_the_playhead_never_into_slivers() {
        let track = track();
        let split = split_at(&track, 800);
        let ids: Vec<_> = split
            .iter()
            .map(|i| (i.id.as_str(), i.src_in_ms, i.src_out_ms))
            .collect();
        assert_eq!(
            ids,
            [("v0", 1000, 1800), ("v2", 1800, 3000), ("v1", 5000, 6000)]
        );
        assert_eq!(total_length(&split), total_length(&track));
        assert_eq!(split_at(&track, 50), track);
        assert_eq!(split_at(&track, 2000), track);
        assert_eq!(track[0], item("v0", 1000, 3000));
    }

    #[test]
    fn trims_stay_inside_the_source_and_keep_a_minimum_length() {
        let track = track();
        assert_eq!(
            trim_item(&track, 0, Edge::In, -500.0, 10000)[0].src_in_ms,
            0
        );
        assert_eq!(
            trim_item(&track, 0, Edge::In, 2990.0, 10000)[0].src_in_ms,
            2900
        );
        assert_eq!(
            trim_item(&track, 1, Edge::Out, 12000.0, 10000)[1].src_out_ms,
            10000
        );
        assert_eq!(
            trim_item(&track, 1, Edge::Out, 4000.0, 10000)[1].src_out_ms,
            5100
        );
    }

    #[test]
    fn move_remove_and_frame_snapping() {
        let track = track();
        assert_eq!(
            move_item(&track, 0, 1)
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            ["v1", "v0"]
        );
        assert_eq!(remove_item(&track, 0).len(), 1);
        assert_eq!(remove_item(&track[..1], 0).len(), 1);
        assert_eq!(snap_to_frame(1010.0, "30"), 1000);
        assert_eq!(snap_to_frame(1020.0, "30"), 1033);
        assert_eq!(snap_to_frame(1000.0, "30000/1001"), 1001);
    }

    #[test]
    fn caption_words_follow_the_edit_and_edits_land_on_the_transcript() {
        let (track, transcript) = (track(), transcript());
        // Second piece first: "bye" plays at 0, then source 1-3 s.
        let words = edited_words(&[track[1].clone(), track[0].clone()], &transcript);
        let got: Vec<_> = words.iter().map(|w| (w.text.as_str(), w.start)).collect();
        assert_eq!(
            got,
            [
                ("bye", 200),
                ("hello", 1100),
                ("big", 1500),
                ("world", 2500)
            ]
        );
        let fixed = set_word(&transcript, 0, 1, "  huge ");
        assert_eq!(fixed[0].words[1].word, "huge");
        assert_eq!(fixed[0].text, "hello huge world");
        assert_eq!(set_word(&transcript, 0, 1, "")[0].text, "hello world");
        assert_eq!(
            set_word(&transcript, 1, 0, "").len(),
            1,
            "a segment with no words left is dropped"
        );
        let on = toggle_emphasis(&["free money".to_string()], "World!");
        assert_eq!(on, ["free money", "world"]);
        assert!(is_emphasized(&on, "money,"));
        assert_eq!(toggle_emphasis(&on, "MONEY"), ["free", "world"]);
    }

    #[test]
    fn emphasis_splits_on_white_space_like_javascript() {
        // `split(/\s+/)` keeps an empty part for leading space, which survives a removal.
        assert_eq!(
            toggle_emphasis(&[" free\u{a0}money".to_string()], "free"),
            [" money"]
        );
        assert_eq!(split_js_space("a \t b"), ["a", "b"]);
        assert_eq!(split_js_space(" "), ["", ""]);
        assert_eq!(
            set_word(&transcript(), 0, 0, "\u{feff} hi\u{3000}there "),
            set_word(&transcript(), 0, 0, "hi there")
        );
    }

    fn text(id: &str, a: i64, b: i64) -> TextLayer {
        TextLayer {
            id: id.into(),
            text: id.into(),
            start_ms: a,
            end_ms: b,
            x: 0.5,
            y: 0.5,
            size: 84,
            preset: "pop".into(),
            animation_in: "none".into(),
            animation_out: "none".into(),
        }
    }

    #[test]
    fn text_layers_stack_in_rows_and_stay_on_the_timeline() {
        let texts = vec![
            text("t0", 0, 2000),
            text("t1", 1000, 3000),
            text("t2", 2000, 2500),
        ];
        let lanes = text_lanes(&texts);
        assert_eq!(
            (lanes.len(), lanes["t0"], lanes["t1"], lanes["t2"]),
            (3, 0, 1, 0)
        );
        assert_eq!(next_text_id(&texts), "t3");
        assert_eq!(
            move_text(&texts, "t1", 9000.0, 4000)[1],
            TextLayer {
                start_ms: 2000,
                end_ms: 4000,
                ..texts[1].clone()
            }
        );
        assert_eq!(move_text(&texts, "t1", -50.0, 4000)[1].start_ms, 0);
        assert_eq!(
            trim_text(&texts, "t0", Edge::Out, 50.0, 4000)[0].end_ms,
            MIN_TEXT_MS
        );
        assert_eq!(
            trim_text(&texts, "t0", Edge::Out, 9000.0, 4000)[0].end_ms,
            4000
        );
    }

    #[test]
    fn detached_sound_moves_and_trims_on_its_own() {
        let video = vec![
            item("v0", 1000, 3000),
            VideoItem {
                volume: 0.5,
                fade_in_ms: 200,
                ..item("v1", 5000, 6000)
            },
        ];
        let (v, audio) = detach_audio(&video, &[], 1);
        assert_eq!(v[1].volume, 0.0);
        let expected = AudioItem {
            id: "a0".into(),
            name: "Original sound".into(),
            file: None,
            src_in_ms: 5000,
            src_out_ms: 6000,
            start_ms: 2000,
            volume: 0.5,
            fade_in_ms: 200,
            fade_out_ms: 0,
            duck: false,
            voice: false,
            speed: 1.0,
        };
        assert_eq!(audio, [expected]);
        assert_eq!(
            detach_audio(&v, &audio, 1).1.len(),
            1,
            "a silent piece has nothing to detach"
        );
        assert_eq!(move_sound(&audio, "a0", -300.0)[0].start_ms, 0);
        // Start edge 300 ms later: timeline start and in point move together.
        let later = &trim_sound(&audio, "a0", Edge::In, 2300.0, 9000)[0];
        assert_eq!(
            (later.start_ms, later.src_in_ms, later.src_out_ms),
            (2300, 5300, 6000)
        );
        // Not before the file's start or time 0, and not past the end.
        assert_eq!(trim_sound(&audio, "a0", Edge::In, 0.0, 9000)[0].start_ms, 0);
        assert_eq!(
            trim_sound(&audio, "a0", Edge::In, 5000.0, 9000)[0].src_in_ms,
            6000 - MIN_ITEM_MS
        );
        assert_eq!(
            trim_sound(&audio, "a0", Edge::Out, 9000.0, 6500)[0].src_out_ms,
            6500,
            "stops at the end of the file"
        );
        let lanes = sound_lanes(&[
            audio[0].clone(),
            AudioItem {
                id: "a1".into(),
                start_ms: 2500,
                ..audio[0].clone()
            },
        ]);
        assert_eq!((lanes.len(), lanes["a0"], lanes["a1"]), (2, 0, 1));
    }

    #[test]
    fn a_sped_up_piece_is_shorter_on_the_timeline_and_maps_back_to_its_source() {
        let fast = vec![
            VideoItem {
                speed: 2.0,
                ..item("v0", 1000, 3000)
            },
            item("v1", 5000, 6000),
        ];
        assert_eq!(spans(&fast), [(0, 1000), (1000, 2000)]);
        assert_eq!(source_time_at(&fast, 500.0), Some(2000));
        let split: Vec<_> = split_at(&fast, 250)[..2]
            .iter()
            .map(|i| (i.src_in_ms, i.src_out_ms, i.speed))
            .collect();
        assert_eq!(split, [(1000, 1500, 2.0), (1500, 3000, 2.0)]);
        let words = edited_words(&fast, &transcript());
        assert_eq!(
            find(&words, "big"),
            &EditedWord {
                seg: 0,
                word: 1,
                text: "big".into(),
                start: 250,
                end: 400.0,
                piece: 0
            }
        );
        // Its detached sound keeps the speed: 1 s on the timeline, trimmed in timeline time.
        let sound = detach_audio(&fast, &[], 0).1.remove(0);
        assert_eq!(sound.speed, 2.0);
        assert_eq!(sound_length(&sound), 1000);
        let cut = &trim_sound(std::slice::from_ref(&sound), "a0", Edge::Out, 500.0, 9000)[0];
        assert_eq!(
            (cut.src_in_ms, cut.src_out_ms, sound_length(cut)),
            (1000, 2000, 500)
        );
        let late = &trim_sound(&[sound], "a0", Edge::In, 250.0, 9000)[0];
        assert_eq!(
            (late.start_ms, late.src_in_ms, sound_length(late)),
            (250, 1500, 750)
        );
    }

    #[test]
    fn a_cross_dissolve_overlaps_two_pieces_and_hands_over_half_way() {
        let video = vec![
            item("v0", 1000, 3000),
            VideoItem {
                dissolve: true,
                transition_ms: 400,
                ..item("v1", 5000, 6000)
            },
        ];
        let got: Vec<_> = placements(&video)
            .iter()
            .map(|p| (p.start, p.end, p.from, p.to))
            .collect();
        assert_eq!(got, [(0, 2000, 0.0, 1800.0), (1600, 2600, 1800.0, 2600.0)]);
        assert_eq!(total_length(&video), 2600);
        assert_eq!(placement_at(&video, 1700.0).unwrap().index, 0);
        assert_eq!(placement_at(&video, 1900.0).unwrap().index, 1);
        // "world" (source 2.5 s, edited 1.5 s) plays in the first piece until the handover.
        let words = edited_words(&video, &transcript());
        let w = find(&words, "world");
        assert_eq!((w.start, w.end, w.piece), (1500, 1800.0, 0));
        // Too long for the pieces: shortened; nothing to fit: the same track.
        let long = vec![
            item("v0", 0, 1000),
            VideoItem {
                dissolve: true,
                transition_ms: 700,
                ..item("v1", 1000, 2000)
            },
            VideoItem {
                dissolve: true,
                transition_ms: 700,
                ..item("v2", 2000, 3000)
            },
        ];
        assert_eq!(
            fit_transitions(&long)
                .iter()
                .map(|i| i.transition_ms)
                .collect::<Vec<_>>(),
            [0, 700, 300]
        );
        assert_eq!(fit_transitions(&video), video);
        // A split keeps the transition before the piece, not inside it.
        let split: Vec<_> = split_at(&video, 2200)
            .iter()
            .map(|i| (i.transition_ms, i.dissolve))
            .collect();
        assert_eq!(split, [(0, false), (400, true), (0, false)]);
    }

    #[test]
    fn dips_to_black_darken_the_instant_preview() {
        let video = vec![
            VideoItem {
                transition_ms: 400,
                ..item("v0", 0, 1000)
            },
            VideoItem {
                transition_ms: 800,
                ..item("v1", 1000, 2000)
            },
        ];
        assert_eq!(dip_at(&video, 0.0), 1.0);
        assert_eq!(dip_at(&video, 100.0), 0.5);
        assert_eq!(dip_at(&video, 500.0), 0.0);
        assert_eq!(dip_at(&video, 800.0), 0.5);
        assert_eq!(dip_at(&video, 1000.0), 1.0);
        let dissolved = [
            video[0].clone(),
            VideoItem {
                dissolve: true,
                ..video[1].clone()
            },
        ];
        assert_eq!(dip_at(&dissolved, 900.0), 0.0, "a dissolve is no dip");
    }

    fn vo(id: &str, start: i64, len: i64) -> AudioItem {
        AudioItem {
            id: id.into(),
            name: id.into(),
            file: Some(format!("{id}.m4a")),
            src_in_ms: 0,
            src_out_ms: len,
            start_ms: start,
            volume: 1.0,
            fade_in_ms: 0,
            fade_out_ms: 0,
            duck: false,
            voice: true,
            speed: 1.0,
        }
    }

    #[test]
    fn a_new_voiceover_take_replaces_what_it_records_over() {
        let music = AudioItem {
            voice: false,
            ..vo("m", 0, 9000)
        };
        let out = punch_in(&[music, vo("a0", 0, 5000)], vo("a2", 1000, 2000));
        // The music stays; the old take is split around the new one.
        let got: Vec<_> = out
            .iter()
            .map(|a| (a.start_ms, a.src_in_ms, a.src_out_ms))
            .collect();
        assert_eq!(
            got,
            [
                (0, 0, 9000),
                (0, 0, 1000),
                (3000, 3000, 5000),
                (1000, 0, 2000)
            ]
        );
        assert_eq!(
            out.iter()
                .map(|a| a.id.as_str())
                .collect::<HashSet<_>>()
                .len(),
            4,
            "the split-off part gets its own id"
        );
        let ids: Vec<_> = punch_in(&[vo("a0", 1000, 1000)], vo("a1", 0, 5000))
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids, ["a1"], "a covered take goes");
    }

    #[test]
    fn caption_words_retime_inside_their_piece_and_between_neighbours() {
        let (video, transcript) = (vec![item("v0", 1000, 3000)], transcript());
        let words = edited_words(&video, &transcript);
        // Edited 400..900 is source 1400..1900: clamped after "hello" (ends 1400).
        let moved =
            &set_word_time(&video, &transcript, find(&words, "big"), 350.0, 900.0)[0].words[1];
        assert_eq!((moved.start_time_ms, moved.end_time_ms), (1400, 1900));
        let late = &set_word_time(&video, &transcript, find(&words, "world"), 1600.0, 5000.0)[0];
        assert_eq!(
            (late.words[2].end_time_ms, late.end_time_ms),
            (3000, 3000),
            "not past the piece"
        );
    }

    #[test]
    fn a_caption_word_cannot_be_dragged_into_the_next_segment() {
        // A one-word segment, then a sentence starting at 2000 (the case that drew two lines at once).
        let segs = vec![
            segment(
                1000,
                2000,
                "Alhamdulillah.",
                vec![word("Alhamdulillah.", 1000, 2000)],
            ),
            segment(
                2000,
                4000,
                "Kalo di radio",
                vec![
                    word("Kalo", 2000, 2300),
                    word("di", 2300, 2500),
                    word("radio", 2500, 4000),
                ],
            ),
        ];
        let video = vec![item("v0", 0, 5000)];
        let words = edited_words(&video, &segs);
        let out = &set_word_time(
            &video,
            &segs,
            find(&words, "Alhamdulillah."),
            2400.0,
            3400.0,
        )[0];
        assert!(out.words[0].end_time_ms <= 2000, "stops at \"Kalo\"");
        assert_eq!(
            out.end_time_ms, out.words[0].end_time_ms,
            "the segment ends with its word"
        );
    }

    #[test]
    fn added_media_goes_in_at_the_playhead_and_has_no_captions() {
        let track = track();
        let clip = VideoItem {
            media: Some("media_1.mp4".into()),
            framing: "crop".into(),
            crop: Some(Crop {
                x: 0.5,
                y: 0.5,
                zoom: 1.0,
            }),
            ..item("v9", 0, 2000)
        };
        // Inside the first piece: it splits there.
        let (video, index) = insert_at(&track, 500, clip.clone());
        assert_eq!(index, 1);
        let got: Vec<_> = video
            .iter()
            .map(|v| (v.src_in_ms, v.src_out_ms, v.media.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                (1000, 1500, None),
                (0, 2000, Some("media_1.mp4")),
                (1500, 3000, None),
                (5000, 6000, None)
            ]
        );
        assert_eq!(insert_at(&track, 3000, clip).1, 2, "at the end");
        assert!(
            edited_words(&video, &transcript())
                .iter()
                .all(|w| w.piece != 1),
            "no words from the added video"
        );
        assert_eq!(
            video
                .iter()
                .map(|v| v.id.as_str())
                .collect::<HashSet<_>>()
                .len(),
            video.len(),
            "ids stay unique after the split"
        );
        let sound = detach_audio(&video, &[], 1).1.remove(0);
        assert_eq!(sound.file.as_deref(), Some("media_1.mp4"));
    }

    #[test]
    fn picture_in_picture_moves_and_trims_like_a_sound() {
        let pip = vec![PipItem {
            id: "p0".into(),
            media: "m.mp4".into(),
            src_in_ms: 0,
            src_out_ms: 2000,
            start_ms: 1000,
            x: 0.5,
            y: 0.5,
            width: 0.5,
            volume: 1.0,
        }];
        assert_eq!(move_sound(&pip, "p0", 2500.0)[0].start_ms, 2500);
        assert_eq!(
            trim_sound(&pip, "p0", Edge::Out, 5000.0, 2500)[0],
            PipItem {
                src_out_ms: 2500,
                ..pip[0].clone()
            }
        );
        assert_eq!(next_pip_id(&pip), "p1");
        let lanes = pip_lanes(&[
            pip[0].clone(),
            PipItem {
                id: "p1".into(),
                start_ms: 2000,
                ..pip[0].clone()
            },
        ]);
        assert_eq!((lanes.len(), lanes["p0"], lanes["p1"]), (2, 0, 1));
    }

    #[test]
    fn crop_keyframes_ease_follow_trims_and_splits_and_snap_to_the_playhead() {
        let key = |t_ms, x, zoom| CropKey {
            t_ms,
            x,
            y: 0.5,
            zoom,
        };
        let keyed = VideoItem {
            framing: "crop".into(),
            keyframes: vec![key(0, 0.2, 1.0), key(2000, 0.8, 3.0)],
            ..item("v0", 1000, 3000)
        };
        assert_eq!(
            crop_at(&keyed, -50.0),
            Crop {
                x: 0.2,
                y: 0.5,
                zoom: 1.0
            }
        );
        assert_eq!(
            crop_at(&keyed, 1000.0),
            Crop {
                x: 0.5,
                y: 0.5,
                zoom: 2.0
            },
            "half way (smoothstep is symmetric)"
        );
        assert!(
            crop_at(&keyed, 500.0).zoom < 1.5,
            "eased: slow at the start"
        );
        assert_eq!(
            crop_at(&keyed, 9000.0),
            Crop {
                x: 0.8,
                y: 0.5,
                zoom: 3.0
            }
        );
        // Trimming 500 ms off the start moves the keys with their frames; the first falls off.
        let trimmed = &trim_item(std::slice::from_ref(&keyed), 0, Edge::In, 1500.0, 10000)[0];
        assert_eq!(
            trimmed.keyframes.iter().map(|k| k.t_ms).collect::<Vec<_>>(),
            [1500]
        );
        let fitted = &fit_keyframes(&[VideoItem {
            src_out_ms: 2000,
            ..keyed.clone()
        }])[0];
        assert_eq!(
            fitted.keyframes.iter().map(|k| k.t_ms).collect::<Vec<_>>(),
            [0]
        );
        // A split keeps the crop continuous: both sides get the crop at the cut.
        let split = split_at(std::slice::from_ref(&keyed), 1000);
        assert_eq!(split[0].keyframes.last(), Some(&key(1000, 0.5, 2.0)));
        assert_eq!(
            split[1]
                .keyframes
                .iter()
                .map(|k| k.t_ms)
                .collect::<Vec<_>>(),
            [0, 1000]
        );
        // Near an existing key: it changes; elsewhere: a new one.
        let crop = Crop {
            x: 0.3,
            y: 0.5,
            zoom: 1.0,
        };
        assert_eq!(set_crop_key(&keyed.keyframes, 20.0, crop).len(), 2);
        assert_eq!(
            set_crop_key(&keyed.keyframes, 700.0, crop)
                .iter()
                .map(|k| k.t_ms)
                .collect::<Vec<_>>(),
            [0, 700, 2000]
        );
        // Without keyframes: the fixed crop, or the centre.
        assert_eq!(
            crop_at(&item("v0", 0, 1000), 0.0),
            Crop {
                x: 0.5,
                y: 0.5,
                zoom: 1.0
            }
        );
    }

    #[test]
    fn text_animations_in_the_instant_preview_follow_the_export() {
        let layer = TextLayer {
            text: "one two\nthree".into(),
            start_ms: 1000,
            end_ms: 4000,
            size: 60,
            ..text("t0", 0, 0)
        };
        let with = |a: &str, b: &str| TextLayer {
            animation_in: a.into(),
            animation_out: b.into(),
            ..layer.clone()
        };
        assert_eq!(
            text_motion(&layer, 1000.0),
            TextMotion {
                opacity: 1.0,
                scale: 1.0,
                drop: 0.0,
                words: 3
            }
        );
        assert_eq!(text_motion(&with("fade", "fade"), 1150.0).opacity, 0.5);
        assert_eq!(
            text_motion(&with("none", "fade"), 4000.0 - 125.0).opacity,
            0.5
        );
        assert_eq!(text_motion(&with("slide", "none"), 1150.0).drop, 0.5);
        assert!(text_motion(&with("pop", "none"), 1100.0).scale < 0.6);
        // Typewriter: 3 words over 1.2 s, one every 400 ms (as the engine's events).
        let typed = with("typewriter", "none");
        let words: Vec<_> = [1000.0, 1399.0, 1400.0, 1800.0, 3000.0]
            .iter()
            .map(|&ms| text_motion(&typed, ms).words)
            .collect();
        assert_eq!(words, [1, 1, 2, 3, 3]);
    }

    #[test]
    fn crop_window_is_the_canvas_shape_kept_inside_the_source() {
        let (landscape, portrait) = ((1920.0, 1080.0), (1080.0, 1920.0));
        let at = |x, y, zoom| Crop { x, y, zoom };
        // 9:16 out of 16:9: full height, 607.5 of 1920 px wide.
        assert_eq!(
            crop_window(landscape, portrait, at(0.5, 0.5, 1.0)),
            Rect {
                x: 0.341796875,
                y: 0.0,
                w: 0.31640625,
                h: 1.0
            }
        );
        // Pushed against the edges.
        assert_eq!(crop_window(landscape, portrait, at(0.0, 0.5, 1.0)).x, 0.0);
        assert_eq!(
            crop_window(landscape, portrait, at(1.0, 0.5, 1.0)).x,
            0.68359375
        );
        // Zoom clamps to 1..MAX_CROP_ZOOM.
        assert_eq!(
            crop_window(landscape, portrait, at(0.5, 0.5, 0.5)),
            crop_window(landscape, portrait, at(0.5, 0.5, 1.0))
        );
        let max = crop_window(landscape, portrait, at(0.5, 0.5, 10.0));
        assert_eq!((max.w, max.h), (607.5 / 5.0 / 1920.0, 0.2));
        assert_eq!((max.x, max.y), (0.5 - max.w / 2.0, 0.5 - max.h / 2.0));
        // 16:9 out of 9:16: full width, 607.5 of 1920 px tall.
        assert_eq!(
            crop_window(portrait, landscape, at(0.5, 0.5, 1.0)),
            Rect {
                x: 0.0,
                y: 0.341796875,
                w: 1.0,
                h: 0.31640625
            }
        );
        // Same shape: the whole frame at zoom 1, a centred half at zoom 2.
        assert_eq!(
            crop_window(landscape, landscape, at(0.9, 0.1, 1.0)),
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0
            }
        );
        assert_eq!(
            crop_window(landscape, landscape, at(0.5, 0.5, 2.0)),
            Rect {
                x: 0.25,
                y: 0.25,
                w: 0.5,
                h: 0.5
            }
        );
    }
}
