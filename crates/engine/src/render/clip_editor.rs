//! Clip Editor - decides which parts of a clip's timeline to keep. Port of clip_editor.py.
//!
//! "Tight" pacing removes dead air and filler words ("um", "uh") the way a
//! short-form editor would, while protecting moments that only look like
//! silence: screen demos, laughter and applause.
//!
//! All times here are milliseconds from the start of the render window. The
//! result is a list of keep intervals plus a TimeMap that converts window time
//! to output time, which captions, layout switches and overlays share so
//! everything stays in sync after the cuts.
use super::pyfmt;
use crate::layout::types::{ClipLayoutPlan, ShotLayout, SCREEN, SCREEN_CAM, TALKING_HEAD, TWO_SHOT};
use crate::types::{TranscriptSegment, TranscriptWord};

pub const TIGHT: &str = "tight";
pub const NATURAL: &str = "natural";

/// Only pure hesitation sounds: words like "like", "so" or "ah" often carry meaning.
pub const FILLER_WORDS: [&str; 11] = ["um", "uh", "uhm", "umm", "uhh", "erm", "er", "hmm", "hm", "mm", "mhm"];

/// Longest pause kept as-is, per layout. Silence on a talking head is dead air;
/// on a screen recording it is usually something happening on screen (None: never cut).
pub fn max_pause_ms(layout: &str) -> Option<i64> {
    match layout {
        TALKING_HEAD | TWO_SHOT => Some(700),
        SCREEN_CAM => Some(2000),
        SCREEN => None,
        _ => Some(700),
    }
}

pub const KEEP_PAUSE_MS: i64 = 260;
pub const LEAD_IN_MS: i64 = 120;
pub const TAIL_MS: i64 = 380;
pub const MIN_CUT_MS: i64 = 200;
pub const MIN_PIECE_MS: i64 = 400;
pub const FRAME_MS: f64 = 1000.0 / 30.0;
pub const LONGFORM_MIN_PAUSE_MS: i64 = 1300;
pub const LONGFORM_KEEP_PAUSE_MS: i64 = 450;
/// Pause threshold when shot content is unknown: the screen+webcam limit.
pub const UNKNOWN_MAX_PAUSE_MS: i64 = 2000;

pub fn is_filler(word: &str) -> bool {
    let key: String = word.to_lowercase().chars().filter(|c| c.is_ascii_lowercase()).collect();
    FILLER_WORDS.contains(&key.as_str())
}

fn snap(t: f64) -> i64 {
    pyfmt::round(pyfmt::round(t / FRAME_MS) as f64 * FRAME_MS)
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowWord {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// Words inside the window, in window time.
pub fn window_words(segments: &[TranscriptSegment], window_start_ms: i64, window_ms: i64) -> Vec<WindowWord> {
    let mut words = Vec::new();
    for seg in segments {
        for w in &seg.words {
            let (s, e) = (w.start_time_ms - window_start_ms, w.end_time_ms - window_start_ms);
            if e > 0 && s < window_ms {
                words.push(WindowWord { start_ms: s.max(0), end_ms: e.min(window_ms), text: w.word.clone() });
            }
        }
    }
    words.sort_by_key(|w| w.start_ms);
    words
}

/// Gaps right after sentences that drew a reaction (laughter, applause).
pub fn reaction_intervals(segments: &[TranscriptSegment], window_start_ms: i64, window_ms: i64) -> Vec<(i64, i64)> {
    let mut ordered: Vec<&TranscriptSegment> = segments.iter().collect();
    ordered.sort_by_key(|s| s.start_time_ms);
    let mut protected = Vec::new();
    for (i, seg) in ordered.iter().enumerate() {
        if seg.audio_events.is_empty() {
            continue;
        }
        let start = seg.end_time_ms - window_start_ms;
        let end = ordered.get(i + 1).map(|n| n.start_time_ms).unwrap_or(seg.end_time_ms + 3000) - window_start_ms;
        if end > 0 && start < window_ms {
            protected.push((start.max(0), end.min(window_ms)));
        }
    }
    protected
}

fn max_pause_at(plan: Option<&ClipLayoutPlan>, t_ms: i64) -> Option<i64> {
    let Some(plan) = plan else { return Some(UNKNOWN_MAX_PAUSE_MS) };
    let shot = plan.shots.iter().find(|s| s.start_ms <= t_ms && t_ms < s.end_ms);
    let layout = match shot {
        Some(s) => s.detected_layout.as_deref().filter(|l| !l.is_empty()).unwrap_or(&s.layout),
        None => TALKING_HEAD,
    };
    max_pause_ms(layout)
}

fn tuple_cmp(a: &(f64, f64), b: &(f64, f64)) -> std::cmp::Ordering {
    a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
}

/// Intervals of the window to keep under tight pacing.
/// `longform` keeps pauses up to LONGFORM_MIN_PAUSE_MS (fillers are still cut).
pub fn compute_keep_intervals(
    words: &[WindowWord],
    window_ms: i64,
    plan: Option<&ClipLayoutPlan>,
    protected: &[(i64, i64)],
    longform: bool,
) -> Vec<(i64, i64)> {
    let keep_pause_ms = if longform { LONGFORM_KEEP_PAUSE_MS } else { KEEP_PAUSE_MS };
    let pause_limit = |t_ms: i64| -> Option<i64> {
        let threshold = max_pause_at(plan, t_ms);
        match threshold {
            Some(t) if longform => Some(t.max(LONGFORM_MIN_PAUSE_MS)),
            other => other,
        }
    };

    let spoken: Vec<&WindowWord> = words.iter().filter(|w| !is_filler(&w.text)).collect();
    if spoken.is_empty() {
        return vec![(0, window_ms)];
    }

    let mut cuts: Vec<(f64, f64)> = Vec::new();
    let propose = |cuts: &mut Vec<(f64, f64)>, gap_start: i64, gap_end: i64, keep_ms: i64, threshold: Option<i64>| {
        let Some(threshold) = threshold else { return };
        if gap_end - gap_start <= threshold.max(keep_ms) {
            return;
        }
        cuts.push(((gap_start + keep_ms / 2) as f64, (gap_end - keep_ms / 2) as f64));
    };

    // Lead-in and tail silence.
    let (first, last) = (spoken[0], spoken[spoken.len() - 1]);
    if pause_limit(first.start_ms).is_some() && first.start_ms > LEAD_IN_MS + MIN_CUT_MS {
        cuts.push((0.0, (first.start_ms - LEAD_IN_MS) as f64));
    }
    if pause_limit(last.end_ms).is_some() && window_ms - last.end_ms > TAIL_MS + MIN_CUT_MS {
        cuts.push(((last.end_ms + TAIL_MS) as f64, window_ms as f64));
    }

    // Pauses between words. A gap that swallowed a filler word is always
    // compressed; plain pauses only past the layout's threshold.
    let mut all_words: Vec<&WindowWord> = words.iter().collect();
    all_words.sort_by_key(|w| w.start_ms);
    for pair in spoken.windows(2) {
        let (prev, nxt) = (pair[0], pair[1]);
        let fillers: Vec<&&WindowWord> = all_words
            .iter()
            .filter(|w| prev.end_ms <= w.start_ms && w.end_ms <= nxt.start_ms && is_filler(&w.text))
            .collect();
        let mut threshold = pause_limit(prev.end_ms);
        if !fillers.is_empty() && threshold.is_some() {
            threshold = Some(0);
        }
        let before = cuts.len();
        propose(&mut cuts, prev.end_ms, nxt.start_ms, keep_pause_ms, threshold);
        if !fillers.is_empty() && cuts.len() > before {
            // Frame rounding must remove each filler in full.
            let (cut_start, cut_end) = cuts[cuts.len() - 1];
            let n = cuts.len() - 1;
            cuts[n] = (
                cut_start.min(fillers[0].start_ms as f64 - FRAME_MS),
                cut_end.max(fillers[fillers.len() - 1].end_ms as f64 + FRAME_MS),
            );
        }
    }

    // Never cut into protected moments; drop cuts too short to matter.
    cuts.sort_by(tuple_cmp);
    let mut trimmed: Vec<(f64, f64)> = Vec::new();
    for &(start0, end) in &cuts {
        let mut start = start0;
        for &(p_start, p_end) in protected {
            let (p_start, p_end) = (p_start as f64, p_end as f64);
            if start < p_end && end > p_start {
                if p_start - start >= MIN_CUT_MS as f64 {
                    trimmed.push((start, p_start));
                }
                start = start.max(p_end);
                if start >= end {
                    break;
                }
            }
        }
        if end - start >= MIN_CUT_MS as f64 {
            trimmed.push((start, end));
        }
    }

    // Snap to the frame grid, then invert cuts into keeps.
    trimmed.sort_by(tuple_cmp);
    let mut keeps: Vec<(i64, i64)> = Vec::new();
    let mut cursor: i64 = 0;
    for (start, end) in trimmed {
        let start = snap(start.max(cursor as f64));
        // A cut through the end of the window must stay flush with that end.
        let end = if end >= window_ms as f64 { window_ms } else { snap(end) };
        if end - start < MIN_CUT_MS {
            continue;
        }
        if start > cursor {
            keeps.push((cursor, start));
        }
        cursor = end;
    }
    if cursor < window_ms {
        keeps.push((cursor, window_ms));
    }

    // Absorb slivers: a tiny piece is glued back to its neighbour.
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for piece in keeps {
        if !merged.is_empty() && piece.1 - piece.0 < MIN_PIECE_MS {
            let n = merged.len() - 1;
            merged[n] = (merged[n].0, piece.1);
        } else {
            merged.push(piece);
        }
    }
    if merged.len() > 1 && merged[0].1 - merged[0].0 < MIN_PIECE_MS {
        let joined = (merged[0].0, merged[1].1);
        merged.splice(0..2, [joined]);
    }
    if merged.is_empty() {
        vec![(0, window_ms)]
    } else {
        merged
    }
}

/// `keeps` with every `cuts` interval removed; pieces under MIN_PIECE_MS drop.
pub fn subtract_intervals(keeps: &[(i64, i64)], cuts: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut sorted = cuts.to_vec();
    sorted.sort();
    let mut result = keeps.to_vec();
    for (c_start, c_end) in sorted {
        let (c_start, c_end) = (snap(c_start as f64), snap(c_end as f64));
        if c_end <= c_start {
            continue;
        }
        let mut pieces = Vec::new();
        for &(k_start, k_end) in &result {
            if c_end <= k_start || c_start >= k_end {
                pieces.push((k_start, k_end));
                continue;
            }
            if c_start - k_start >= MIN_PIECE_MS {
                pieces.push((k_start, c_start));
            }
            if k_end - c_end >= MIN_PIECE_MS {
                pieces.push((c_end, k_end));
            }
        }
        result = pieces;
    }
    if result.is_empty() {
        keeps.to_vec()
    } else {
        result
    }
}

/// Maps window time to output time for a list of keep intervals.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeMap {
    pub keeps: Vec<(i64, i64)>,
    pub window_ms: i64,
    offsets: Vec<i64>,
    pub output_ms: i64,
}

impl TimeMap {
    /// `window_ms` None means the end of the last keep.
    pub fn new(keeps: Vec<(i64, i64)>, window_ms: Option<i64>) -> Self {
        let window_ms = window_ms.unwrap_or_else(|| keeps.last().map(|k| k.1).unwrap_or(0));
        let mut offsets = Vec::with_capacity(keeps.len());
        let mut total = 0;
        for &(start, end) in &keeps {
            offsets.push(total);
            total += end - start;
        }
        Self { keeps, window_ms, offsets, output_ms: total }
    }

    pub fn removed_ms(&self) -> i64 {
        self.window_ms - self.output_ms
    }

    pub fn has_cuts(&self) -> bool {
        self.removed_ms() > 0
    }

    pub fn cut_count(&self) -> usize {
        if !self.has_cuts() || self.keeps.is_empty() {
            return 0;
        }
        self.keeps.len() - 1
            + usize::from(self.keeps[0].0 > 0)
            + usize::from(self.keeps[self.keeps.len() - 1].1 < self.window_ms)
    }

    /// Output time of a window instant, or None if it was cut.
    pub fn to_output(&self, t_ms: i64) -> Option<i64> {
        self.keeps
            .iter()
            .zip(&self.offsets)
            .find(|((s, e), _)| *s <= t_ms && t_ms <= *e)
            .map(|((s, _), off)| off + t_ms - s)
    }

    /// Output time of a window instant; cut instants map to the next kept frame.
    pub fn to_output_clamped(&self, t_ms: i64) -> i64 {
        for (&(start, end), &offset) in self.keeps.iter().zip(&self.offsets) {
            if t_ms < start {
                return offset;
            }
            if t_ms <= end {
                return offset + t_ms - start;
            }
        }
        self.output_ms
    }

    /// Kept sub-intervals of [start_ms, end_ms).
    pub fn pieces_within(&self, start_ms: i64, end_ms: i64) -> Vec<(i64, i64)> {
        self.keeps
            .iter()
            .filter_map(|&(ks, ke)| {
                let (s, e) = (start_ms.max(ks), end_ms.min(ke));
                (e - s > 0).then_some((s, e))
            })
            .collect()
    }
}

/// Copy of the plan with shot boundaries in output time (for overlays/captions).
pub fn remap_plan(plan: &ClipLayoutPlan, time_map: &TimeMap) -> ClipLayoutPlan {
    let mut shots: Vec<ShotLayout> = Vec::new();
    for shot in &plan.shots {
        let (start, end) = (time_map.to_output_clamped(shot.start_ms), time_map.to_output_clamped(shot.end_ms));
        if end > start {
            let mut copy = shot.clone();
            copy.start_ms = start;
            copy.end_ms = end;
            shots.push(copy);
        }
    }
    if let Some(last) = shots.last_mut() {
        last.end_ms = time_map.output_ms;
    }
    if shots.is_empty() {
        shots = plan.shots.iter().take(1).cloned().collect();
    }
    ClipLayoutPlan {
        shots,
        source_width: plan.source_width,
        source_height: plan.source_height,
        vision_cost_usd: plan.vision_cost_usd,
        face_samples: Vec::new(),
    }
}

/// Transcript with filler words dropped and word times moved to output time.
/// Times stay absolute (window_start + output offset).
pub fn remap_segments(segments: &[TranscriptSegment], window_start_ms: i64, time_map: &TimeMap) -> Vec<TranscriptSegment> {
    let mut remapped = Vec::new();
    for seg in segments {
        if seg.words.is_empty() {
            // Segment-level transcript (no word timings): keep it whole.
            let start = time_map.to_output_clamped(seg.start_time_ms - window_start_ms);
            let end = time_map.to_output_clamped(seg.end_time_ms - window_start_ms);
            if !seg.text.is_empty() && end > start {
                remapped.push(TranscriptSegment {
                    start_time_ms: window_start_ms + start,
                    end_time_ms: window_start_ms + end,
                    text: seg.text.clone(),
                    speaker_label: seg.speaker_label.clone(),
                    words: Vec::new(),
                    audio_events: Vec::new(),
                });
            }
            continue;
        }
        let mut words = Vec::new();
        for w in &seg.words {
            // Only hide a filler when the corresponding audio was removed.
            let relative_start = w.start_time_ms - window_start_ms;
            let relative_end = w.end_time_ms - window_start_ms;
            if time_map.pieces_within(relative_start, relative_end).is_empty() {
                continue;
            }
            let start = time_map.to_output(relative_start);
            let end = time_map.to_output(relative_end);
            if start.is_none() && end.is_none() {
                continue;
            }
            let start = start.unwrap_or_else(|| time_map.to_output_clamped(relative_start));
            let end = end.unwrap_or(start + 1);
            words.push(TranscriptWord {
                word: w.word.clone(),
                start_time_ms: window_start_ms + start,
                end_time_ms: window_start_ms + end.max(start + 1),
            });
        }
        if words.is_empty() {
            continue;
        }
        remapped.push(TranscriptSegment {
            start_time_ms: words[0].start_time_ms,
            end_time_ms: words[words.len() - 1].end_time_ms,
            text: words.iter().map(|w| w.word.as_str()).collect::<Vec<_>>().join(" "),
            speaker_label: seg.speaker_label.clone(),
            words,
            audio_events: seg.audio_events.clone(),
        });
    }
    remapped
}
