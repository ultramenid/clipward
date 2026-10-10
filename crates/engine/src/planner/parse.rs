//! Planner response parsing: duration bounds, range clamping, sentence snapping,
//! longform skips/chapters, rubric scoring and overlap filtering.
//! Port of `IntelligencePlannerService._parse_clip_plan_response` and helpers.
use super::boundaries::{
    find_sentence_end_boundary, find_sentence_start_boundary, last_sentence_end_between,
    next_start_at_or_after, sentence_end_forward, Direction,
};
use super::config::{resolve_clip_duration_bounds, PlannerSettings};
use super::prompts::RUBRIC_DIMENSIONS;
use crate::types::{ClipPlanSegment, TranscriptSegment};
use regex::Regex;
use serde_json::{Map, Value};
use std::sync::OnceLock;

/// Two clips may share at most this much footage before the weaker one is dropped.
pub const MAX_CLIP_OVERLAP_MS: i64 = 5000;
/// Longform skips: shorter than this is pacing's job, not a content cut.
pub const MIN_SKIP_MS: i64 = 8000;
/// A skip never removes more than this share of the clip's footage.
pub const MAX_SKIP_SHARE: f64 = 0.4;
/// Chapters closer together than this are merged (YouTube needs 10 s minimum).
pub const MIN_CHAPTER_GAP_MS: i64 = 20000;

/// A planning failure. Every parse failure is retryable.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanError {
    pub message: String,
    pub retryable: bool,
}

impl PlanError {
    pub fn retry(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }
}

/// What the parser needs to know about the current plan (the Python `_current_*` state).
#[derive(Debug, Clone, Default)]
pub struct ParseContext<'a> {
    pub min_duration: Option<i64>,
    pub max_duration: Option<i64>,
    pub duration_ranges: Option<&'a [String]>,
    pub start_limit: Option<f64>,
    pub end_limit: Option<f64>,
    pub transcript: &'a [TranscriptSegment],
    pub video_duration: Option<f64>,
    pub longform: bool,
    pub visual_frame_times: &'a [f64],
    /// Drop clips entirely outside start/end_limit instead of clamping them in.
    pub drop_outside_range: bool,
    /// The prompt showed line times in whole seconds, floored (see `line_edge`).
    pub whole_second_lines: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedPlan {
    pub segments: Vec<ClipPlanSegment>,
    pub insights: Option<String>,
}

/// A JSON number the way Python arithmetic sees it (bools are 0/1).
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Python `float(x)`: numbers, bools and numeric strings.
fn py_float(value: &Value) -> Option<f64> {
    match value {
        Value::String(s) => {
            let t = s.trim().to_ascii_lowercase().replace('_', "");
            match t.as_str() {
                "nan" | "+nan" | "-nan" => Some(f64::NAN),
                "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
                "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
                _ => t.parse().ok(),
            }
        }
        other => number(other),
    }
}

/// `clip.get("start_time", clip.get("startTime", clip.get("start", 0)))`.
fn clip_time(clip: &Map<String, Value>, keys: [&str; 3]) -> Result<f64, PlanError> {
    for key in keys {
        if let Some(v) = clip.get(key) {
            return number(v)
                .ok_or_else(|| PlanError::retry("Failed to parse clip plan: invalid clip time"));
        }
    }
    Ok(0.0)
}

/// Python `round(x, digits)`: rounds the exact binary value, like float formatting.
pub(crate) fn round_to(x: f64, digits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.digits$}").parse().unwrap_or(x)
}

// max/min (not clamp) so NaN scores become 0 as in Python.
#[allow(clippy::manual_clamp)]
/// Virality score in [0, 1]: mean of the rubric scores / 10, else a model-supplied
/// `virality_score`, else 0.5.
pub fn score_clip(clip: &Map<String, Value>) -> f64 {
    if let Some(Value::Object(scores)) = clip.get("scores") {
        let values: Vec<f64> = RUBRIC_DIMENSIONS
            .iter()
            .filter_map(|dim| scores.get(*dim).and_then(py_float))
            .map(|v| v.max(0.0).min(10.0))
            .collect();
        if !values.is_empty() {
            return round_to(values.iter().sum::<f64>() / values.len() as f64 / 10.0, 3);
        }
    }
    match clip.get("virality_score") {
        None => 0.5,
        Some(v) => py_float(v).map(|f| f.max(0.0).min(1.0)).unwrap_or(0.5),
    }
}

fn json_extract_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[\[{][\s\S]*[\]}]").unwrap())
}

/// Parse a planner reply into validated clips.
pub fn parse_clip_plan_response(
    content: Option<&str>,
    finish_reason: Option<&str>,
    ctx: &ParseContext,
    settings: &PlannerSettings,
) -> Result<ParsedPlan, PlanError> {
    let content = match content {
        Some(c) if !c.is_empty() => c,
        _ => {
            return Err(PlanError::retry(format!(
                "Planner returned no content (finish_reason={})",
                finish_reason.unwrap_or("None")
            )))
        }
    };
    if finish_reason == Some("length") {
        tracing::warn!(
            "Planner output hit max_tokens ({}); response may be truncated",
            settings.planner_max_output_tokens
        );
    }
    let parsed: Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => match json_extract_regex().find(content) {
            Some(m) => serde_json::from_str(m.as_str())
                .map_err(|_| PlanError::retry("Failed to parse clip plan: invalid JSON"))?,
            None => {
                return Err(PlanError::retry(
                    "Failed to parse JSON from planner response",
                ))
            }
        },
    };
    let (clips_value, insights) = match &parsed {
        Value::Array(_) => (parsed.clone(), None),
        Value::Object(obj) => (
            obj.get("clips").cloned().unwrap_or(Value::Array(vec![])),
            obj.get("insights")
                .and_then(|i| i.as_str())
                .map(str::to_string),
        ),
        _ => {
            return Err(PlanError::retry(
                "Failed to parse clip plan: unexpected JSON shape",
            ))
        }
    };
    let Value::Array(clips_data) = clips_value else {
        return Err(PlanError::retry(
            "Failed to parse clip plan: clips is not a list",
        ));
    };
    let mut clips = Vec::with_capacity(clips_data.len());
    for clip in &clips_data {
        match clip {
            Value::Object(obj) => clips.push(obj),
            _ => {
                return Err(PlanError::retry(
                    "Failed to parse clip plan: clip is not an object",
                ))
            }
        }
    }

    let (min_duration, max_duration) =
        resolve_clip_duration_bounds(ctx.duration_ranges, ctx.min_duration, ctx.max_duration);
    let (min_f, max_f) = (min_duration as f64, max_duration as f64);
    let start_limit = ctx.start_limit;
    let mut end_limit = ctx.end_limit;
    if let Some(video_duration) = ctx.video_duration.filter(|d| *d > 0.0) {
        end_limit = Some(
            end_limit
                .map(|e| e.min(video_duration))
                .unwrap_or(video_duration),
        );
    }
    let transcript = ctx.transcript;
    let snapping = settings.sentence_snapping_enabled && !transcript.is_empty();

    // Pre-process: drop invalid clips and enforce duration bounds.
    let mut valid: Vec<(&Map<String, Value>, f64, f64)> = Vec::new();
    for clip in clips {
        let mut start = clip_time(clip, ["start_time", "startTime", "start"])?;
        let mut end = clip_time(clip, ["end_time", "endTime", "end"])?;
        // Values too large for seconds are milliseconds.
        if start > 100000.0 {
            start /= 1000.0;
        }
        if end > 100000.0 {
            end /= 1000.0;
        }
        if ctx.whole_second_lines {
            start = line_edge(transcript, start, |s| s.start_time_ms, false);
            end = line_edge(transcript, end, |s| s.end_time_ms, true);
        }
        let mut duration = end - start;
        if duration < 5.0 {
            tracing::warn!("Filtering clip with duration {duration}s - too short (< 5s)");
            continue;
        }
        // A planning part drops clips entirely outside its range: clamping would invent
        // a clip at the seam. (A single call keeps the Python behaviour.)
        if ctx.drop_outside_range
            && (start_limit.is_some_and(|l| end <= l) || end_limit.is_some_and(|l| start >= l))
        {
            tracing::warn!("Filtering clip outside the selected range");
            continue;
        }
        if let Some(limit) = start_limit {
            if start < limit {
                start = limit;
                duration = end - start;
            }
        }
        if let Some(limit) = end_limit {
            if end > limit {
                end = limit;
                duration = end - start;
            }
        }
        if transcript.is_empty() && !ctx.visual_frame_times.is_empty() {
            let supporting: Vec<f64> = ctx
                .visual_frame_times
                .iter()
                .copied()
                .filter(|t| start <= *t && *t <= end)
                .collect();
            if supporting.len() < 2 || supporting.windows(2).any(|w| w[1] - w[0] > 45.0) {
                tracing::warn!("Filtering visual-only clip without two nearby sampled frames");
                continue;
            }
        }
        if duration < min_f {
            let mut new_end = end + (min_f - duration);
            if let Some(limit) = end_limit {
                if new_end > limit {
                    new_end = limit;
                }
            }
            if new_end - start >= min_f {
                end = new_end;
                duration = end - start;
            } else {
                tracing::warn!("Filtering clip ({duration:.1}s) - too short and cannot extend to minimum {min_duration}s");
                continue;
            }
        }
        if duration > max_f {
            // End on the last sentence that fits (a hard cut only when none ends in range).
            end = start + max_f;
            if snapping {
                if let Some(ms) = last_sentence_end_between(
                    transcript,
                    ((start + min_f) * 1000.0) as i64,
                    (end * 1000.0) as i64,
                ) {
                    end = ms as f64 / 1000.0;
                }
            }
        }
        valid.push((clip, start, end));
    }

    let mut segments = Vec::new();
    for (clip, start_sec, end_sec) in valid {
        let mut start_ms = (start_sec * 1000.0) as i64;
        let mut end_ms = (end_sec * 1000.0) as i64;
        if snapping {
            let max_extension_ms = (settings.sentence_extension_max_seconds * 1000.0) as i64;
            let start_adjust_ms = (settings.start_boundary_max_adjustment_seconds * 1000.0) as i64;
            // 1. Snap START to a word/sentence boundary.
            let original_start_ms = start_ms;
            let adjusted =
                find_sentence_start_boundary(transcript, start_ms, start_adjust_ms).max(0);
            if adjusted != original_start_ms {
                start_ms = adjusted;
            }
            // Snapping back must not leave the selected range: start on the next
            // sentence (or at least word) inside it, keeping the raw time only if
            // the clip would get too short.
            if let Some(limit) = start_limit {
                if (start_ms as f64) < limit * 1000.0 {
                    let limit_ms = (limit * 1000.0) as i64;
                    start_ms = original_start_ms;
                    for within_ms in [start_adjust_ms, 0] {
                        if let Some(moved) = next_start_at_or_after(transcript, limit_ms, within_ms)
                        {
                            if end_ms - moved >= min_duration * 1000 {
                                start_ms = moved;
                                break;
                            }
                        }
                    }
                }
            }
            // 2. Snap END to a sentence boundary, never past max duration or the range.
            let original_end_ms = end_ms;
            let mut adjusted_end = sentence_end_forward(transcript, end_ms, max_extension_ms);
            let mut max_end_ms = start_ms + max_duration * 1000;
            if let Some(limit) = end_limit {
                max_end_ms = max_end_ms.min((limit * 1000.0) as i64);
            }
            if adjusted_end > max_end_ms {
                let fitting = last_sentence_end_between(
                    transcript,
                    start_ms + min_duration * 1000,
                    max_end_ms,
                );
                adjusted_end = fitting.unwrap_or(original_end_ms.min(max_end_ms));
            }
            if adjusted_end != original_end_ms {
                end_ms = adjusted_end;
            }
        }

        // Snapping can move either edge; find a complete sentence within the
        // allowed window before accepting a clip that has become too short.
        let minimum_end_ms = start_ms + min_duration * 1000;
        let mut maximum_end_ms = start_ms + max_duration * 1000;
        if let Some(limit) = end_limit {
            maximum_end_ms = maximum_end_ms.min((limit * 1000.0) as i64);
        }
        if end_ms < minimum_end_ms && snapping && minimum_end_ms <= maximum_end_ms {
            let sentence_end = find_sentence_end_boundary(
                transcript,
                minimum_end_ms,
                maximum_end_ms - minimum_end_ms,
                Direction::Forward,
                0,
            );
            if last_sentence_end_between(transcript, minimum_end_ms, sentence_end)
                == Some(sentence_end)
            {
                end_ms = sentence_end;
            }
        }
        if end_ms < minimum_end_ms || end_ms > maximum_end_ms {
            tracing::warn!("Filtering clip after boundary snapping: duration falls outside {min_duration}-{max_duration}s");
            continue;
        }

        let str_list = |key: &str| -> Vec<String> {
            match clip.get(key) {
                Some(Value::Array(items)) => items
                    .iter()
                    .filter_map(|w| w.as_str().map(str::to_string))
                    .collect(),
                _ => vec![],
            }
        };
        let mut segment = ClipPlanSegment {
            start_time_ms: start_ms,
            end_time_ms: end_ms,
            virality_score: score_clip(clip),
            layout_type: "center_crop".into(),
            summary: clip
                .get("summary")
                .and_then(|s| s.as_str())
                .map(str::to_string),
            tags: str_list("tags"),
            emphasis_words: if transcript.is_empty() {
                vec![]
            } else {
                str_list("emphasis").into_iter().take(5).collect()
            },
            ..Default::default()
        };
        if ctx.longform {
            segment.skip_ranges_ms = clean_skips(
                clip.get("skip"),
                start_ms,
                end_ms,
                min_duration * 1000,
                transcript,
            );
            segment.chapters = clean_chapters(
                clip.get("chapters"),
                start_ms,
                end_ms,
                &segment.skip_ranges_ms,
                segment.summary.as_deref(),
            );
            if let Some(description) = clip.get("description").and_then(|d| d.as_str()) {
                let trimmed = py_strip(description);
                if !trimmed.is_empty() {
                    segment.description = Some(trimmed.chars().take(1500).collect());
                }
            }
        }
        segments.push(segment);
    }
    tracing::info!("Parsed {} clips from planner response", segments.len());
    Ok(ParsedPlan { segments, insights })
}

/// The prompt prints line times as floored whole seconds, so a whole-second time
/// means the line `edge` inside that second, when there is one (the first, or with
/// `last` the last, so two short lines in one second stay in the clip). Without this a
/// floored start lands in the previous line's last word (Whisper lines touch) and the
/// start snap keeps that word. Other times are returned unchanged.
fn line_edge(
    transcript: &[TranscriptSegment],
    seconds: f64,
    edge: impl Fn(&TranscriptSegment) -> i64,
    last: bool,
) -> f64 {
    if seconds.fract() != 0.0 || !seconds.is_finite() {
        return seconds;
    }
    let shown = seconds as i64 * 1000;
    let mut inside = transcript
        .iter()
        .map(edge)
        .filter(|ms| (shown..shown + 1000).contains(ms));
    let ms = if last { inside.last() } else { inside.next() };
    ms.map_or(seconds, |ms| ms as f64 / 1000.0)
}

/// Python `str.strip()` (Unicode whitespace).
fn py_strip(s: &str) -> &str {
    s.trim()
}

/// Python `" ".join(s.split())`.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Validated longform skips in source ms: inside the clip, on sentence
/// boundaries, merged, and never cutting the runtime below the minimum.
pub fn clean_skips(
    raw: Option<&Value>,
    start_ms: i64,
    end_ms: i64,
    min_runtime_ms: i64,
    transcript: &[TranscriptSegment],
) -> Vec<(i64, i64)> {
    let Some(Value::Array(items)) = raw else {
        return vec![];
    };
    // A skip must leave some of the clip on both sides of it.
    let (inner_start, inner_end) = (start_ms + 5000, end_ms - 5000);
    let mut skips = Vec::new();
    for item in items {
        let Value::Object(item) = item else { continue };
        let (Some(s), Some(e)) = (
            item.get("start_time").and_then(py_float),
            item.get("end_time").and_then(py_float),
        ) else {
            continue;
        };
        let (s_ms, e_ms) = ((s * 1000.0), (e * 1000.0));
        if !s_ms.is_finite() || !e_ms.is_finite() {
            continue;
        }
        let (mut s_ms, mut e_ms) = (s_ms as i64, e_ms as i64);
        if !transcript.is_empty() {
            // Jump from the end of a sentence to the start of the next one.
            if let Some(snapped) = last_sentence_end_between(transcript, s_ms - 4000, s_ms + 1500) {
                s_ms = snapped;
            }
            if let Some(snapped) = next_start_at_or_after(transcript, e_ms - 1500, 4000) {
                e_ms = snapped;
            }
        }
        let (s_ms, e_ms) = (s_ms.max(inner_start), e_ms.min(inner_end));
        if e_ms - s_ms >= MIN_SKIP_MS {
            skips.push((s_ms, e_ms));
        }
    }
    skips.sort();
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (s, e) in skips {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    // Keep the longest skips that fit the share and runtime budgets.
    let span = end_ms - start_ms;
    let budget = (span as f64 * MAX_SKIP_SHARE).min((span - min_runtime_ms) as f64);
    let mut by_length = merged.clone();
    by_length.sort_by(|a, b| (b.1 - b.0).cmp(&(a.1 - a.0)));
    let mut kept = Vec::new();
    let mut used = 0i64;
    for (s, e) in by_length {
        if ((used + (e - s)) as f64) <= budget {
            kept.push((s, e));
            used += e - s;
        }
    }
    if kept.len() < merged.len() {
        tracing::info!(
            "Dropped {} skip(s) that would cut the episode too short",
            merged.len() - kept.len()
        );
    }
    kept.sort();
    kept
}

/// Chapter starts in source ms: inside the clip, out of skips, spaced out, and
/// with the first at the clip start.
pub fn clean_chapters(
    raw: Option<&Value>,
    start_ms: i64,
    end_ms: i64,
    skips: &[(i64, i64)],
    fallback_title: Option<&str>,
) -> Vec<(i64, String)> {
    let mut chapters: Vec<(i64, String)> = Vec::new();
    if let Some(Value::Array(items)) = raw {
        for item in items {
            let Value::Object(item) = item else { continue };
            let Some(t) = item.get("time").and_then(py_float) else {
                continue;
            };
            let t = t * 1000.0;
            if !t.is_finite() {
                continue;
            }
            let mut t_ms = t as i64;
            let Some(title) = item
                .get("title")
                .and_then(|t| t.as_str())
                .filter(|t| !t.trim().is_empty())
            else {
                continue;
            };
            for (s, e) in skips {
                if *s <= t_ms && t_ms < *e {
                    t_ms = *e;
                }
            }
            if start_ms <= t_ms && t_ms < end_ms - MIN_CHAPTER_GAP_MS {
                chapters.push((t_ms, collapse_whitespace(title).chars().take(80).collect()));
            }
        }
    }
    chapters.sort();
    if chapters.is_empty() {
        return chapters;
    }
    // The first chapter always opens the episode.
    if chapters[0].0 - start_ms < MIN_CHAPTER_GAP_MS {
        chapters[0].0 = start_ms;
    } else {
        let fallback = fallback_title.unwrap_or("").trim();
        let fallback = if fallback.is_empty() {
            "Intro"
        } else {
            fallback
        };
        let mut title: String = fallback.chars().take(80).collect();
        if title.is_empty() {
            title = "Intro".into();
        }
        chapters.insert(0, (start_ms, title));
    }
    let mut spaced = vec![chapters[0].clone()];
    for (t, title) in chapters.into_iter().skip(1) {
        if t - spaced.last().unwrap().0 >= MIN_CHAPTER_GAP_MS {
            spaced.push((t, title));
        }
    }
    spaced
}

/// Rank clips best-first, drop heavy overlaps, and cap to `clip_count`.
pub fn finalize_clips(clips: Vec<ClipPlanSegment>, clip_count: i64) -> Vec<ClipPlanSegment> {
    let mut ranked = clips;
    ranked.sort_by(|a, b| {
        b.virality_score
            .partial_cmp(&a.virality_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept: Vec<ClipPlanSegment> = Vec::new();
    for clip in ranked {
        let overlaps = kept.iter().any(|other| {
            clip.end_time_ms.min(other.end_time_ms) - clip.start_time_ms.max(other.start_time_ms)
                > MAX_CLIP_OVERLAP_MS
        });
        if overlaps {
            tracing::info!(
                "Dropping overlapping clip {}-{}ms (score {:.2})",
                clip.start_time_ms,
                clip.end_time_ms,
                clip.virality_score
            );
            continue;
        }
        kept.push(clip);
    }
    kept.truncate(clip_count.max(0) as usize);
    kept
}
