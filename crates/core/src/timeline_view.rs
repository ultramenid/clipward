//! The editor timeline's geometry (the React renderer's `Timeline.tsx`, `Filmstrip.tsx`
//! and `Waveform.tsx`; see docs/GPUI_PORT.md): pixels and time, zooming about the
//! pointer, where each track sits, what a drag does to an item, and which thumbnails
//! and loudness bars a box shows. Sizes are CSS pixels; the view scales them.
use crate::timeline::{self, sound_length, snap_to_frame, Edge, EditedWord};
use clip_engine::render::project::{OverlayBox, Project, VideoItem};
use clip_engine::render::project::{FILMSTRIP_BLOCK_S, WAVEFORM_RATE};
use std::collections::HashMap;
use std::ops::Range;

/// Space before time 0 and after the end.
pub const GUTTER: f64 = 16.0;
/// Pointer travel before a press on an item becomes a drag.
pub const DRAG_SLOP: f64 = 4.0;
/// Shortest an overlay may be shown.
pub const MIN_OVERLAY_MS: i64 = 200;

/// Track heights: the ruler, one overlay row (and its box), a PiP or text lane (and
/// its box), the caption words, a sound lane (and its box), the gap between tracks.
pub const RULER_H: f64 = 24.0;
pub const OVERLAY_ROW: f64 = 22.0;
pub const OVERLAY_H: f64 = 18.0;
pub const LANE_ROW: f64 = 26.0;
pub const LANE_H: f64 = 22.0;
pub const WORDS_H: f64 = 20.0;
pub const SOUND_ROW: f64 = 34.0;
pub const SOUND_H: f64 = 30.0;
pub const GAP: f64 = 8.0;

/// Where time `ms` sits across the content, px.
pub fn x_at(ms: f64, zoom: f64) -> f64 {
    GUTTER + ms * zoom / 1000.0
}

/// The frame-snapped time at content x `x`, within [0, total].
pub fn time_at(x: f64, zoom: f64, fps: &str, total: i64) -> f64 {
    let ms = snap_to_frame((x - GUTTER) / (zoom / 1000.0), fps);
    ms.max(0).min(total) as f64
}

/// The zoom that shows the whole edit across `view_width` (never below `min`).
pub fn fit_zoom(view_width: f64, total_ms: i64, min: f64) -> f64 {
    min.max((view_width - GUTTER * 2.0) / (total_ms as f64 / 1000.0))
}

/// Zoom by `factor` about the pointer at `pointer_x` across the view, keeping the time
/// under it in place: the new zoom and scroll, or None when the zoom is at its bound.
pub fn zoom_about(current: f64, factor: f64, min: f64, max: f64, pointer_x: f64, scroll_x: f64) -> Option<(f64, f64)> {
    let next = (current * factor).clamp(min, max);
    if next == current || !next.is_finite() {
        return None;
    }
    let time = (pointer_x + scroll_x - GUTTER) / current;
    Some((next, time * next + GUTTER - pointer_x))
}

/// ⌘/Ctrl + wheel: a wheel step (browser `deltaY`, px; positive scrolls down) as a zoom factor.
pub fn wheel_zoom_factor(delta_y: f64) -> f64 {
    (-delta_y.clamp(-50.0, 50.0) * 0.01).exp()
}

/// The edit and anything after it: sounds and PiP may run past the end of the video.
pub fn extent(project: &Project) -> i64 {
    let total = timeline::total_length(&project.video);
    let sounds = project.audio.iter().map(|a| a.start_ms + sound_length(a));
    let pip = project.pip.iter().map(|p| p.start_ms + sound_length(p));
    sounds.chain(pip).fold(total, i64::max)
}

/// Content width: the extent plus gutters, at least the view (so the ruler keeps counting).
pub fn content_width(extent_ms: i64, zoom: f64, view_width: f64) -> f64 {
    (extent_ms as f64 * zoom / 1000.0 + GUTTER * 2.0).max(view_width)
}

/// Ruler labels whose span reaches [from_x, to_x): (x, seconds). Labels sit at least
/// ~60 px apart (`tick_step`) over the whole content width.
pub fn ruler_ticks(zoom: f64, width: f64, from_x: f64, to_x: f64) -> Vec<(f64, f64)> {
    let step = crate::format::tick_step(zoom);
    let ruler_ms = (width - GUTTER) / (zoom / 1000.0);
    let count = (ruler_ms / 1000.0 / step).floor() as i64 + 1;
    let gap = step * zoom;
    // A label reaches right of its line, so the one before the view still shows.
    let first = (((from_x - GUTTER) / gap).floor() as i64 - 1).max(0);
    let last = (((to_x - GUTTER) / gap).ceil() as i64 + 1).min(count);
    (first..last).map(|i| (GUTTER + i as f64 * gap, i as f64 * step)).collect()
}

/// Rows a set of lanes needs.
pub fn lane_rows(lanes: &HashMap<String, usize>) -> usize {
    lanes.values().max().map_or(0, |m| m + 1)
}

/// The top of each track, top to bottom, and the whole height. A track that isn't
/// shown has no top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackLayout {
    pub overlays: Option<f64>,
    pub pip: Option<f64>,
    pub texts: Option<f64>,
    pub words: Option<f64>,
    pub main: f64,
    pub sounds: Option<f64>,
    pub height: f64,
}

pub fn track_layout(
    overlay_rows: usize,
    pip_rows: usize,
    text_rows: usize,
    words: bool,
    track_height: f64,
    sound_rows: usize,
) -> TrackLayout {
    let mut y = RULER_H;
    let mut track = |shown: bool, height: f64| {
        shown.then(|| {
            y += GAP;
            let top = y;
            y += height;
            top
        })
    };
    let overlays = track(overlay_rows > 0, overlay_rows as f64 * OVERLAY_ROW);
    let pip = track(pip_rows > 0, pip_rows as f64 * LANE_ROW);
    let texts = track(text_rows > 0, text_rows as f64 * LANE_ROW);
    let words = track(words, WORDS_H);
    let main = y + GAP;
    y = main + track_height + GAP;
    let sounds = (sound_rows > 0).then(|| {
        let top = y;
        y += sound_rows as f64 * SOUND_ROW + GAP;
        top
    });
    TrackLayout { overlays, pip, texts, words, main, sounds, height: y }
}

/// An overlay's box after dragging it (edge None) or an edge by `shift_ms` (frame
/// snapped), within [0, total] and at least MIN_OVERLAY_MS long; and the time to show.
pub fn drag_overlay(b: &OverlayBox, edge: Option<Edge>, shift_ms: i64, total: i64) -> (OverlayBox, i64) {
    let len = b.end_ms - b.start_ms;
    let (mut start, mut end) = (b.start_ms, b.end_ms);
    match edge {
        Some(Edge::In) => start = (b.start_ms + shift_ms).max(0).min(b.end_ms - MIN_OVERLAY_MS),
        Some(Edge::Out) => end = (b.end_ms + shift_ms).min(total).max(b.start_ms + MIN_OVERLAY_MS),
        None => {
            start = (b.start_ms + shift_ms).max(0).min(0.max(total - len));
            end = start + len;
        }
    }
    let seek = if edge == Some(Edge::Out) { end } else { start }.min(total);
    (OverlayBox { start_ms: start, end_ms: end, ..*b }, seek)
}

/// Where a piece dragged `dx_ms` along the track lands: its middle passes the middles
/// of the pieces before it.
pub fn drop_index(video: &[VideoItem], index: usize, dx_ms: f64) -> usize {
    let Some(p) = timeline::placements(video).get(index).copied() else { return index };
    let middle = (p.start + p.end) as f64 / 2.0 + dx_ms;
    let others: Vec<VideoItem> = video.iter().enumerate().filter(|&(i, _)| i != index).map(|(_, v)| v.clone()).collect();
    timeline::placements(&others).iter().filter(|o| ((o.start + o.end) as f64 / 2.0) < middle).count()
}

/// How far a piece's out point may go: an added video's own length, else the source's.
pub fn trim_bound(project: &Project, item: &VideoItem, source_duration_ms: i64) -> i64 {
    match &item.media {
        Some(media) => project.media.iter().find(|m| &m.file == media).map_or(item.src_out_ms, |m| m.duration_ms),
        None => source_duration_ms,
    }
}

/// A caption word dragged (edge None) or an edge dragged by `shift_ms`: its new edited
/// [start, end) and the time to show.
pub fn drag_word(w: &EditedWord, edge: Option<Edge>, shift_ms: f64, total: i64) -> (f64, f64, f64) {
    let start = if edge == Some(Edge::Out) { w.start as f64 } else { w.start as f64 + shift_ms };
    let end = if edge == Some(Edge::In) { w.end } else { w.end + shift_ms };
    let seek = if edge == Some(Edge::Out) { end } else { start };
    (start, end, seek.clamp(0.0, total as f64))
}

/// The word to type over after Enter or Tab (`tab`) on word `typing` of `count`: Tab goes
/// to the next, Shift+Tab to the previous. A word typed away (`kept` false) shifts the
/// rest down one, so the next is then at the same place. None closes the box.
pub fn next_word(typing: usize, shift: bool, kept: bool, tab: bool, count: usize) -> Option<usize> {
    let next = typing as i64 + if shift { -1 } else if kept { 1 } else { 0 };
    let limit = count as i64 - if kept { 0 } else { 1 };
    (tab && next >= 0 && next < limit).then_some(next as usize)
}

/// A word's text as saved: runs of white space become one space, none at the ends.
pub fn clean_word(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Words (sorted by start) that may show in edited [from_ms, to_ms).
pub fn visible_words(words: &[EditedWord], from_ms: f64, to_ms: f64) -> Range<usize> {
    // Words don't overlap much: one starting this long before the view has ended.
    const LONGEST_MS: f64 = 30_000.0;
    let lo = words.partition_point(|w| (w.start as f64) < from_ms - LONGEST_MS);
    let hi = words.partition_point(|w| (w.start as f64) < to_ms);
    lo..hi.max(lo)
}

/// The tiles of a filmstrip box: each `tile` px wide showing second `seconds[i]` of the
/// file, cut from a frame `frame_width` px wide at the box's height.
#[derive(Debug, Clone, PartialEq)]
pub struct Tiles {
    pub tile: f64,
    pub frame_width: f64,
    pub seconds: Vec<i64>,
}

/// Frames of [from, to) ms of a file across a `width` × `height` box, CapCut style: each
/// tile shows as much of a frame as the export's shape (`aspect`), so a vertical export
/// gets narrow tiles. `frame_aspect` is the file's picture, width / height.
pub fn filmstrip_tiles(from: i64, to: i64, width: f64, height: f64, frame_aspect: f64, aspect: f64) -> Tiles {
    let frame_width = height * frame_aspect;
    let tile = (height * aspect.min(frame_aspect)).max(8.0);
    let count = ((width / tile).ceil() as usize).max(1);
    let span = (to - from) as f64;
    // The second each tile shows: the file time under its middle.
    let seconds = (0..count)
        .map(|i| {
            let at = from as f64 + ((i as f64 + 0.5) * tile / width.max(1.0)) * span;
            (at.min((to - 1) as f64) / 1000.0).floor() as i64
        })
        .collect();
    Tiles { tile, frame_width, seconds }
}

/// The filmstrip block (an image of FILMSTRIP_BLOCK_S frames) and the frame in it for
/// second `s`.
pub fn block_of(s: i64) -> (u32, usize) {
    let s = s.max(0);
    ((s / FILMSTRIP_BLOCK_S as i64) as u32, (s % FILMSTRIP_BLOCK_S as i64) as usize)
}

/// The blocks tiles `range` need, first seen first.
pub fn blocks_for(seconds: &[i64], range: Range<usize>) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for &s in &seconds[range.start.min(seconds.len())..range.end.min(seconds.len())] {
        let block = block_of(s).0;
        if out.last() != Some(&block) && !out.contains(&block) {
            out.push(block);
        }
    }
    out
}

/// Tiles of `tile` px (from content x 0) that reach into [from_x, to_x).
pub fn visible_tiles(tile: f64, count: usize, from_x: f64, to_x: f64) -> Range<usize> {
    let lo = ((from_x / tile).floor().max(0.0) as usize).min(count);
    let hi = ((to_x / tile).ceil().max(0.0) as usize).min(count);
    lo..hi.max(lo)
}

/// The peaks of [from, to) ms of a file's `len` peaks (WAVEFORM_RATE a second).
pub fn peak_range(len: usize, from: i64, to: i64) -> Range<usize> {
    let rate = WAVEFORM_RATE as f64;
    let first = ((from as f64 / 1000.0) * rate).floor().max(0.0) as usize;
    let last = (((to as f64 / 1000.0) * rate).ceil().max(0.0) as usize).min(len);
    first.min(last)..last
}

/// A file's length from its peaks, ms.
pub fn waveform_length(peaks: usize) -> i64 {
    (peaks as f64 * 1000.0 / WAVEFORM_RATE as f64) as i64
}

/// One loudness bar: x from..to, y from..to, px in the box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    pub x0: f64,
    pub x1: f64,
    pub y0: f64,
    pub y1: f64,
}

/// Width of a bar where the peaks are spread out (the SVG's 0.7 px stroke).
const BAR_W: f64 = 0.7;

/// Mirrored loudness bars for `peaks` stretched across a `width` × `height` box, only
/// those reaching [from_x, to_x). Each peak is a bar about the middle line, 48% of the
/// height either way at full loudness; where peaks are denser than a pixel, each pixel
/// column shows the loudest of its peaks (what the overlapping strokes drew).
pub fn waveform_bars(peaks: &[u8], width: f64, height: f64, from_x: f64, to_x: f64) -> Vec<Bar> {
    let n = peaks.len();
    if n == 0 || width <= 0.0 {
        return Vec::new();
    }
    let mid = height / 2.0;
    let bar = |x0: f64, x1: f64, p: u8| {
        let half = p as f64 / 255.0 * 0.48 * height;
        Bar { x0, x1, y0: mid - half, y1: mid + half }
    };
    let step = width / n as f64;
    let (from_x, to_x) = (from_x.max(0.0), to_x.min(width));
    if from_x >= to_x {
        return Vec::new();
    }
    if step >= 1.0 {
        let lo = ((from_x / step).floor() as usize).saturating_sub(1);
        let hi = ((to_x / step).ceil() as usize + 1).min(n);
        return (lo..hi)
            .filter(|&i| peaks[i] > 0)
            .map(|i| {
                let x = (i as f64 + 0.5) * step;
                bar(x - BAR_W / 2.0, x + BAR_W / 2.0, peaks[i])
            })
            .collect();
    }
    let (lo, hi) = (from_x.floor() as usize, (to_x.ceil() as usize).min(width.ceil() as usize));
    (lo..hi)
        .filter_map(|c| {
            let first = ((c as f64 / step).floor() as usize).min(n - 1);
            let last = (((c + 1) as f64 / step).ceil() as usize).clamp(first + 1, n);
            let p = peaks[first..last].iter().copied().max().unwrap_or(0);
            (p > 0).then(|| bar(c as f64, (c as f64 + 1.0).min(width), p))
        })
        .collect()
}

/// The hatching past the end of the video: the gray bands of a 135° 6 px / 6 px stripe
/// across a `width` × `height` box, as parallelograms (corners, px in the box), only
/// those reaching [from_x, to_x).
pub fn hatch_bands(width: f64, height: f64, from_x: f64, to_x: f64) -> Vec<[(f64, f64); 4]> {
    // Along the gradient (down and right) a band is gray where (x + y) / √2 mod 12 ≥ 6.
    let unit = 12.0 * std::f64::consts::SQRT_2;
    let (from_x, to_x) = (from_x.max(0.0), to_x.min(width));
    let first = (from_x / unit).floor() as i64;
    let last = ((to_x + height) / unit).ceil() as i64;
    (first..=last)
        .map(|k| {
            let a = k as f64 * unit + unit / 2.0;
            let b = (k + 1) as f64 * unit;
            [(a, 0.0), (b, 0.0), (b - height, height), (a - height, height)]
        })
        .collect()
}

/// A dissolve's blend as the inspector names it (TRANSITION_EFFECTS in project.ts).
pub fn effect_label(effect: &str) -> &'static str {
    match effect {
        "fade" => "Dissolve",
        "slideleft" => "Slide",
        "slideup" => "Slide up",
        "coverleft" => "Cover",
        "wipeleft" => "Wipe",
        "smoothleft" => "Smooth wipe",
        "zoomin" => "Zoom",
        "circleopen" => "Circle",
        "radial" => "Clock",
        "fadewhite" => "Flash",
        _ => "Blend",
    }
}

/// A split framing's panel count as the chip shows it ("Split 2"); None for others.
pub fn split_label(framing: &str) -> Option<&'static str> {
    match framing {
        "split" => Some("2"),
        "split3" => Some("3"),
        "split4" => Some("4"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clip_engine::render::project::MediaFile;

    fn piece(id: &str, a: i64, b: i64) -> VideoItem {
        VideoItem { id: id.into(), src_in_ms: a, src_out_ms: b, ..VideoItem::default() }
    }

    fn word(start: i64, end: f64) -> EditedWord {
        EditedWord { seg: 0, word: 0, text: "w".into(), start, end, piece: 0 }
    }

    #[test]
    fn pixels_and_time_round_trip() {
        assert_eq!(x_at(0.0, 60.0), GUTTER);
        assert_eq!(x_at(2000.0, 60.0), GUTTER + 120.0);
        // 30 fps snaps to frames; outside the edit clamps.
        assert_eq!(time_at(GUTTER + 120.0, 60.0, "30", 10_000), 2000.0);
        // 1016.7 ms is frame 30.5: rounds up to frame 31.
        assert_eq!(time_at(GUTTER + 61.0, 60.0, "30", 10_000), 1033.0);
        assert_eq!(time_at(0.0, 60.0, "30", 10_000), 0.0);
        assert_eq!(time_at(1e6, 60.0, "30", 10_000), 10_000.0);
    }

    #[test]
    fn fit_fills_the_view_but_not_below_min() {
        assert_eq!(fit_zoom(1032.0, 10_000, 8.0), 100.0);
        assert_eq!(fit_zoom(232.0, 3_600_000, 8.0), 8.0);
    }

    #[test]
    fn zoom_keeps_the_time_under_the_pointer() {
        let (zoom, scroll) = zoom_about(100.0, 2.0, 8.0, 400.0, 300.0, 500.0).unwrap();
        assert_eq!(zoom, 200.0);
        // The time under the pointer before and after.
        let before = (300.0 + 500.0 - GUTTER) / 100.0;
        let after = (300.0 + scroll - GUTTER) / zoom;
        assert!((before - after).abs() < 1e-9);
        // Clamped at the bounds; nothing to do at a bound.
        assert_eq!(zoom_about(300.0, 2.0, 8.0, 400.0, 0.0, 0.0).unwrap().0, 400.0);
        assert_eq!(zoom_about(400.0, 2.0, 8.0, 400.0, 0.0, 0.0), None);
        assert_eq!(zoom_about(8.0, 0.5, 8.0, 400.0, 0.0, 0.0), None);
    }

    #[test]
    fn wheel_steps_are_bounded() {
        assert!((wheel_zoom_factor(-100.0) - 0.5_f64.exp()).abs() < 1e-12);
        assert!((wheel_zoom_factor(50.0) - (-0.5_f64).exp()).abs() < 1e-12);
        assert_eq!(wheel_zoom_factor(0.0), 1.0);
    }

    #[test]
    fn width_fills_the_view() {
        assert_eq!(content_width(10_000, 60.0, 100.0), 600.0 + GUTTER * 2.0);
        assert_eq!(content_width(10_000, 60.0, 1000.0), 1000.0);
    }

    #[test]
    fn ruler_ticks_cover_only_the_view() {
        // 60 px/s: a label each second.
        let ticks = ruler_ticks(60.0, 10_000.0, 0.0, 200.0);
        assert_eq!(ticks[0], (GUTTER, 0.0));
        assert_eq!(ticks[1], (GUTTER + 60.0, 1.0));
        assert_eq!(ticks.len(), 5);
        let later = ruler_ticks(60.0, 10_000.0, 6000.0, 6200.0);
        assert_eq!(later.first().unwrap().1, 98.0);
        assert!(later.iter().all(|&(x, _)| x >= 6000.0 - 120.0 && x <= 6200.0 + 60.0));
        // The ruler stops at the content's end.
        let last = ruler_ticks(60.0, 136.0, 0.0, 1e9);
        assert_eq!(last.len(), 3);
        // At 20 px/s labels are 5 s apart (tick_step).
        assert_eq!(ruler_ticks(20.0, 1000.0, 0.0, 400.0)[1].1, 5.0);
    }

    #[test]
    fn tracks_stack_top_to_bottom() {
        let only = track_layout(0, 0, 0, false, 120.0, 0);
        assert_eq!(only, TrackLayout { overlays: None, pip: None, texts: None, words: None, main: 32.0, sounds: None, height: 160.0 });
        let all = track_layout(2, 1, 2, true, 100.0, 2);
        assert_eq!(all.overlays, Some(32.0));
        assert_eq!(all.pip, Some(32.0 + 44.0 + 8.0));
        assert_eq!(all.texts, Some(84.0 + 26.0 + 8.0));
        assert_eq!(all.words, Some(118.0 + 52.0 + 8.0));
        assert_eq!(all.main, 178.0 + 20.0 + 8.0);
        assert_eq!(all.sounds, Some(206.0 + 100.0 + 8.0));
        assert_eq!(all.height, 314.0 + 68.0 + 8.0);
        let lanes: HashMap<String, usize> = [("a".to_string(), 0), ("b".to_string(), 2)].into();
        assert_eq!(lane_rows(&lanes), 3);
        assert_eq!(lane_rows(&HashMap::new()), 0);
    }

    #[test]
    fn overlay_drags_stay_inside_and_long_enough() {
        let b = OverlayBox { x: 0.5, y: 0.5, scale: 1.0, start_ms: 1000, end_ms: 3000 };
        assert_eq!(drag_overlay(&b, None, 500, 10_000).0.start_ms, 1500);
        assert_eq!(drag_overlay(&b, None, 500, 10_000).0.end_ms, 3500);
        assert_eq!(drag_overlay(&b, None, -5000, 10_000).0.start_ms, 0);
        let (end, _) = drag_overlay(&b, None, 9000, 10_000);
        assert_eq!((end.start_ms, end.end_ms), (8000, 10_000));
        let (trim, seek) = drag_overlay(&b, Some(Edge::In), 5000, 10_000);
        assert_eq!((trim.start_ms, trim.end_ms, seek), (2800, 3000, 2800));
        let (trim, seek) = drag_overlay(&b, Some(Edge::Out), -5000, 10_000);
        assert_eq!((trim.start_ms, trim.end_ms, seek), (1000, 1200, 1200));
        assert_eq!(drag_overlay(&b, Some(Edge::Out), 50_000, 10_000).0.end_ms, 10_000);
    }

    #[test]
    fn pieces_drop_where_their_middle_lands() {
        let video = vec![piece("a", 0, 1000), piece("b", 0, 1000), piece("c", 0, 1000)];
        assert_eq!(drop_index(&video, 0, 0.0), 0);
        assert_eq!(drop_index(&video, 0, 600.0), 1);
        assert_eq!(drop_index(&video, 0, 1600.0), 2);
        assert_eq!(drop_index(&video, 2, -1200.0), 1);
        assert_eq!(drop_index(&video, 2, -2400.0), 0);
        assert_eq!(drop_index(&video, 5, 0.0), 5);
    }

    #[test]
    fn trims_stop_at_the_file() {
        let mut project: Project = serde_json::from_value(serde_json::json!({
            "version": 1, "source_width": 1920, "source_height": 1080, "width": 1080, "height": 1920,
            "fps": "30", "aspect_ratio": "9:16", "video": [], "shots": [], "face_samples": [], "transcript": [],
            "captions": true, "caption_preset": "pop", "emphasis_words": [], "speed": 1.0,
            "title": null, "title_box": null, "banner_box": null, "banner_platform": null, "banner_channel_url": null,
            "logos": []
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        project.media.push(MediaFile { file: "m.mp4".into(), name: "M".into(), width: 1, height: 1, duration_ms: 4000, image: false });
        let source = piece("a", 0, 1000);
        assert_eq!(trim_bound(&project, &source, 60_000), 60_000);
        let added = VideoItem { media: Some("m.mp4".into()), ..piece("b", 0, 1000) };
        assert_eq!(trim_bound(&project, &added, 60_000), 4000);
        let missing = VideoItem { media: Some("gone.mp4".into()), ..piece("c", 0, 1500) };
        assert_eq!(trim_bound(&project, &missing, 60_000), 1500);
    }

    #[test]
    fn word_drags_move_or_retime() {
        let w = word(1000, 1400.0);
        assert_eq!(drag_word(&w, None, 200.0, 10_000), (1200.0, 1600.0, 1200.0));
        assert_eq!(drag_word(&w, Some(Edge::In), -200.0, 10_000), (800.0, 1400.0, 800.0));
        assert_eq!(drag_word(&w, Some(Edge::Out), 100.0, 10_000), (1000.0, 1500.0, 1500.0));
        assert_eq!(drag_word(&w, None, -5000.0, 10_000).2, 0.0);
    }

    #[test]
    fn tab_walks_the_words() {
        // Enter closes; Tab goes on; Shift+Tab back; the ends close.
        assert_eq!(next_word(2, false, true, false, 5), None);
        assert_eq!(next_word(2, false, true, true, 5), Some(3));
        assert_eq!(next_word(2, true, true, true, 5), Some(1));
        assert_eq!(next_word(4, false, true, true, 5), None);
        assert_eq!(next_word(0, true, true, true, 5), None);
        // A deleted word: the next one moves into its place.
        assert_eq!(next_word(2, false, false, true, 5), Some(2));
        assert_eq!(next_word(4, false, false, true, 5), None);
        assert_eq!(clean_word("  a \t b  "), "a b");
    }

    #[test]
    fn only_words_in_view() {
        let words: Vec<EditedWord> = (0..1000).map(|i| word(i * 500, (i * 500 + 400) as f64)).collect();
        let r = visible_words(&words, 100_000.0, 110_000.0);
        assert!(r.start <= 200 && r.end == 220);
        assert!(r.start >= 140);
        assert_eq!(visible_words(&words, 1e9, 2e9), 1000..1000);
        assert_eq!(visible_words(&[], 0.0, 1.0), 0..0);
    }

    #[test]
    fn filmstrip_tiles_follow_the_export_shape() {
        // A 16:9 source in a 9:16 export at 90 px: 50.625 px tiles of 160 px frames.
        let t = filmstrip_tiles(0, 10_000, 500.0, 90.0, 16.0 / 9.0, 9.0 / 16.0);
        assert_eq!(t.frame_width, 160.0);
        assert_eq!(t.tile, 50.625);
        assert_eq!(t.seconds.len(), 10);
        assert_eq!(t.seconds[0], 0);
        assert_eq!(t.seconds[9], 9);
        // Never past the end; tiny tiles grow to 8 px.
        let t = filmstrip_tiles(61_000, 62_000, 4.0, 4.0, 1.0, 1.0);
        assert_eq!((t.tile, t.seconds.clone()), (8.0, vec![61]));
        assert_eq!(block_of(61), (1, 1));
        assert_eq!(block_of(59), (0, 59));
        assert_eq!(blocks_for(&[0, 30, 59, 60, 61, 130, 2], 0..7), vec![0, 1, 2]);
        assert_eq!(blocks_for(&[0, 60], 1..9), vec![1]);
        assert_eq!(visible_tiles(10.0, 100, 25.0, 55.0), 2..6);
        assert_eq!(visible_tiles(10.0, 3, 25.0, 55.0), 2..3);
        assert_eq!(visible_tiles(10.0, 3, -50.0, -1.0), 0..0);
    }

    #[test]
    fn waveform_slices_and_bars() {
        assert_eq!(peak_range(100, 1000, 2000), 20..40);
        assert_eq!(peak_range(30, 1000, 2000), 20..30);
        assert_eq!(peak_range(10, 1000, 2000), 10..10);
        assert_eq!(waveform_length(40), 2000);
        // Spread out: one 0.7 px bar a peak, centred in its slot, 48% of the height either way.
        let bars = waveform_bars(&[255, 0, 128], 30.0, 100.0, 0.0, 30.0);
        assert_eq!(bars.len(), 2);
        assert!((bars[0].x0 - 4.65).abs() < 1e-9 && (bars[0].y0 - 2.0).abs() < 1e-9 && (bars[0].y1 - 98.0).abs() < 1e-9);
        assert!((bars[1].x0 - 24.65).abs() < 1e-9);
        // Only what's in view.
        assert_eq!(waveform_bars(&[255; 100], 100.0, 10.0, 50.0, 60.0).len(), 12);
        // Dense: the loudest peak in each pixel column.
        let dense = waveform_bars(&[10, 200, 30, 40], 2.0, 100.0, 0.0, 2.0);
        assert_eq!(dense.len(), 2);
        assert_eq!((dense[0].x0, dense[0].x1), (0.0, 1.0));
        assert!((dense[0].y0 - (50.0 - 200.0 / 255.0 * 48.0)).abs() < 1e-9);
        assert!((dense[1].y0 - (50.0 - 40.0 / 255.0 * 48.0)).abs() < 1e-9);
        assert!(waveform_bars(&[], 10.0, 10.0, 0.0, 10.0).is_empty());
    }

    #[test]
    fn hatching_covers_the_view() {
        let bands = hatch_bands(100.0, 20.0, 0.0, 100.0);
        // Every gray band's top edge is 12√2 apart.
        let unit = 12.0 * std::f64::consts::SQRT_2;
        assert!((bands[1][0].0 - bands[0][0].0 - unit).abs() < 1e-9);
        assert!(bands.first().unwrap()[1].0 > 0.0);
        assert!(bands.last().unwrap()[3].0 >= 100.0 - unit);
    }

    #[test]
    fn labels() {
        assert_eq!(effect_label("radial"), "Clock");
        assert_eq!(effect_label("nope"), "Blend");
        assert_eq!(split_label("split3"), Some("3"));
        assert_eq!(split_label("crop"), None);
    }
}
