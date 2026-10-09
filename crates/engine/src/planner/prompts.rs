//! Planner prompts and output schemas, byte-identical to `intelligence_planner.py`.
//!
//! The `prompts/*.txt` templates are the Python f-string bodies extracted verbatim
//! (`{{`/`}}` escapes kept); [`fill`] substitutes `{name}` fields the same way.
use super::config::{resolve_clip_duration_bounds, DURATION_RANGES};
use crate::types::TranscriptSegment;
use serde_json::{json, Value};

const SYSTEM: &str = include_str!("prompts/system.txt");
const DURATION_GUIDANCE: &str = include_str!("prompts/duration_guidance.txt");
const LONGFORM: &str = include_str!("prompts/longform.txt");
const VISUAL_ONLY: &str = include_str!("prompts/visual_only.txt");
const CAMPAIGN_BRIEF: &str = include_str!("prompts/campaign_brief.txt");

/// Python f-string substitution: `{name}` from `fields`, `{{`/`}}` to braces.
fn fill(template: &str, fields: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len() + 256);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
        } else if tail.starts_with('{') {
            let end = tail.find('}').expect("unclosed template field");
            let name = &tail[1..end];
            let value = fields
                .iter()
                .find(|(n, _)| *n == name)
                .unwrap_or_else(|| panic!("missing template field {name}"));
            out.push_str(&value.1);
            rest = &tail[end + 1..];
        } else {
            panic!("stray '}}' in template");
        }
    }
    out.push_str(rest);
    out
}

/// Prompt section for a clipping campaign's rules (content rewards briefs).
pub fn campaign_brief_section(requirements: &str) -> String {
    fill(
        CAMPAIGN_BRIEF,
        &[("requirements", requirements.trim().to_string())],
    )
}

/// The count line of the output format: a fixed count, or (`None`, "Let AI decide")
/// no number at all, so the model returns what the video holds instead of filling a quota.
fn count_instruction(clip_count: Option<i64>) -> String {
    match clip_count {
        Some(n) => format!("Return exactly {n} clips as JSON:"),
        None => "Return every moment a viewer would enjoy on its own as a separate clip. When \
unsure, include it: clips are ranked by score, so weaker ones sort to the bottom. Return them as JSON:"
            .into(),
    }
}

/// `clip_count` None means the user let the AI decide how many clips to cut.
pub fn build_system_prompt(
    clip_count: Option<i64>,
    min_duration: Option<i64>,
    max_duration: Option<i64>,
    duration_ranges: Option<&[String]>,
) -> String {
    let (min_duration, max_duration) =
        resolve_clip_duration_bounds(duration_ranges, min_duration, max_duration);
    let strict_bounds_text = format!("STRICTLY between {min_duration} and {max_duration} seconds");
    let selected: Vec<&str> = duration_ranges
        .unwrap_or(&[])
        .iter()
        .filter_map(|r| {
            DURATION_RANGES
                .iter()
                .find(|(id, ..)| id == r)
                .map(|(_, _, _, desc)| *desc)
        })
        .collect();
    let duration_guidance = if selected.is_empty() {
        String::new()
    } else {
        let ranges_list = selected
            .iter()
            .map(|r| format!("- {r}"))
            .collect::<Vec<_>>()
            .join("\n");
        fill(
            DURATION_GUIDANCE,
            &[
                ("ranges_list", ranges_list),
                ("strict_bounds_text", strict_bounds_text.clone()),
                ("min_duration", min_duration.to_string()),
                ("max_duration", max_duration.to_string()),
            ],
        )
    };
    fill(
        SYSTEM,
        &[
            ("count_instruction", count_instruction(clip_count)),
            ("duration_guidance", duration_guidance),
            ("strict_bounds_text", strict_bounds_text),
            ("min_duration", min_duration.to_string()),
            ("max_duration", max_duration.to_string()),
        ],
    )
}

/// System prompt for 16:9 longform edits (5+ minute YouTube-style episodes).
pub fn build_longform_system_prompt(
    clip_count: i64,
    min_duration: i64,
    max_duration: i64,
) -> String {
    fill(
        LONGFORM,
        &[
            ("clip_count", clip_count.to_string()),
            ("min_minutes", min_duration.div_euclid(60).to_string()),
            ("max_minutes", max_duration.div_euclid(60).to_string()),
            ("min_duration", min_duration.to_string()),
            ("max_duration", max_duration.to_string()),
        ],
    )
}

/// Ask for clips only when sampled video frames show a clear visual event.
pub fn build_visual_only_system_prompt(
    clip_count: i64,
    min_duration: Option<i64>,
    max_duration: Option<i64>,
    duration_ranges: Option<&[String]>,
) -> String {
    let (minimum, maximum) =
        resolve_clip_duration_bounds(duration_ranges, min_duration, max_duration);
    fill(
        VISUAL_ONLY,
        &[
            ("clip_count", clip_count.to_string()),
            ("minimum", minimum.to_string()),
            ("maximum", maximum.to_string()),
        ],
    )
}

/// `[start - end] (speaker) text (events)` lines.
pub fn build_transcript_text(transcript: &[TranscriptSegment]) -> String {
    if transcript.is_empty() {
        return "[No transcript available]".into();
    }
    transcript
        .iter()
        .map(|seg| {
            let time = format!(
                "[{:.1} - {:.1}]",
                seg.start_time_ms as f64 / 1000.0,
                seg.end_time_ms as f64 / 1000.0
            );
            let speaker = seg
                .speaker_label
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| format!("({s}) "))
                .unwrap_or_default();
            let events = if seg.audio_events.is_empty() {
                String::new()
            } else {
                format!(" {}", seg.audio_events.join(" "))
            };
            format!("{time} {speaker}{}{events}", seg.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One loaded frame: timestamp and base64 image data.
#[derive(Debug, Clone)]
pub struct FrameImage {
    pub timestamp_ms: i64,
    pub mime: String,
    pub base64: String,
}

/// A user message part (mirrors the OpenRouter content list).
#[derive(Debug, Clone, PartialEq)]
pub enum UserPart {
    Text(String),
    /// Index into the frame images.
    Image(usize),
}

/// The planner's user message: transcript (or visual note), frames, final instruction.
/// `fixed_count` false ("Let AI decide") leaves the number out of the ask.
#[allow(clippy::too_many_arguments)]
pub fn build_user_parts(
    transcript_text: &str,
    frames: &[FrameImage],
    clip_count: i64,
    fixed_count: bool,
    transcript: &[TranscriptSegment],
    video_duration_seconds: f64,
    longform: bool,
) -> Vec<UserPart> {
    let mut parts = Vec::new();
    let video_duration = if video_duration_seconds != 0.0 {
        video_duration_seconds
    } else {
        transcript
            .last()
            .map(|s| s.end_time_ms as f64 / 1000.0)
            .unwrap_or(0.0)
    };
    let frames_note = if frames.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nBelow are {} sample frames from the video at regular intervals. Use these to understand the visual content and identify compelling moments.",
            frames.len()
        )
    };
    let source_description = if transcript.is_empty() {
        "No usable speech transcript is available. Judge only the visible sample frames."
            .to_string()
    } else {
        format!("Here is the transcript of the video:\n\n{transcript_text}")
    };
    parts.push(UserPart::Text(format!(
        "{source_description}\n\nThe video is approximately {video_duration:.0} seconds long.{frames_note}"
    )));
    for (i, frame) in frames.iter().enumerate() {
        parts.push(UserPart::Text(format!(
            "Frame at {:.1} seconds:",
            frame.timestamp_ms as f64 / 1000.0
        )));
        parts.push(UserPart::Image(i));
    }
    let basis = if frames.is_empty() {
        "transcript"
    } else {
        "transcript and frames"
    };
    let last = if longform {
        format!(
            "\nBased on the {basis} above, identify up to {clip_count} complete, self-contained longform episodes. Return fewer rather than pad with weak material. Return your response as JSON."
        )
    } else if !transcript.is_empty() && !fixed_count {
        format!("\nBased on the {basis} above, identify every moment a viewer would enjoy on its own as a clip. When unsure, include it. Return your response as JSON.")
    } else if !transcript.is_empty() {
        format!("\nBased on the {basis} above, identify the {clip_count} most viral-worthy segments. Return your response as JSON.")
    } else {
        format!(
            "\nSelect up to {clip_count} visually compelling segments supported by these frames. Return an empty clips array if none qualify. Return your response as JSON."
        )
    };
    parts.push(UserPart::Text(last));
    parts
}

/// The five rubric dimensions the model scores each clip on (0-10 each).
pub const RUBRIC_DIMENSIONS: [&str; 5] = ["hook", "standalone", "arc", "quotability", "ending"];

/// Planner output schema; longform clips also carry skips, chapters and a description.
pub fn clip_plan_schema(longform: bool) -> Value {
    let mut score_props = serde_json::Map::new();
    for dim in RUBRIC_DIMENSIONS {
        score_props.insert(dim.into(), json!({"type": "number", "description": "0-10"}));
    }
    let mut item = json!({
        "type": "object",
        "properties": {
            "start_time": {"type": "number", "description": "Clip start, seconds from the start of the video."},
            "end_time": {"type": "number", "description": "Clip end, seconds from the start of the video."},
            "summary": {"type": "string", "description": "2-7 word on-screen title."},
            "scores": {
                "type": "object",
                "properties": score_props,
                "required": RUBRIC_DIMENSIONS,
                "additionalProperties": false,
            },
            "tags": {"type": "array", "items": {"type": "string"}},
            "emphasis": {
                "type": "array",
                "items": {"type": "string"},
                "description": "2-5 single punch words spoken in the clip, highlighted in captions.",
            },
        },
        "required": ["start_time", "end_time", "summary", "scores", "tags", "emphasis"],
        "additionalProperties": false,
    });
    if longform {
        let time_range = json!({
            "type": "object",
            "properties": {
                "start_time": {"type": "number", "description": "Seconds from the start of the video."},
                "end_time": {"type": "number", "description": "Seconds from the start of the video."},
            },
            "required": ["start_time", "end_time"],
            "additionalProperties": false,
        });
        let chapter = json!({
            "type": "object",
            "properties": {
                "time": {"type": "number", "description": "Chapter start, seconds from the start of the video."},
                "title": {"type": "string", "description": "2-6 word chapter title."},
            },
            "required": ["time", "title"],
            "additionalProperties": false,
        });
        let props = item["properties"].as_object_mut().unwrap();
        props.insert(
            "skip".into(),
            json!({"type": "array", "items": time_range, "description": "Tangents or dead stretches inside the clip to cut out (may be empty)."}),
        );
        props.insert(
            "chapters".into(),
            json!({"type": "array", "items": chapter, "description": "Chapter starts inside the clip, the first at the clip start."}),
        );
        props.insert(
            "description".into(),
            json!({"type": "string", "description": "2-4 sentence upload description."}),
        );
        let required = item["required"].as_array_mut().unwrap();
        for key in ["skip", "chapters", "description"] {
            required.push(json!(key));
        }
    }
    json!({
        "type": "object",
        "properties": {
            "insights": {
                "type": "string",
                "description": "Content type classification plus a brief analysis of the video's themes and why these clips were chosen.",
            },
            "clips": {"type": "array", "items": item},
        },
        "required": ["insights", "clips"],
        "additionalProperties": false,
    })
}
