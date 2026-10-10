//! Planner tests: ports of engine/tests/test_planner.py, test_plan_boundaries.py,
//! test_visual_fallback.py and the planner parts of test_longform.py, plus golden
//! fixtures generated from the Python planner (prompts, clip counts, 700 parse cases).
use super::parse::{
    clean_chapters, clean_skips, finalize_clips, parse_clip_plan_response, score_clip, ParseContext,
};
use super::*;
use crate::llm::test_server::{serve, Reply};
use crate::types::TranscriptWord;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ---------------------------------------------------------------- helpers

fn word(w: &str, s: i64, e: i64) -> TranscriptWord {
    TranscriptWord {
        word: w.into(),
        start_time_ms: s,
        end_time_ms: e,
    }
}

/// test_plan_boundaries / test_longform make_transcript: sentences of 12 words, odd timings.
fn make_transcript(n_sentences: usize) -> Vec<TranscriptSegment> {
    let (words_per, word_ms, gap_ms, pause_ms) = (12, 300, 80, 400);
    let mut segments = Vec::new();
    let mut t = 1000;
    for s in 0..n_sentences {
        let mut words = Vec::new();
        for w in 0..words_per {
            let text = format!("w{s}_{w}{}", if w == words_per - 1 { "." } else { "" });
            words.push(word(&text, t, t + word_ms));
            t += word_ms + gap_ms;
        }
        t += pause_ms;
        segments.push(TranscriptSegment {
            start_time_ms: words[0].start_time_ms,
            end_time_ms: words.last().unwrap().end_time_ms,
            text: words
                .iter()
                .map(|w| w.word.clone())
                .collect::<Vec<_>>()
                .join(" "),
            speaker_label: Some("S1".into()),
            words,
            audio_events: vec![],
        });
    }
    segments
}

/// test_planner make_transcript: 5 s lines "wordN end.", alternating speakers.
fn planner_transcript(seconds: i64) -> Vec<TranscriptSegment> {
    (0..seconds)
        .step_by(5)
        .map(|start| TranscriptSegment {
            start_time_ms: start * 1000,
            end_time_ms: start * 1000 + 4500,
            text: format!("word{start} end."),
            speaker_label: Some(if (start / 5) % 2 == 0 { "S1" } else { "S2" }.into()),
            words: vec![
                word(&format!("word{start}"), start * 1000, start * 1000 + 2000),
                word("end.", start * 1000 + 2000, start * 1000 + 4500),
            ],
            audio_events: if start == 60 {
                vec!["(laughter)".into()]
            } else {
                vec![]
            },
        })
        .collect()
}

fn shown(ms: i64) -> f64 {
    (ms as f64 / 100.0).round() / 10.0
}

fn clip(start: f64, end: f64, scores: [f64; 5]) -> Value {
    json!({
        "start_time": start, "end_time": end, "summary": "Great Title Here",
        "scores": {"hook": scores[0], "standalone": scores[1], "arc": scores[2], "quotability": scores[3], "ending": scores[4]},
        "tags": ["tag"],
    })
}

fn five(v: f64) -> [f64; 5] {
    [v; 5]
}

fn content(clips: Vec<Value>) -> String {
    json!({"insights": "x", "clips": clips}).to_string()
}

fn ctx<'a>(
    transcript: &'a [TranscriptSegment],
    min: Option<i64>,
    max: Option<i64>,
) -> ParseContext<'a> {
    ParseContext {
        min_duration: min,
        max_duration: max,
        transcript,
        ..Default::default()
    }
}

fn parse(ctx: &ParseContext, text: &str) -> Vec<ClipPlanSegment> {
    parse_clip_plan_response(Some(text), Some("stop"), ctx, &PlannerSettings::default())
        .unwrap()
        .segments
}

fn seg(start: i64, end: i64, score: f64) -> ClipPlanSegment {
    ClipPlanSegment {
        start_time_ms: start,
        end_time_ms: end,
        virality_score: score,
        ..Default::default()
    }
}

fn completion(content: &str, model: &str, cost: Option<f64>) -> Value {
    let mut usage = json!({"prompt_tokens": 1000, "completion_tokens": 200, "total_tokens": 1200});
    if let Some(c) = cost {
        usage["cost"] = json!(c);
    }
    json!({"model": model, "choices": [{"message": {"content": content}, "finish_reason": "stop"}], "usage": usage})
}

fn openrouter() -> AiSettings {
    AiSettings {
        provider: "openrouter".into(),
        openrouter_api_key: Some("test".into()),
        ..Default::default()
    }
}

fn test_settings(base: &str) -> PlannerSettings {
    PlannerSettings {
        base_url: Some(base.into()),
        retry_base_delay: Duration::ZERO,
        ..Default::default()
    }
}

fn input(transcript: &[TranscriptSegment]) -> PlanInput<'_> {
    PlanInput {
        transcript,
        video_duration: Some(300.0),
        max_clips: Some(3),
        auto_clip_count: false,
        min_duration_seconds: Some(15),
        max_duration_seconds: Some(60),
        aspect_ratio: "9:16",
        ..Default::default()
    }
}

// ---------------------------------------------------------------- golden fixtures

fn fixture_transcripts() -> HashMap<String, Vec<TranscriptSegment>> {
    serde_json::from_str(include_str!("fixtures/transcripts.json")).unwrap()
}

fn opt_i64(v: &Value) -> Option<i64> {
    v.as_i64()
}

fn ranges_of(v: &Value) -> Option<Vec<String>> {
    v.as_array()
        .map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect())
}

#[test]
fn prompts_match_python_byte_for_byte() {
    let transcripts = fixture_transcripts();
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/prompts.json")).unwrap();
    for case in &cases {
        let count = case["clip_count"].as_i64().unwrap_or(0);
        let ranges = ranges_of(&case["ranges"]);
        let (min, max) = (opt_i64(&case["min"]), opt_i64(&case["max"]));
        match case["kind"].as_str().unwrap() {
            "system" => assert_eq!(
                prompts::build_system_prompt(Some(count), min, max, ranges.as_deref()),
                case["text"],
                "{case}"
            ),
            "visual" => {
                assert_eq!(
                    prompts::build_visual_only_system_prompt(count, min, max, ranges.as_deref()),
                    case["text"]
                )
            }
            "longform" => {
                assert_eq!(
                    prompts::build_longform_system_prompt(count, min.unwrap(), max.unwrap()),
                    case["text"]
                )
            }
            "brief" => assert_eq!(
                prompts::campaign_brief_section(case["requirements"].as_str().unwrap()),
                case["text"]
            ),
            "transcript" => {
                let tr = &transcripts[case["transcript"].as_str().unwrap()];
                assert_eq!(prompts::build_transcript_text(tr), case["text"]);
            }
            "user" => {
                let tr = &transcripts[case["transcript"].as_str().unwrap()];
                let frames: Vec<prompts::FrameImage> = case["frames"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| prompts::FrameImage {
                        timestamp_ms: t.as_i64().unwrap(),
                        mime: "image/jpeg".into(),
                        base64: "QUJD".into(),
                    })
                    .collect();
                let text = prompts::build_transcript_text(tr);
                let parts = prompts::build_user_parts(
                    &text,
                    &frames,
                    count,
                    true,
                    tr,
                    case["duration"].as_f64().unwrap(),
                    case["longform"].as_bool().unwrap(),
                );
                let messages = PlannerMessages {
                    system: "SYS".into(),
                    user: parts,
                    frames,
                };
                let request = build_request(&PlannerSettings::default(), &messages, false);
                let payload = llm::openrouter_payload(&request);
                let sent: Vec<Value> = payload["messages"][1]["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| {
                        if p["type"] == "text" {
                            json!({"text": p["text"]})
                        } else {
                            json!({"image": p["image_url"]["url"], "detail": p["image_url"]["detail"]})
                        }
                    })
                    .collect();
                assert_eq!(Value::Array(sent), case["parts"], "{case}");
                assert_eq!(
                    payload["messages"][0],
                    json!({"role": "system", "content": "SYS"})
                );
            }
            "schema" => assert_eq!(
                prompts::clip_plan_schema(case["longform"].as_bool().unwrap()),
                case["schema"]
            ),
            other => panic!("unknown fixture kind {other}"),
        }
    }
    assert!(cases.len() > 60);
}

#[test]
fn clip_counts_match_python() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/clip_counts.json")).unwrap();
    for case in cases {
        let got = calculate_optimal_clip_count(
            case["duration"].as_f64().unwrap(),
            case["user_max"].as_i64(),
            case["auto"].as_bool().unwrap(),
            case["wpm"].as_f64(),
        );
        assert_eq!(got, case["expected"].as_i64().unwrap(), "{case}");
    }
}

#[test]
fn parse_cases_match_python() {
    let transcripts = fixture_transcripts();
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/parse_cases.json")).unwrap();
    let settings = PlannerSettings::default();
    let (mut clips_checked, mut errors_checked) = (0, 0);
    for (i, case) in cases.iter().enumerate() {
        let tr = &transcripts[case["transcript"].as_str().unwrap()];
        let ranges = ranges_of(&case["ranges"]);
        let times: Vec<f64> = case["visual_times"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_f64().unwrap())
            .collect();
        let ctx = ParseContext {
            min_duration: opt_i64(&case["min"]),
            max_duration: opt_i64(&case["max"]),
            duration_ranges: ranges.as_deref(),
            start_limit: case["start_limit"].as_f64(),
            end_limit: case["end_limit"].as_f64(),
            transcript: tr,
            video_duration: case["video_duration"].as_f64(),
            longform: case["longform"].as_bool().unwrap(),
            visual_frame_times: &times,
            drop_outside_range: false,
            // The Python planner printed one-decimal times.
            whole_second_lines: false,
        };
        let result =
            parse_clip_plan_response(case["content"].as_str(), Some("stop"), &ctx, &settings);
        let expected = &case["expected"];
        if expected.get("error").is_some() {
            let err = result.expect_err(&format!("case {i} should fail"));
            assert!(err.retryable);
            errors_checked += 1;
            continue;
        }
        let plan = result.unwrap_or_else(|e| panic!("case {i}: {e:?}"));
        let want = expected["segments"].as_array().unwrap();
        assert_eq!(plan.segments.len(), want.len(), "case {i}: {case}");
        for (got, want) in plan.segments.iter().zip(want) {
            assert_eq!(
                got.start_time_ms,
                want["start"].as_i64().unwrap(),
                "case {i} start"
            );
            assert_eq!(
                got.end_time_ms,
                want["end"].as_i64().unwrap(),
                "case {i} end"
            );
            assert!(
                (got.virality_score - want["score"].as_f64().unwrap()).abs() < 1e-9,
                "case {i} score"
            );
            assert_eq!(
                got.summary.as_deref(),
                want["summary"].as_str(),
                "case {i} summary"
            );
            assert_eq!(json!(got.tags), want["tags"], "case {i} tags");
            assert_eq!(
                json!(got.emphasis_words),
                want["emphasis"],
                "case {i} emphasis"
            );
            assert_eq!(json!(got.skip_ranges_ms), want["skips"], "case {i} skips");
            assert_eq!(json!(got.chapters), want["chapters"], "case {i} chapters");
            assert_eq!(
                got.description.as_deref(),
                want["description"].as_str(),
                "case {i} description"
            );
            clips_checked += 1;
        }
        assert_eq!(plan.insights.as_deref(), expected["insights"].as_str());
    }
    assert!(
        clips_checked > 500 && errors_checked > 10,
        "{clips_checked} {errors_checked}"
    );
}

// ---------------------------------------------------------------- test_planner.py

#[test]
fn advanced_uses_selected_model_limit_and_provider_reasoning_defaults() {
    let settings = PlannerSettings {
        clipping_mode: ClippingMode::Advanced,
        planner_model: "custom/model".into(),
        planner_max_output_tokens: 8192,
        ..Default::default()
    };
    let messages = PlannerMessages {
        system: "s".into(),
        user: vec![],
        frames: vec![],
    };
    let payload = llm::openrouter_payload(&build_request(&settings, &messages, false));
    assert_eq!(payload["model"], "custom/model");
    assert_eq!(payload["max_tokens"], 8192);
    assert!(payload.get("models").is_none());
    assert!(payload.get("reasoning").is_none());
    assert!(payload.get("temperature").is_none());
    assert_eq!(payload["response_format"]["type"], "json_schema");
}

#[test]
fn reasoning_payload_uses_schema_fallbacks_and_no_temperature() {
    let settings = PlannerSettings {
        planner_model: "primary/model".into(),
        planner_fallback_models: "fallback/one, primary/model ,fallback/two".into(),
        planner_reasoning_effort: "high".into(),
        ..Default::default()
    };
    assert_eq!(
        settings.planner_fallback_models(),
        ["fallback/one", "fallback/two"]
    );
    let messages = PlannerMessages {
        system: "s".into(),
        user: vec![],
        frames: vec![],
    };
    let payload = llm::openrouter_payload(&build_request(&settings, &messages, false));
    assert_eq!(payload["models"], json!(["fallback/one", "fallback/two"]));
    assert_eq!(
        payload["reasoning"],
        json!({"effort": "high", "exclude": true})
    );
    assert!(payload.get("temperature").is_none());
    assert_eq!(
        payload["response_format"]["json_schema"]["schema"],
        prompts::clip_plan_schema(false)
    );
    assert_eq!(payload["response_format"]["json_schema"]["strict"], true);
    assert_eq!(payload["provider"], json!({"require_parameters": true}));
    assert_eq!(payload["plugins"], json!([{"id": "response-healing"}]));
}

#[test]
fn reasoning_none_sends_temperature_instead() {
    let settings = PlannerSettings {
        planner_reasoning_effort: "none".into(),
        planner_fallback_models: String::new(),
        ..Default::default()
    };
    let messages = PlannerMessages {
        system: "s".into(),
        user: vec![],
        frames: vec![],
    };
    let payload = llm::openrouter_payload(&build_request(&settings, &messages, false));
    assert!(payload.get("reasoning").is_none());
    assert_eq!(payload["temperature"], 0.2);
    assert!(payload.get("models").is_none());
}

#[test]
fn schema_is_strict_compatible() {
    fn walk(node: &Value) {
        if node["type"] == "object" {
            assert_eq!(node["additionalProperties"], false);
            let mut required: Vec<&str> = node["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            let mut props: Vec<&str> = node["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(|k| k.as_str())
                .collect();
            required.sort();
            props.sort();
            assert_eq!(required, props);
            for child in node["properties"].as_object().unwrap().values() {
                walk(child);
            }
        }
        if node["type"] == "array" {
            walk(&node["items"]);
        }
    }
    walk(&prompts::clip_plan_schema(false));
    walk(&prompts::clip_plan_schema(true));
}

#[test]
fn transcript_includes_speakers_and_audio_events() {
    let text = prompts::build_transcript_text(&planner_transcript(70));
    assert!(text.contains("[0 - 4] (S1) word0 end."));
    assert!(text.contains("[60 - 64] (S1) word60 end. (laughter)"));
    // One speaker: no label on every line.
    let text = prompts::build_transcript_text(&make_transcript(2));
    assert_eq!(text.lines().next().unwrap(), "[1 - 5] w0_0 w0_1 w0_2 w0_3 w0_4 w0_5 w0_6 w0_7 w0_8 w0_9 w0_10 w0_11.");
}

/// Whisper-like lines that touch: each starts where the last word before it ends.
/// Every line is at least a second long.
fn touching_lines() -> Vec<TranscriptSegment> {
    let (mut t, mut seed) = (730i64, 7u64);
    (0..120)
        .map(|s| {
            let n = 5 + s % 9;
            let words: Vec<TranscriptWord> = (0..n)
                .map(|w| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    let dur = 200 + (seed >> 33) as i64 % 500;
                    t += dur;
                    word(&format!("t{s}_{w}{}", if w == n - 1 { "." } else { "" }), t - dur, t)
                })
                .collect();
            TranscriptSegment {
                start_time_ms: words[0].start_time_ms,
                end_time_ms: t,
                text: words.iter().map(|w| w.word.clone()).collect::<Vec<_>>().join(" "),
                words,
                ..Default::default()
            }
        })
        .collect()
}

/// The prompt shows whole seconds (floored). A clip the model copies from those lines
/// must get the same edges as one with the lines' exact times, and as one copied from
/// the old one-decimal lines.
#[test]
fn whole_second_lines_snap_to_the_same_clip_edges() {
    let mut transcripts = fixture_transcripts();
    transcripts.remove("empty");
    transcripts.insert("touching".into(), touching_lines());
    let settings = PlannerSettings::default();
    let (mut checked, mut old_misses) = (0, 0);
    for (name, tr) in &transcripts {
        let edges = |start: f64, end: f64, whole_second_lines: bool| -> Vec<(i64, i64)> {
            let c = ParseContext { whole_second_lines, ..ctx(tr, Some(5), Some(600)) };
            let text = content(vec![clip(start, end, five(8.0))]);
            let plan = parse_clip_plan_response(Some(&text), Some("stop"), &c, &settings).unwrap();
            plan.segments.iter().map(|s| (s.start_time_ms, s.end_time_ms)).collect()
        };
        for i in 0..tr.len() {
            for j in [i + 2, i + 7].into_iter().filter(|&j| j < tr.len()) {
                let (s, e) = (tr[i].start_time_ms, tr[j].end_time_ms);
                let exact = edges(s as f64 / 1000.0, e as f64 / 1000.0, false);
                let whole = edges(s.div_euclid(1000) as f64, e.div_euclid(1000) as f64, true);
                let old = edges(shown(s), shown(e), false);
                assert_eq!(whole, exact, "{name} lines {i}-{j}");
                if name != "touching" {
                    // The same, but for the old display's own rounding of an unsnapped edge.
                    let near = |a: &[(i64, i64)], b: &[(i64, i64)]| {
                        a.len() == b.len()
                            && a.iter().zip(b).all(|(x, y)| (x.0 - y.0).abs() <= 50 && (x.1 - y.1).abs() <= 50)
                    };
                    assert!(near(&whole, &old), "{name} lines {i}-{j}: {whole:?} {old:?}");
                } else if whole != old {
                    // One decimal rounded down lands in the word before a touching line.
                    old_misses += 1;
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 1000 && old_misses > 0, "{checked} {old_misses}");
}

#[test]
fn text_only_messages_do_not_mention_frames() {
    let segs = planner_transcript(30);
    let parts = prompts::build_user_parts("transcript", &[], 3, true, &segs, 0.0, false);
    let text: String = parts
        .iter()
        .map(|p| match p {
            prompts::UserPart::Text(t) => t.clone(),
            prompts::UserPart::Image(_) => panic!("no images"),
        })
        .collect::<Vec<_>>()
        .join(" ");
    assert!(!text.to_lowercase().contains("frame"));
}

#[test]
fn scoring() {
    let as_map = |v: Value| v.as_object().unwrap().clone();
    assert_eq!(
        score_clip(&as_map(clip(0.0, 30.0, [10.0, 8.0, 6.0, 4.0, 2.0]))),
        0.6
    );
    assert_eq!(score_clip(&as_map(clip(0.0, 30.0, five(15.0)))), 1.0);
    assert_eq!(score_clip(&as_map(json!({"virality_score": 0.7}))), 0.7);
    assert_eq!(score_clip(&as_map(json!({}))), 0.5);
}

#[test]
fn finalize_drops_overlaps_keeps_best_and_caps() {
    let clips = vec![
        seg(0, 30000, 0.6),
        seg(10000, 40000, 0.9),
        seg(60000, 90000, 0.7),
        seg(100000, 130000, 0.5),
    ];
    let kept = finalize_clips(clips, 2);
    assert_eq!(
        kept.iter().map(|c| c.virality_score).collect::<Vec<_>>(),
        [0.9, 0.7]
    );
    assert_eq!(
        finalize_clips(vec![seg(0, 30000, 0.8), seg(26000, 56000, 0.7)], 5).len(),
        2
    );
}

#[test]
fn empty_and_malformed_content_is_retryable() {
    let settings = PlannerSettings::default();
    let c = ctx(&[], None, None);
    let err = parse_clip_plan_response(None, Some("length"), &c, &settings).unwrap_err();
    assert!(err.retryable && err.message.contains("finish_reason=length"));
    let err =
        parse_clip_plan_response(Some("not json at all"), Some("stop"), &c, &settings).unwrap_err();
    assert!(err.retryable);
}

async fn plan_with_server(
    settings: PlannerSettings,
    replies: Vec<Reply>,
) -> (Result<PlanResponse>, crate::llm::test_server::Server) {
    let server = serve(replies).await;
    let settings = PlannerSettings {
        base_url: Some(server.url.clone()),
        retry_base_delay: Duration::ZERO,
        ..settings
    };
    let tr = planner_transcript(300);
    let result = plan(
        &settings,
        &openrouter(),
        &input(&tr),
        &CancellationToken::new(),
    )
    .await;
    (result, server)
}

fn advanced_priced() -> PlannerSettings {
    PlannerSettings {
        clipping_mode: ClippingMode::Advanced,
        planner_model: "custom/model".into(),
        planner_input_price: Some(0.000001),
        planner_output_price: Some(0.000005),
        ..Default::default()
    }
}

#[tokio::test]
async fn advanced_text_only_model_explains_silent_video_incompatibility() {
    let settings = PlannerSettings {
        clipping_mode: ClippingMode::Advanced,
        planner_supports_images: false,
        ..Default::default()
    };
    let frames: Vec<VisionFrame> = [0, 10000, 20000]
        .iter()
        .map(|t| VisionFrame {
            timestamp_ms: *t,
            path: "missing.jpg".into(),
        })
        .collect();
    let input = PlanInput {
        frames: &frames,
        video_duration: Some(300.0),
        max_clips: Some(3),
        min_duration_seconds: Some(15),
        max_duration_seconds: Some(60),
        aspect_ratio: "9:16",
        ..Default::default()
    };
    let err = plan(&settings, &openrouter(), &input, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), VISUAL_PLANNING_UNSUPPORTED);
    assert!(err
        .to_string()
        .to_lowercase()
        .contains("selected planner requires a video with speech"));
}

#[tokio::test]
async fn advanced_cost_estimate_uses_selected_model_catalog_price() {
    let body = completion(
        &content(vec![clip(10.0, 40.0, five(8.0))]),
        "custom/model",
        None,
    );
    let (result, _) = plan_with_server(advanced_priced(), vec![Reply::json(200, &body)]).await;
    let costs = result.unwrap().costs.unwrap();
    assert_eq!(costs.estimated_cost_usd, 0.002);
    assert!(!costs.cost_incomplete);
}

#[tokio::test]
async fn retry_preserves_reported_cost_when_another_attempt_needs_an_estimate() {
    for reported_first in [true, false] {
        let bad = completion("{{not json", "custom/model", reported_first.then_some(0.01));
        let good = completion(
            &content(vec![clip(10.0, 40.0, five(8.0))]),
            "custom/model",
            (!reported_first).then_some(0.01),
        );
        let (result, _) = plan_with_server(
            advanced_priced(),
            vec![Reply::json(200, &bad), Reply::json(200, &good)],
        )
        .await;
        let costs = result.unwrap().costs.unwrap();
        assert_eq!(costs.estimated_cost_usd, 0.012);
        assert!(!costs.cost_incomplete);
    }
}

#[tokio::test]
async fn unknown_retry_price_keeps_known_charges_and_marks_total_incomplete() {
    let settings = PlannerSettings {
        clipping_mode: ClippingMode::Advanced,
        planner_model: "custom/model".into(),
        ..Default::default()
    };
    let bad = completion("{{not json", "custom/model", None);
    let good = completion(
        &content(vec![clip(10.0, 40.0, five(8.0))]),
        "custom/model",
        Some(0.01),
    );
    let (result, _) = plan_with_server(
        settings,
        vec![Reply::json(200, &bad), Reply::json(200, &good)],
    )
    .await;
    let costs = result.unwrap().costs.unwrap();
    assert_eq!(costs.estimated_cost_usd, 0.01);
    assert!(costs.cost_incomplete);
}

#[tokio::test]
async fn happy_path_records_real_cost_and_serving_model() {
    let settings = PlannerSettings {
        planner_model: "primary/model".into(),
        ..Default::default()
    };
    let text = json!({
        "insights": "Podcast",
        "clips": [clip(10.0, 40.0, five(6.0)), clip(100.0, 130.0, five(9.0)), clip(200.0, 230.0, five(8.0))],
    })
    .to_string();
    let (result, server) = plan_with_server(
        settings,
        vec![Reply::json(
            200,
            &completion(&text, "fallback/served", Some(0.0123)),
        )],
    )
    .await;
    let plan = result.unwrap();
    let scores: Vec<f64> = plan
        .segments
        .iter()
        .map(|c| (c.virality_score * 100.0).round() / 100.0)
        .collect();
    assert_eq!(scores, [0.9, 0.8, 0.6]);
    let costs = plan.costs.unwrap();
    assert_eq!(costs.model, "fallback/served");
    assert_eq!(costs.estimated_cost_usd, 0.0123);
    assert_eq!(costs.attempts, 1);
    assert_eq!(costs.provider, "openrouter");
    assert_eq!(plan.insights.as_deref(), Some("Podcast"));
    let sent = server.seen.lock().unwrap()[0].json();
    assert_eq!(sent["model"], "primary/model");
    assert_eq!(sent["max_tokens"], 32000);
    assert_eq!(sent["reasoning"]["effort"], "medium");
}

#[tokio::test]
async fn retries_rate_limit_then_succeeds() {
    let text = content(vec![clip(10.0, 40.0, five(8.0))]);
    let replies = vec![
        Reply::json(429, &json!({"error": {"message": "rate limited"}})),
        Reply::dropped(),
        Reply::json(200, &completion(&text, "google/test-model", Some(0.01))),
    ];
    let (result, _) = plan_with_server(PlannerSettings::default(), replies).await;
    let plan = result.unwrap();
    assert_eq!(plan.costs.unwrap().attempts, 3);
    assert_eq!(plan.segments.len(), 1);
}

#[tokio::test]
async fn non_retryable_error_fails_fast() {
    let (result, server) = plan_with_server(
        PlannerSettings::default(),
        vec![Reply::json(401, &json!({"error": {"message": "bad key"}}))],
    )
    .await;
    let err = result.unwrap_err().to_string();
    assert!(err.contains("(401)") && !err.contains("bad key"));
    assert_eq!(server.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_reply_cut_off_at_the_token_limit_is_not_resent() {
    let mut cut = completion("{\"insights\": \"x\", \"clips\": [{\"start_time\": 1", "m", Some(0.01));
    cut["choices"][0]["finish_reason"] = json!("length");
    let good = content(vec![clip(10.0, 40.0, five(8.0))]);
    let replies = vec![Reply::json(200, &cut), Reply::json(200, &completion(&good, "m", None))];
    let (result, server) = plan_with_server(PlannerSettings::default(), replies).await;
    assert!(result.unwrap_err().to_string().contains("output token limit"));
    assert_eq!(server.seen.lock().unwrap().len(), 1);
}

#[test]
fn a_timed_out_cli_is_retried_once() {
    let failed = PlanError::retry("Claude Code CLI failed (exit 3)");
    let timed_out = PlanError::retry("Claude Code CLI timed out");
    assert!(!give_up(&failed, 0) && !give_up(&failed, 1) && give_up(&failed, 2));
    assert!(!give_up(&timed_out, 0) && give_up(&timed_out, 1));
    assert!(give_up(&PlanError { message: "x".into(), retryable: false }, 0));
}

#[tokio::test]
async fn retries_malformed_output_and_sums_cost() {
    let good = content(vec![clip(10.0, 40.0, five(8.0))]);
    let replies = vec![
        Reply::json(200, &completion("{{not json", "m", Some(0.01))),
        Reply::json(200, &completion(&good, "m", Some(0.02))),
    ];
    let (result, _) = plan_with_server(PlannerSettings::default(), replies).await;
    let costs = result.unwrap().costs.unwrap();
    assert_eq!(costs.attempts, 2);
    assert_eq!(costs.estimated_cost_usd, 0.03);
}

// ---------------------------------------------------------------- test_plan_boundaries.py

#[test]
fn duration_bounds() {
    let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        resolve_clip_duration_bounds(Some(&r(&["long"])), Some(15), Some(90)),
        (120, 300)
    );
    assert_eq!(
        resolve_clip_duration_bounds(Some(&r(&["short", "long"])), None, None),
        (30, 300)
    );
    assert_eq!(
        resolve_clip_duration_bounds(None, Some(20), Some(45)),
        (20, 45)
    );
    assert_eq!(
        resolve_clip_duration_bounds(Some(&r(&["bogus"])), None, None),
        (15, 90)
    );
    assert_eq!(resolve_clip_duration_bounds(None, None, None), (15, 90));
    assert_eq!(
        resolve_clip_duration_bounds(None, Some(100), None),
        (100, 200)
    );
    assert_eq!(resolve_clip_duration_bounds(None, Some(20), None), (20, 90));
    let prompt = prompts::build_system_prompt(Some(5), Some(15), Some(90), Some(&r(&["long"])));
    assert!(prompt.contains("STRICTLY between 120 and 300 seconds"));
    assert!(prompt.contains("<= 300") && !prompt.contains("<= 90"));
}

fn four_sentences() -> Vec<TranscriptSegment> {
    (0..4)
        .map(|i| TranscriptSegment {
            start_time_ms: i * 5000,
            end_time_ms: (i + 1) * 5000 - 50,
            text: "Sentence.".into(),
            words: vec![word("Sentence.", i * 5000, (i + 1) * 5000 - 50)],
            ..Default::default()
        })
        .collect()
}

#[test]
fn rounding_below_minimum_extends_to_next_sentence_or_filters() {
    let tr = four_sentences();
    let segs = parse(
        &ctx(&tr, Some(15), Some(20)),
        &content(vec![clip(0.0, 15.0, five(5.0))]),
    );
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].end_time_ms, 19_950);
    assert!(parse(
        &ctx(&tr, Some(15), Some(15)),
        &content(vec![clip(0.0, 15.0, five(5.0))])
    )
    .is_empty());
}

#[test]
fn long_clip_survives_with_default_bounds_passed() {
    let tr = make_transcript(120);
    let (start, end) = (tr[10].start_time_ms, tr[60].end_time_ms);
    let ranges = vec!["long".to_string()];
    let c = ParseContext {
        duration_ranges: Some(&ranges),
        ..ctx(&tr, Some(15), Some(90))
    };
    let seg = &parse(
        &c,
        &content(vec![clip(shown(start), shown(end), five(5.0))]),
    )[0];
    assert_eq!(seg.end_time_ms, end);
}

#[test]
fn rounded_sentence_end_is_not_extended_into_next_sentence() {
    let tr = make_transcript(120);
    let c = ctx(&tr, Some(5), Some(600));
    for i in 20..60 {
        let true_end = tr[i].end_time_ms;
        let segs = parse(
            &c,
            &content(vec![clip(
                shown(tr[i - 5].start_time_ms),
                shown(true_end),
                five(5.0),
            )]),
        );
        assert_eq!(segs[0].end_time_ms, true_end);
    }
}

#[test]
fn over_long_clip_snaps_back_to_last_sentence_that_fits() {
    let tr = make_transcript(120);
    let seg = &parse(
        &ctx(&tr, Some(15), Some(60)),
        &content(vec![clip(
            shown(tr[30].start_time_ms),
            shown(tr[50].end_time_ms),
            five(5.0),
        )]),
    )[0];
    let d = (seg.end_time_ms - seg.start_time_ms) as f64 / 1000.0;
    assert!((15.0..=60.0).contains(&d));
    assert!(tr
        .iter()
        .any(|s| s.words.last().unwrap().end_time_ms == seg.end_time_ms));
}

#[test]
fn snapping_stays_inside_selected_range() {
    let tr = make_transcript(120);
    let range_start = tr[40].start_time_ms as f64 / 1000.0 + 1.0;
    let range_end = tr[70].end_time_ms as f64 / 1000.0 - 1.0;
    let c = ParseContext {
        start_limit: Some(range_start),
        end_limit: Some(range_end),
        ..ctx(&tr, Some(15), Some(90))
    };
    let segs = parse(
        &c,
        &content(vec![
            clip(range_start - 5.0, range_start + 40.0, five(5.0)),
            clip(range_end - 40.0, range_end + 5.0, five(5.0)),
        ]),
    );
    assert!(!segs.is_empty());
    for s in segs {
        assert!(s.start_time_ms as f64 >= range_start * 1000.0);
        assert!(s.end_time_ms as f64 <= range_end * 1000.0);
    }
}

#[test]
fn range_edges_mid_sentence() {
    let tr = make_transcript(120);
    let range_end = tr[70].end_time_ms as f64 / 1000.0 - 1.0;
    let c = ParseContext {
        end_limit: Some(range_end),
        ..ctx(&tr, Some(15), Some(90))
    };
    assert_eq!(
        parse(
            &c,
            &content(vec![clip(range_end - 40.0, range_end + 5.0, five(5.0))])
        )[0]
        .end_time_ms,
        tr[69].end_time_ms
    );

    let range_start = tr[40].start_time_ms as f64 / 1000.0 + 1.0;
    let c = ParseContext {
        start_limit: Some(range_start),
        ..ctx(&tr, Some(15), Some(90))
    };
    assert_eq!(
        parse(
            &c,
            &content(vec![clip(range_start - 5.0, range_start + 40.0, five(5.0))])
        )[0]
        .start_time_ms,
        tr[40].words[3].start_time_ms
    );

    let range_start = tr[40].start_time_ms as f64 / 1000.0 + 2.0;
    let c = ParseContext {
        start_limit: Some(range_start),
        ..ctx(&tr, Some(15), Some(90))
    };
    assert_eq!(
        parse(
            &c,
            &content(vec![clip(range_start - 5.0, range_start + 40.0, five(5.0))])
        )[0]
        .start_time_ms,
        tr[41].start_time_ms
    );
}

#[test]
fn empty_clip_list_parses_to_empty_plan() {
    let tr = make_transcript(120);
    assert!(parse(&ctx(&tr, None, None), &content(vec![])).is_empty());
}

#[tokio::test]
async fn range_shorter_than_min_clip_skips_the_paid_call() {
    let tr = make_transcript(10);
    let start = tr[2].start_time_ms as f64 / 1000.0;
    let ranges = vec!["short".to_string()];
    let input = PlanInput {
        transcript: &tr,
        duration_ranges: Some(&ranges),
        start_time_seconds: Some(start),
        end_time_seconds: Some(start + 20.0),
        max_clips: Some(5),
        auto_clip_count: true,
        aspect_ratio: "9:16",
        ..Default::default()
    };
    // No key and an unroutable base URL: any provider call would fail the test.
    let settings = test_settings("http://127.0.0.1:1");
    let ai = AiSettings {
        provider: "openrouter".into(),
        ..Default::default()
    };
    let plan = plan(&settings, &ai, &input, &CancellationToken::new())
        .await
        .unwrap();
    assert!(plan.segments.is_empty());
    assert!(plan
        .insights
        .unwrap()
        .contains("shorter than the minimum clip length"));
}

fn engine_config(work: &Path, ai: AiSettings) -> EngineConfig {
    EngineConfig {
        ffmpeg: work.join("no-ffmpeg"),
        ffprobe: work.join("no-ffprobe"),
        ytdlp: None,
        assets_dir: work.into(),
        whisper_model_path: work.join("model.bin"),
        ai,
        custom_vocabulary: vec![],
        work_dir: work.into(),
        output_dir: work.into(),
    }
}

fn job_request() -> ClipJobRequest {
    serde_json::from_value(json!({
        "videoUrl": "x", "maxClips": null, "autoClipCount": true, "durationRanges": null, "aspectRatio": "9:16",
        "layoutStyle": "auto", "layoutVision": false, "pacing": "tight", "includeCaptions": true,
        "captionPreset": "pop", "startTimeSeconds": null, "endTimeSeconds": null,
        "bannerPlatform": null, "bannerChannelUrl": null,
    }))
    .unwrap()
}

#[tokio::test]
async fn no_speech_fails_with_clear_message() {
    let work = tempfile::tempdir().unwrap();
    let cfg = engine_config(work.path(), openrouter());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let progress: ProgressFn = Arc::new(move |p: Progress| seen2.lock().unwrap().push(p.step));
    let settings = test_settings("http://127.0.0.1:1");
    let err = plan_clips_with(
        &settings,
        &Transcript::default(),
        &job_request(),
        &cfg,
        &work.path().join("x.mp4"),
        300.0,
        &progress,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(err
        .to_string()
        .starts_with("No clip-worthy moments found ("));
    assert_eq!(
        *seen.lock().unwrap(),
        ["Analyzing video frames...", "Planning viral clips..."]
    );
}

#[tokio::test]
async fn plan_clips_returns_a_transcript_plan() {
    let tr = planner_transcript(300);
    let transcript = Transcript {
        segments: tr,
        ..Default::default()
    };
    let text = content(vec![clip(10.0, 40.0, five(8.0))]);
    let server = serve(vec![Reply::json(200, &completion(&text, "m", Some(0.5)))]).await;
    let work = tempfile::tempdir().unwrap();
    let cfg = engine_config(work.path(), openrouter());
    let progress: ProgressFn = Arc::new(|_| {});
    let mut req = job_request();
    req.requirements = Some("Mention Acme.".into());
    let settings = PlannerSettings {
        base_url: Some(server.url.clone()),
        ..PlannerSettings::for_job(&cfg.ai, req.requirements.as_deref())
    };
    let plan = plan_clips_with(
        &settings,
        &transcript,
        &req,
        &cfg,
        Path::new("x.mp4"),
        300.0,
        &progress,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(plan.planning_source, "transcript");
    assert_eq!(plan.clips.len(), 1);
    assert_eq!(plan.visual_frame_count, 0);
    assert_eq!(plan.insights.as_deref(), Some("x"));
    let sent = server.seen.lock().unwrap()[0].json();
    assert!(sent["messages"][0]["content"]
        .as_str()
        .unwrap()
        .ends_with("<<<BRIEF\nMention Acme.\nBRIEF>>>"));
}

#[tokio::test]
async fn empty_plan_under_a_brief_blames_the_brief() {
    let transcript = Transcript { segments: planner_transcript(300), ..Default::default() };
    let server = serve(vec![Reply::json(200, &completion(&content(vec![]), "m", Some(0.5)))]).await;
    let work = tempfile::tempdir().unwrap();
    let cfg = engine_config(work.path(), openrouter());
    let progress: ProgressFn = Arc::new(|_| {});
    let mut req = job_request();
    req.requirements = Some("Only cooking clips.".into());
    let settings = PlannerSettings {
        base_url: Some(server.url.clone()),
        ..PlannerSettings::for_job(&cfg.ai, req.requirements.as_deref())
    };
    let err = plan_clips_with(&settings, &transcript, &req, &cfg, Path::new("x.mp4"), 300.0, &progress, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), NO_CLIPS_MATCH_BRIEF);
}

// ---------------------------------------------------------------- settings per provider

#[test]
fn job_settings_follow_the_bridge() {
    let cli = AiSettings {
        provider: "cli:claude".into(),
        ..Default::default()
    };
    let s = PlannerSettings::for_job(&cli, None);
    assert!(!s.planner_supports_images);
    assert!(!s.planner_fallback_models().is_empty());
    let direct = AiSettings {
        provider: "anthropic".into(),
        ..Default::default()
    };
    let s = PlannerSettings::for_job(&direct, Some("rules"));
    assert!(s.planner_fallback_models().is_empty() && s.planner_supports_images);
    assert_eq!(s.clip_requirements, "rules");
    let or = AiSettings {
        provider: "openrouter".into(),
        model: "google/gemini-3.8-flash".into(),
        ..Default::default()
    };
    let s = PlannerSettings::for_job(&or, None);
    assert_eq!(s.planner_model, "google/gemini-3.8-flash");
    assert_eq!(s.planner_fallback_models(), ["openai/gpt-6-sol"]);
}

// ---------------------------------------------------------------- test_visual_fallback.py

#[test]
fn visual_sampler_bounds_count_and_selected_range() {
    let times = visual::sample_timestamps(3600.0, Some(120.0), Some(240.0));
    assert_eq!(times.len(), 8);
    assert!(times.iter().all(|t| 120.0 < *t && *t < 240.0));

    let times = visual::sample_timestamps(3600.0, None, None);
    assert_eq!(times.len(), visual::MAX_PLANNING_FRAMES);
    assert!(times.windows(2).all(|w| w[0] <= w[1]));
    assert!(times.iter().all(|t| 0.0 < *t && *t < 3600.0));
    assert!(times.windows(2).filter(|w| w[1] - w[0] <= 45.0).count() >= 16);

    assert!(visual::sample_timestamps(f64::NAN, None, None).is_empty());
    assert!(visual::sample_timestamps(60.0, Some(50.0), Some(40.0)).is_empty());
    assert_eq!(visual::sample_timestamps(10.0, None, None).len(), 3);
}

#[test]
fn visual_change_gate() {
    let still = |shade: u8| Some(vec![shade; 64 * 36]);
    assert!(!visual::visual_change_in(
        &[0, 1000, 2000],
        &[still(32), still(32), still(32)]
    ));
    assert!(visual::visual_change_in(
        &[0, 1000, 2000],
        &[still(32), still(32), still(200)]
    ));
    // Change only between frames far apart does not count.
    assert!(!visual::visual_change_in(
        &[10_000, 20_000, 2_000_000],
        &[still(32), still(32), still(200)]
    ));
    // An unreadable frame stops the check.
    assert!(!visual::visual_change_in(
        &[0, 1000, 2000],
        &[still(32), None, still(200)]
    ));
    assert!(!visual::visual_change_in(
        &[0, 1000],
        &[still(0), still(255)]
    ));
}

#[tokio::test]
async fn visual_only_planner_uses_duration_and_rejects_unsupported_clip() {
    let work = tempfile::tempdir().unwrap();
    let frames: Vec<VisionFrame> = [10, 20, 30, 40]
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let path = work.path().join(format!("{i}.jpg"));
            std::fs::write(&path, vec![b'j'; 1200]).unwrap();
            VisionFrame {
                timestamp_ms: s * 1000,
                path,
            }
        })
        .collect();
    let body = json!({"insights": "Visual demonstration", "clips": [
        {"start_time": 10, "end_time": 40, "summary": "Visible Result", "emphasis": ["invented"]},
        {"start_time": 45, "end_time": 75, "summary": "Unsupported Result"},
    ]})
    .to_string();
    let server = serve(vec![Reply::json(200, &completion(&body, "m", Some(0.01)))]).await;
    let input = PlanInput {
        frames: &frames,
        video_duration: Some(60.0),
        min_duration_seconds: Some(15),
        max_duration_seconds: Some(45),
        max_clips: Some(5),
        auto_clip_count: true,
        aspect_ratio: "9:16",
        ..Default::default()
    };
    let plan = plan(
        &test_settings(&server.url),
        &openrouter(),
        &input,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(plan.segments.len(), 1);
    assert_eq!(
        (plan.segments[0].start_time_ms, plan.segments[0].end_time_ms),
        (10_000, 40_000)
    );
    assert!(plan.segments[0].emphasis_words.is_empty());
    let sent = server.seen.lock().unwrap()[0].json();
    let first = sent["messages"][1]["content"][0]["text"].as_str().unwrap();
    assert!(first.contains("No usable speech transcript"));
    assert!(first.contains("approximately 60 seconds"));
    assert_eq!(
        sent["messages"][1]["content"][2]["image_url"]["detail"],
        "low"
    );
    assert!(sent["messages"][0]["content"]
        .as_str()
        .unwrap()
        .starts_with("You are selecting short clips"));
}

#[tokio::test]
async fn cli_planner_sends_no_images_for_speech() {
    // Text-only providers still plan transcripts; frames are dropped.
    let settings = PlannerSettings {
        planner_supports_images: false,
        ..Default::default()
    };
    let tr = planner_transcript(300);
    let frames = vec![
        VisionFrame {
            timestamp_ms: 0,
            path: "x.jpg".into()
        };
        3
    ];
    let text = content(vec![clip(10.0, 40.0, five(8.0))]);
    let server = serve(vec![Reply::json(200, &completion(&text, "m", Some(0.0)))]).await;
    let input = PlanInput {
        frames: &frames,
        ..input(&tr)
    };
    let settings = PlannerSettings {
        base_url: Some(server.url.clone()),
        ..settings
    };
    plan(&settings, &openrouter(), &input, &CancellationToken::new())
        .await
        .unwrap();
    let sent = server.seen.lock().unwrap()[0].json();
    assert!(sent["messages"][1]["content"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["type"] == "text"));
}

// ---------------------------------------------------------------- test_longform.py (planner)

#[test]
fn longform_is_16_9_and_five_minutes_plus() {
    assert!(is_longform("16:9", 600));
    assert!(is_longform("16:9", 300));
    assert!(!is_longform("16:9", 120));
    assert!(!is_longform("9:16", 600));
}

async fn run_longform_plan(
    transcript: &[TranscriptSegment],
    ranges: &[&str],
    aspect: &str,
) -> Value {
    let server = serve(vec![Reply::json(
        200,
        &completion(&content(vec![]), "m", Some(0.0)),
    )])
    .await;
    let ranges: Vec<String> = ranges.iter().map(|s| s.to_string()).collect();
    let input = PlanInput {
        transcript,
        duration_ranges: Some(&ranges),
        aspect_ratio: aspect,
        max_clips: Some(5),
        auto_clip_count: true,
        ..Default::default()
    };
    plan(
        &test_settings(&server.url),
        &openrouter(),
        &input,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let sent = server.seen.lock().unwrap()[0].json();
    sent
}

#[tokio::test]
async fn clip_count_is_capped_to_what_fits() {
    let payload = run_longform_plan(&make_transcript(300), &["extended"], "16:9").await;
    let prompt = payload["messages"][0]["content"].as_str().unwrap();
    let parts = payload["messages"][1]["content"].as_array().unwrap();
    let last = parts.last().unwrap()["text"].as_str().unwrap();
    assert!(prompt.contains("up to 2 standalone horizontal episodes"));
    assert!(last.contains("up to 2 complete, self-contained longform episodes"));
    let item = &payload["response_format"]["json_schema"]["schema"]["properties"]["clips"]["items"];
    for key in ["skip", "chapters", "description"] {
        assert!(item["required"].as_array().unwrap().contains(&json!(key)));
    }
}

#[tokio::test]
async fn short_form_prompt_and_schema_are_unchanged() {
    let payload = run_longform_plan(&make_transcript(300), &["short"], "16:9").await;
    assert!(payload["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("scroll-stopping"));
    assert!(
        payload["response_format"]["json_schema"]["schema"]["properties"]["clips"]["items"]
            ["properties"]
            .get("skip")
            .is_none()
    );
    let payload = run_longform_plan(&make_transcript(300), &["extended"], "9:16").await;
    assert!(!payload["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("senior YouTube editor"));
}

fn longform_clip(start: f64, end: f64, skip: &[(f64, f64)], chapters: &[(f64, &str)]) -> Value {
    json!({
        "start_time": start, "end_time": end, "summary": "The Real Story", "tags": ["a"], "emphasis": [],
        "scores": {"hook": 7, "standalone": 7, "arc": 7, "quotability": 7, "ending": 7},
        "skip": skip.iter().map(|(a, b)| json!({"start_time": a, "end_time": b})).collect::<Vec<_>>(),
        "chapters": chapters.iter().map(|(t, title)| json!({"time": t, "title": title})).collect::<Vec<_>>(),
        "description": "About things.",
    })
}

fn longform_ctx<'a>(tr: &'a [TranscriptSegment], ranges: &'a [String]) -> ParseContext<'a> {
    ParseContext {
        duration_ranges: Some(ranges),
        transcript: tr,
        longform: true,
        ..Default::default()
    }
}

#[test]
fn skips_and_chapters_are_cleaned() {
    let tr = make_transcript(200);
    let ranges = vec!["extended".to_string()];
    let s = |i: usize| tr[i].start_time_ms as f64 / 1000.0;
    let start = s(0);
    let end = tr[159].end_time_ms as f64 / 1000.0;
    let tangent = (s(40) + 1.0, s(52) + 0.5);
    let clip = longform_clip(
        start,
        end,
        &[tangent, (s(80), s(80) + 3.0), (end + 10.0, end + 60.0)],
        &[
            (start + 5.0, "Opening"),
            (s(45), "Inside skip"),
            (s(100), "Main point"),
            (s(101), "Too close"),
        ],
    );
    let seg = &parse(&longform_ctx(&tr, &ranges), &content(vec![clip]))[0];
    assert_eq!(seg.skip_ranges_ms.len(), 1);
    let (s_ms, e_ms) = seg.skip_ranges_ms[0];
    assert!(tr
        .iter()
        .any(|x| x.words.last().unwrap().end_time_ms == s_ms));
    assert!(tr.iter().any(|x| x.start_time_ms == e_ms));
    assert_eq!(seg.chapters[0].0, seg.start_time_ms);
    let titles: Vec<&str> = seg.chapters.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(titles, ["Opening", "Inside skip", "Main point"]);
    assert_eq!(seg.chapters[1].0, e_ms);
    assert_eq!(seg.description.as_deref(), Some("About things."));
}

#[test]
fn skips_never_cut_below_the_minimum_runtime() {
    let tr = make_transcript(200);
    let ranges = vec!["extended".to_string()];
    let s = |i: usize| tr[i].start_time_ms as f64 / 1000.0;
    let clip = longform_clip(
        s(0),
        tr[125].end_time_ms as f64 / 1000.0,
        &[(s(20), s(60))],
        &[],
    );
    let seg = &parse(&longform_ctx(&tr, &ranges), &content(vec![clip]))[0];
    assert!(seg.skip_ranges_ms.is_empty());
}

#[test]
fn chapter_and_skip_helpers_handle_garbage() {
    assert!(clean_skips(Some(&json!("x")), 0, 100_000, 10_000, &[]).is_empty());
    assert!(clean_chapters(
        Some(&json!([{"time": "x", "title": "t"}, 3])),
        0,
        100_000,
        &[],
        None
    )
    .is_empty());
    let chapters = clean_chapters(
        Some(&json!([{"time": 50, "title": "  Mid   point "}])),
        0,
        100_000,
        &[],
        None,
    );
    assert_eq!(
        chapters,
        [(0, "Intro".to_string()), (50_000, "Mid point".to_string())]
    );
}

#[tokio::test]
async fn local_planning_uses_the_same_prompts_and_costs_nothing() {
    let ai = AiSettings {
        provider: "local".into(),
        model: "qwen2.5:14b".into(),
        ..Default::default()
    };
    let s = PlannerSettings::for_job(&ai, None);
    assert!(!s.planner_supports_images && s.planner_fallback_models().is_empty());
    let with_images = AiSettings {
        local_images: true,
        ..ai.clone()
    };
    assert!(PlannerSettings::for_job(&with_images, None).planner_supports_images);

    let text = content(vec![clip(10.0, 40.0, five(8.0))]);
    // The server refuses the schema once; the plan still parses from a fenced reply.
    let server = serve(vec![
        Reply::json(400, &json!({"error": "json_schema is not supported"})),
        Reply::json(
            200,
            &completion(&format!("```json\n{text}\n```"), "qwen2.5:14b", None),
        ),
    ])
    .await;
    let settings = PlannerSettings {
        base_url: Some(server.url.clone()),
        retry_base_delay: Duration::ZERO,
        ..s
    };
    let tr = planner_transcript(300);
    let plan = plan(&settings, &ai, &input(&tr), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(plan.segments.len(), 1);
    let costs = plan.costs.unwrap();
    assert_eq!(costs.provider, "local");
    assert_eq!(costs.model, "qwen2.5:14b");
    assert_eq!(costs.estimated_cost_usd, 0.0);
    assert!(!costs.cost_incomplete);
    let seen = server.seen.lock().unwrap().clone();
    let first = seen[0].json();
    assert_eq!(first["model"], "qwen2.5:14b");
    // Same system prompt as a hosted provider; the fallback only appends the schema.
    let fallback = seen[1].json()["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(fallback.starts_with(first["messages"][0]["content"].as_str().unwrap()));
}

// ---------------------------------------------------------------- long transcripts in parts

#[test]
fn long_ranges_split_into_equal_overlapping_parts() {
    assert_eq!(planning_parts(0.0, 75.0 * 60.0, 60), None);
    let parts = planning_parts(0.0, 76.0 * 60.0, 60).unwrap();
    assert_eq!(parts, [(0.0, 2340.0), (2280.0, 4560.0)]);
    assert_eq!(planning_parts(0.0, 4.0 * 3600.0, 90).unwrap().len(), 4);
    // Feature-length clips (15-30 min) get 90 min parts, so 80 min stays one call.
    assert_eq!(planning_parts(0.0, 80.0 * 60.0, 1800), None);
    let parts = planning_parts(600.0, 600.0 + 3.0 * 3600.0, 60).unwrap();
    assert_eq!((parts[0].0, parts[2].1), (600.0, 11_400.0));
}

#[test]
fn clips_entirely_outside_the_range_are_dropped_not_clamped() {
    let tr = planner_transcript(600);
    let c = ParseContext {
        start_limit: Some(200.0),
        end_limit: Some(400.0),
        drop_outside_range: true,
        ..ctx(&tr, Some(15), Some(60))
    };
    let clips = vec![
        clip(10.0, 40.0, five(8.0)),
        clip(450.0, 480.0, five(8.0)),
        clip(250.0, 280.0, five(8.0)),
    ];
    let segs = parse(&c, &content(clips));
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].start_time_ms, 250_000);
}

#[tokio::test]
async fn long_transcript_is_planned_in_parallel_parts_and_merged() {
    // Every part gets the same reply and keeps only the clips inside its range.
    // 3160 s lies in the overlap of parts 1 and 2, so both return it.
    let starts = [600.0, 3160.0, 5000.0, 9000.0, 12_000.0];
    let reply = completion(
        &content(starts.iter().map(|s| clip(*s, s + 30.0, five(8.0))).collect()),
        "m",
        Some(0.01),
    );
    let server = serve((0..4).map(|_| Reply::json(200, &reply)).collect()).await;
    let tr = planner_transcript(12_600);
    let input = PlanInput {
        video_duration: Some(12_600.0),
        max_clips: Some(10),
        ..input(&tr)
    };
    let steps = Arc::new(Mutex::new(Vec::new()));
    let seen = steps.clone();
    let progress: ProgressFn = Arc::new(move |p: Progress| seen.lock().unwrap().push(p.step));
    let plan = plan_in_parts(
        &test_settings(&server.url),
        &openrouter(),
        &input,
        &progress,
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    let mut got: Vec<i64> = plan.segments.iter().map(|c| c.start_time_ms / 1000).collect();
    got.sort();
    assert_eq!(got, [600, 3160, 5000, 9000, 12_000]);
    let costs = plan.costs.unwrap();
    assert_eq!((costs.attempts, costs.estimated_cost_usd), (4, 0.04));
    // Each call carries only its own part, and the seam goes to both neighbours.
    let bodies: Vec<String> = server.seen.lock().unwrap().iter().map(|r| r.json().to_string()).collect();
    assert_eq!(bodies.len(), 4);
    let calls_with = |line: &str| bodies.iter().filter(|b| b.contains(line)).count();
    assert_eq!(calls_with("word12500 end."), 1);
    assert_eq!(calls_with("word3180 end."), 2);
    assert_eq!(steps.lock().unwrap().last().unwrap(), "Planning clips · 4 of 4 parts done");
}

#[test]
fn let_ai_decide_leaves_the_clip_count_out_of_the_prompt() {
    let system = prompts::build_system_prompt(None, Some(15), Some(90), None);
    assert!(!system.contains("Return exactly") && system.contains("When unsure, include it"));
    let segs = planner_transcript(60);
    let parts = prompts::build_user_parts("transcript", &[], 34, false, &segs, 60.0, false);
    let UserPart::Text(last) = parts.last().unwrap() else { panic!() };
    assert!(!last.contains("34") && last.contains("When unsure, include it"));
}
