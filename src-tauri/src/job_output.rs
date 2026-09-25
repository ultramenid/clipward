//! Validate engine and persisted pipeline output before it reaches the renderer
//! (port of parseJobOutput in src/shared/job-output.ts). Everything not listed
//! here is dropped; text is bounded.
use serde_json::{Map, Value};

type Obj = Map<String, Value>;

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

fn finite(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}

fn non_negative(value: &Value) -> Option<f64> {
    finite(value).filter(|n| *n >= 0.0)
}

fn safe_integer(value: &Value) -> Option<f64> {
    finite(value).filter(|n| n.fract() == 0.0 && n.abs() <= MAX_SAFE_INTEGER)
}

fn bounded_text(value: &Value, max: usize) -> Option<String> {
    value.as_str().map(|text| text.chars().take(max).collect())
}

fn text_or_null(value: &Value, max: usize) -> Value {
    bounded_text(value, max).map_or(Value::Null, Value::from)
}

fn one_of(value: &Value, allowed: &[&str]) -> bool {
    value.as_str().is_some_and(|text| allowed.contains(&text))
}

fn safe_box(value: &Value) -> Option<Value> {
    let parts = value.as_array()?;
    (parts.len() == 4
        && parts
            .iter()
            .all(|part| finite(part).is_some_and(|n| (0.0..=1.0).contains(&n))))
    .then(|| value.clone())
}

fn safe_shot(value: &Value) -> Option<Value> {
    let shot_in = value.as_object()?;
    let start = non_negative(shot_in.get("start_ms")?)?;
    let end = non_negative(shot_in.get("end_ms")?)?;
    if end < start {
        return None;
    }
    let mut shot = Obj::new();
    shot.insert("start_ms".into(), shot_in["start_ms"].clone());
    shot.insert("end_ms".into(), shot_in["end_ms"].clone());
    for field in ["layout", "source"] {
        if let Some(label) = shot_in.get(field).and_then(|v| bounded_text(v, 64)) {
            shot.insert(field.into(), Value::from(label));
        }
    }
    for field in ["screen_box", "screen_focus", "cam_box"] {
        if let Some(bbox) = shot_in.get(field).and_then(safe_box) {
            shot.insert(field.into(), bbox);
        }
    }
    if let Some(people) = shot_in.get("people").and_then(Value::as_array) {
        shot.insert(
            "people".into(),
            Value::Array(people.iter().take(8).filter_map(safe_box).collect()),
        );
    }
    Some(Value::Object(shot))
}

fn copy_non_negative(from: &Obj, to: &mut Obj, fields: &[&str]) {
    for field in fields {
        if from.get(*field).and_then(non_negative).is_some() {
            to.insert((*field).into(), from[*field].clone());
        }
    }
}

fn safe_metrics(value: &Value) -> Value {
    let Some(metrics) = value.as_object() else {
        return Value::Null;
    };
    let mut result = Obj::new();
    if let Some(flag) = metrics
        .get("smart_framing_available")
        .filter(|v| v.is_boolean())
    {
        result.insert("smart_framing_available".into(), flag.clone());
    }
    let enums: [(&str, &[&str]); 3] = [
        (
            "transcription_status",
            &["available", "no_speech", "failed"],
        ),
        ("planning_source", &["visual", "transcript"]),
        (
            "captions_status",
            &[
                "enabled",
                "disabled_by_request",
                "unavailable_without_transcript",
            ],
        ),
    ];
    for (field, allowed) in enums {
        if let Some(v) = metrics.get(field).filter(|v| one_of(v, allowed)) {
            result.insert(field.into(), v.clone());
        }
    }
    copy_non_negative(metrics, &mut result, &["visual_frame_count"]);
    if let Some(requested) = metrics.get("requested_settings").and_then(Value::as_object) {
        let mut safe = Obj::new();
        for field in ["ai_provider", "planner_model", "transcription_model"] {
            if let Some(model) = requested.get(field).and_then(|v| bounded_text(v, 120)) {
                safe.insert(field.into(), Value::from(model));
            }
        }
        let enums: [(&str, &[&str]); 3] = [
            ("aspect_ratio", &["9:16", "16:9"]),
            ("layout_style", &["auto", "fill", "fit"]),
            ("pacing", &["tight", "natural"]),
        ];
        for (field, allowed) in enums {
            if let Some(v) = requested.get(field).filter(|v| one_of(v, allowed)) {
                safe.insert(field.into(), v.clone());
            }
        }
        if let Some(v) = requested
            .get("layout_vision_enabled")
            .filter(|v| v.is_boolean())
        {
            safe.insert("layout_vision_enabled".into(), v.clone());
        }
        if let Some(v) = requested
            .get("video_speed")
            .filter(|v| finite(v).is_some_and(|n| (1.0..=2.0).contains(&n)))
        {
            safe.insert("video_speed".into(), v.clone());
        }
        result.insert("requested_settings".into(), Value::Object(safe));
    }
    copy_non_negative(
        metrics,
        &mut result,
        &[
            "planned_clip_count",
            "rendered_clip_count",
            "failed_clip_count",
            "uploaded_clip_count",
            "source_video_size_bytes",
            "rendered_output_bytes",
            "peak_rss_mb",
            "analysis_duration_seconds",
        ],
    );
    if let Some(stages) = metrics
        .get("stage_durations_seconds")
        .and_then(Value::as_object)
    {
        let mut safe = Obj::new();
        for (name, duration) in stages.iter().take(20) {
            let valid_name = !name.is_empty()
                && name.len() <= 40
                && name.chars().all(|c| c.is_ascii_lowercase() || c == '_');
            if valid_name && non_negative(duration).is_some() {
                safe.insert(name.clone(), duration.clone());
            }
        }
        result.insert("stage_durations_seconds".into(), Value::Object(safe));
    }
    if let Some(layouts) = metrics.get("clip_layouts").and_then(Value::as_array) {
        let safe_layouts: Vec<Value> = layouts
            .iter()
            .take(1000)
            .filter_map(|entry| {
                let entry = entry.as_object()?;
                safe_integer(entry.get("clip_index")?).filter(|n| *n >= 0.0)?;
                let mut safe = Obj::new();
                safe.insert("clip_index".into(), entry["clip_index"].clone());
                if let Some(layout) = entry.get("layout_type").and_then(|v| bounded_text(v, 64)) {
                    safe.insert("layout_type".into(), Value::from(layout));
                }
                if let Some(status) = entry
                    .get("framing_status")
                    .filter(|v| one_of(v, &["smart", "classic", "whole_frame_auto", "fallback"]))
                {
                    safe.insert("framing_status".into(), status.clone());
                }
                copy_non_negative(entry, &mut safe, &["pacing_removed_ms"]);
                if let Some(fallback) = entry
                    .get("render_fallback")
                    .and_then(|v| bounded_text(v, 64))
                {
                    safe.insert("render_fallback".into(), Value::from(fallback));
                }
                if let Some(shots) = entry.get("shots").and_then(Value::as_array) {
                    safe.insert("shot_count".into(), Value::from(shots.len()));
                    safe.insert(
                        "shots".into(),
                        Value::Array(shots.iter().take(256).filter_map(safe_shot).collect()),
                    );
                }
                Some(Value::Object(safe))
            })
            .collect();
        result.insert("clip_layouts".into(), Value::Array(safe_layouts));
    }
    if let Some(costs) = metrics.get("api_costs").and_then(Value::as_object) {
        if costs
            .get("total_estimated_cost_usd")
            .and_then(non_negative)
            .is_some()
        {
            let mut safe_costs = Obj::new();
            safe_costs.insert(
                "total_estimated_cost_usd".into(),
                costs["total_estimated_cost_usd"].clone(),
            );
            if let Some(v) = costs.get("cost_incomplete").filter(|v| v.is_boolean()) {
                safe_costs.insert("cost_incomplete".into(), v.clone());
            }
            for name in ["transcription", "planning", "layout_vision"] {
                let Some(section) = costs.get(name).and_then(Value::as_object) else {
                    continue;
                };
                let mut safe = Obj::new();
                if let Some(v) = section.get("cost_incomplete").filter(|v| v.is_boolean()) {
                    safe.insert("cost_incomplete".into(), v.clone());
                }
                for field in ["provider", "model"] {
                    if let Some(text) = section.get(field).and_then(|v| bounded_text(v, 120)) {
                        safe.insert(field.into(), Value::from(text));
                    }
                }
                copy_non_negative(
                    section,
                    &mut safe,
                    &[
                        "estimated_cost_usd",
                        "audio_duration_seconds",
                        "prompt_tokens",
                        "completion_tokens",
                        "total_tokens",
                        "attempts",
                    ],
                );
                safe_costs.insert(name.into(), Value::Object(safe));
            }
            result.insert("api_costs".into(), Value::Object(safe_costs));
        }
    }
    Value::Object(result)
}

const LAYOUT_TYPES: &[&str] = &[
    "talking_head",
    "two_shot",
    "screen_cam",
    "screen",
    "fit",
    "center_crop",
];

fn parse_clip(item: &Value, indices: &mut std::collections::HashSet<i64>) -> Option<Value> {
    let item = item.as_object()?;
    let index = safe_integer(item.get("clip_index")?).filter(|n| *n >= 0.0)? as i64;
    let url = item.get("s3_url")?.as_str()?;
    if url.trim().is_empty() || url.encode_utf16().count() > 8192 {
        return None;
    }
    let duration = finite(item.get("duration_ms")?)?;
    let start = finite(item.get("start_time_ms")?)?;
    let end = finite(item.get("end_time_ms")?)?;
    finite(item.get("virality_score")?)?;
    if duration < 0.0 || start < 0.0 || end < start || !indices.insert(index) {
        return None;
    }
    let mut clip = Obj::new();
    clip.insert("clip_index".into(), item["clip_index"].clone());
    clip.insert("s3_url".into(), Value::from(url));
    for field in [
        "duration_ms",
        "start_time_ms",
        "end_time_ms",
        "virality_score",
    ] {
        clip.insert(field.into(), item[field].clone());
    }
    let layout = item
        .get("layout_type")
        .filter(|v| one_of(v, LAYOUT_TYPES))
        .cloned()
        .unwrap_or(Value::from(""));
    clip.insert("layout_type".into(), layout);
    clip.insert(
        "summary".into(),
        item.get("summary")
            .map_or(Value::Null, |v| text_or_null(v, 2048)),
    );
    let tags: Vec<Value> = item
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .take(50)
                .map(|tag| Value::from(tag.chars().take(64).collect::<String>()))
                .collect()
        })
        .unwrap_or_default();
    clip.insert("tags".into(), Value::Array(tags));
    clip.insert(
        "render_fallback".into(),
        item.get("render_fallback")
            .map_or(Value::Null, |v| text_or_null(v, 256)),
    );
    Some(Value::Object(clip))
}

pub fn parse_job_output(value: &Value) -> Option<Value> {
    let input = value.as_object()?;
    let raw_clips = input.get("clips")?.as_array()?;
    if raw_clips.len() > 1000 {
        return None;
    }
    let mut indices = std::collections::HashSet::new();
    let mut clips = Vec::with_capacity(raw_clips.len());
    for item in raw_clips {
        clips.push(parse_clip(item, &mut indices)?);
    }
    let get = |name: &str| input.get(name).unwrap_or(&Value::Null);
    let mut output = Obj::new();
    output.insert(
        "job_id".into(),
        Value::from(bounded_text(get("job_id"), 128).unwrap_or_default()),
    );
    output.insert(
        "source_video_url".into(),
        Value::from(bounded_text(get("source_video_url"), 8192).unwrap_or_default()),
    );
    output.insert(
        "source_video_title".into(),
        Value::from(
            bounded_text(get("source_video_title"), 1024)
                .unwrap_or_else(|| "Untitled video".into()),
        ),
    );
    output.insert(
        "source_video_duration_seconds".into(),
        finite(get("source_video_duration_seconds")).map_or(Value::from(0), |_| {
            get("source_video_duration_seconds").clone()
        }),
    );
    output.insert("total_clips".into(), Value::from(clips.len()));
    output.insert("clips".into(), Value::Array(clips));
    output.insert(
        "transcript_url".into(),
        text_or_null(get("transcript_url"), 8192),
    );
    output.insert("plan_url".into(), text_or_null(get("plan_url"), 8192));
    output.insert(
        "processing_time_seconds".into(),
        finite(get("processing_time_seconds"))
            .map_or(Value::from(0), |_| get("processing_time_seconds").clone()),
    );
    output.insert("metrics".into(), safe_metrics(get("metrics")));
    if let Some(created) = get("created_at").as_str() {
        output.insert(
            "created_at".into(),
            Value::from(created.chars().take(64).collect::<String>()),
        );
    }
    if let Some(user) = get("user_id").as_str() {
        output.insert(
            "user_id".into(),
            Value::from(user.chars().take(128).collect::<String>()),
        );
    }
    Some(Value::Object(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_valid_output_and_drops_extras() {
        let output = parse_job_output(&json!({
            "job_id": "j", "source_video_title": "T", "secret": "x",
            "clips": [{ "clip_index": 0, "s3_url": "/a.mp4", "duration_ms": 1000, "start_time_ms": 0,
                        "end_time_ms": 1000, "virality_score": 7.5, "layout_type": "weird", "tags": ["a", 3] }],
            "metrics": { "api_costs": { "total_estimated_cost_usd": 0.1, "planning": { "model": "m", "raw": "x" } }, "junk": 1 }
        }))
        .unwrap();
        assert!(output.get("secret").is_none());
        assert_eq!(output["clips"][0]["layout_type"], "");
        assert_eq!(output["clips"][0]["tags"], json!(["a"]));
        assert_eq!(
            output["metrics"]["api_costs"]["planning"],
            json!({ "model": "m" })
        );
        assert!(output["metrics"].get("junk").is_none());
        assert_eq!(output["total_clips"], 1);
    }

    #[test]
    fn rejects_invalid_clips() {
        assert!(parse_job_output(
            &json!({ "clips": [{ "clip_index": 0, "s3_url": "", "duration_ms": 1,
            "start_time_ms": 0, "end_time_ms": 1, "virality_score": 1 }] })
        )
        .is_none());
        assert!(parse_job_output(&json!({ "clips": "nope" })).is_none());
    }
}
