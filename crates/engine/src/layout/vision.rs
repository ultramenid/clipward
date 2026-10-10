//! Optional vision-model refinement: one call per distinct visual setup,
//! routed through `llm::complete`.
use super::analysis::{FrameInfo, ALL_LAYOUTS};
use super::imgproc::compare_hist_bhattacharyya;
use super::types::ShotLayout;
use crate::error::{EngineError, Result};
use crate::llm::{self, LlmRequest};
use crate::types::AiSettings;
use serde_json::{json, Value};
use std::sync::Mutex;
use tokio::sync::{watch, Semaphore};
use tokio_util::sync::CancellationToken;

pub fn vision_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "layout": {"type": "string", "enum": ALL_LAYOUTS},
            "cam_box": {
                "type": "array", "items": {"type": "integer"},
                "description": "Webcam overlay [ymin, xmin, ymax, xmax] 0-1000, or [] if none.",
            },
            "screen_box": {
                "type": "array", "items": {"type": "integer"},
                "description": "Screen/app content region [ymin, xmin, ymax, xmax] 0-1000, or [] if none.",
            },
            "screen_focus": {
                "type": "array", "items": {"type": "integer"},
                "description": "Most important area inside the screen [ymin, xmin, ymax, xmax] 0-1000, or [] if none.",
            },
            "people": {
                "type": "array",
                "items": {"type": "array", "items": {"type": "integer"}},
                "description": "Head-and-shoulders box [ymin, xmin, ymax, xmax] 0-1000 for each on-camera person (not inside the webcam overlay).",
            },
        },
        "required": ["layout", "cam_box", "screen_box", "screen_focus", "people"],
        "additionalProperties": false,
    })
}

pub const VISION_PROMPT: &str = r#"You are framing a frame from a video for a 9:16 vertical short.

Classify the frame's layout:
- "talking_head": one person on camera fills a meaningful part of the frame (no screen recording).
- "two_shot": two people on camera, e.g. a podcast wide shot.
- "screen_cam": a screen recording, slides, browser, code or gameplay WITH a webcam overlay of the presenter.
- "screen": screen content, graphics or b-roll with NO webcam overlay, or a crowd/group where no single framing works.

Return boxes as [ymin, xmin, ymax, xmax] integers from 0 to 1000 relative to the full frame:
- cam_box: the ENTIRE webcam overlay rectangle (its visible border/edges, including background around the person), not just the face. [] if there is no webcam overlay.
- screen_box: the region holding the screen/app content, excluding black bars and the webcam overlay if it sits outside the screen. [] if there is no screen content.
- screen_focus: inside screen_box, the area a viewer should see when the screen is cropped for a phone: the active editor/document pane, chat window, chart, or game view. Leave out sidebars, toolbars and empty space. [] if the whole screen matters equally.
- people: one head-and-shoulders box per on-camera person, left to right. Exclude people inside the webcam overlay and people shown inside screen content.

Detected faces (normalized x, y, w, h, may be incomplete): {faces}"#;

/// A retained analysis frame for the vision model: window time, JPEG bytes
/// and the frame's H/S histogram (for the per-setup answer cache).
#[derive(Debug, Clone)]
pub struct Keyframe {
    pub t_ms: i64,
    pub jpeg: Vec<u8>,
    pub hist: Vec<f32>,
}

/// Vision answers reused across clips of the same video, keyed by frame look
/// and heuristic signature. Share one per job (it is `Sync`). A setup still
/// being asked about holds a sender; callers for the same setup wait on it.
type Signature = (String, usize);
type CacheEntry = (Vec<f32>, Signature, std::result::Result<Value, watch::Sender<()>>);

#[derive(Debug, Default)]
pub struct VisionCache {
    entries: Mutex<Vec<CacheEntry>>,
}

enum Claim<'a> {
    Answer(Value),
    /// Another call is asking about this setup; closes when it settles.
    Wait(watch::Receiver<()>),
    /// Ask the model; the setup stays reserved until `Asking` answers or drops.
    Ask(Asking<'a>),
}

struct Asking<'a> {
    cache: &'a VisionCache,
    done: watch::Receiver<()>,
}

impl VisionCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn claim(&self, hist: &[f32], signature: &Signature) -> Claim<'_> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        match entries.iter().find(|(h, sig, _)| sig == signature && compare_hist_bhattacharyya(hist, h) < 0.2) {
            Some((_, _, Ok(v))) => Claim::Answer(v.clone()),
            Some((_, _, Err(tx))) => Claim::Wait(tx.subscribe()),
            None => {
                let (tx, done) = watch::channel(());
                entries.push((hist.to_vec(), signature.clone(), Err(tx)));
                Claim::Ask(Asking { cache: self, done })
            }
        }
    }
}

impl Asking<'_> {
    /// Store the answer (dropping the sender wakes the waiters).
    fn answer(self, result: Value) {
        let mut entries = self.cache.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = entries.iter_mut().find(|e| matches!(&e.2, Err(tx) if tx.subscribe().same_channel(&self.done))) {
            e.2 = Ok(result);
        }
    }
}

impl Drop for Asking<'_> {
    /// No answer (failure, cancellation): free the setup; waiters claim it again.
    fn drop(&mut self) {
        let mut entries = self.cache.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|e| !matches!(&e.2, Err(tx) if tx.subscribe().same_channel(&self.done)));
    }
}

/// Whether the configured provider can answer layout questions about images.
pub fn vision_available(ai: &AiSettings) -> bool {
    let has_key = ai.openrouter_api_key.as_deref().is_some_and(|k| !k.trim().is_empty());
    llm::ai_supports_images(ai) && (has_key || ai.provider != "openrouter")
}

pub fn pick_keyframe(keyframes: &[Keyframe], t_ms: i64) -> Option<&Keyframe> {
    // min(): the first of equally near keyframes wins.
    keyframes.iter().fold(None, |best: Option<&Keyframe>, k| match best {
        Some(b) if (b.t_ms - t_ms).abs() <= (k.t_ms - t_ms).abs() => Some(b),
        _ => Some(k),
    })
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// JSON list of up to six distinct face boxes sampled across the shot, in the
/// same text form the Python prompt used (`json.dumps`).
pub fn faces_hint(shot_frames: &[FrameInfo]) -> String {
    let step = (shot_frames.len() / 4).max(1);
    let mut faces: Vec<[f64; 4]> = shot_frames
        .iter()
        .step_by(step)
        .flat_map(|f| f.faces.iter().map(|b| b.to_list().map(round3)))
        .collect();
    faces.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    faces.dedup();
    faces.truncate(6);
    let fmt = |v: f64| serde_json::to_string(&v).unwrap_or_else(|_| "0.0".into());
    let items: Vec<String> =
        faces.iter().map(|b| format!("[{}]", b.iter().map(|v| fmt(*v)).collect::<Vec<_>>().join(", "))).collect();
    format!("[{}]", items.join(", "))
}

/// Pull the JSON object out of a model reply (tolerates code fences / prose).
fn parse_reply(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return v.is_object().then_some(v);
    }
    let (start, end) = (trimmed.find('{')?, trimmed.rfind('}')?);
    serde_json::from_str::<Value>(trimmed.get(start..=end)?).ok().filter(Value::is_object)
}

/// One shot to ask about: its keyframe (None: skip it), frames and heuristic layout.
pub type VisionAsk<'a> = (Option<&'a Keyframe>, &'a [FrameInfo], &'a ShotLayout);

/// Ask the vision model about each shot's keyframe, all at once; cached per visual setup.
/// Setups are claimed in shot order before anything is sent, so a shot reuses
/// the same earlier answer it would asking one after another (when calls succeed).
/// Provider failures fall back to heuristics (None); only cancellation errors out.
pub async fn vision_classify(
    ai: &AiSettings,
    shots: &[VisionAsk<'_>],
    cache: &VisionCache,
    cancel: &CancellationToken,
) -> Result<Vec<(Option<Value>, f64)>> {
    let signature = |h: &ShotLayout| (h.layout.clone(), h.people.len());
    let claims: Vec<Option<Claim>> = shots.iter().map(|(k, _, h)| k.map(|k| cache.claim(&k.hist, &signature(h)))).collect();
    // At most 4 calls at once per clip (a local server answers one at a time).
    let calls = &Semaphore::new(4);
    futures::future::try_join_all(shots.iter().zip(claims).map(|(&(keyframe, shot_frames, heuristic), claim)| async move {
        let (Some(keyframe), Some(mut claim)) = (keyframe, claim) else { return Ok((None, 0.0)) };
        loop {
            match claim {
                Claim::Answer(cached) => return Ok((Some(cached), 0.0)),
                Claim::Wait(mut done) => {
                    tokio::select! {
                        _ = done.changed() => {}
                        _ = cancel.cancelled() => return Err(EngineError::Cancelled),
                    }
                    claim = cache.claim(&keyframe.hist, &signature(heuristic));
                }
                Claim::Ask(asking) => {
                    let _call = calls.acquire().await.expect("never closed");
                    return ask(ai, keyframe, shot_frames, asking, cancel).await;
                }
            }
        }
    }))
    .await
}

async fn ask(
    ai: &AiSettings,
    keyframe: &Keyframe,
    shot_frames: &[FrameInfo],
    asking: Asking<'_>,
    cancel: &CancellationToken,
) -> Result<(Option<Value>, f64)> {
    let req = LlmRequest {
        system: String::new(),
        user: VISION_PROMPT.replace("{faces}", &faces_hint(shot_frames)),
        images: vec![keyframe.jpeg.clone()],
        json_schema: Some(vision_schema()),
        max_tokens: 4000,
        temperature: Some(0.0),
    };
    match llm::complete(ai, req, cancel).await {
        Ok(resp) => match parse_reply(&resp.text) {
            Some(result) => {
                asking.answer(result.clone());
                Ok((Some(result), resp.cost_usd))
            }
            None => {
                tracing::warn!("Layout vision returned invalid JSON, using heuristics");
                Ok((None, resp.cost_usd))
            }
        },
        Err(EngineError::Cancelled) => Err(EngineError::Cancelled),
        Err(e) => {
            tracing::warn!("Layout vision failed, using heuristics: {e}");
            Ok((None, 0.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::types::Box;

    #[test]
    fn faces_hint_matches_python_json() {
        let frames: Vec<FrameInfo> = (0..8)
            .map(|i| FrameInfo {
                t_ms: i * 250,
                faces: vec![Box { x: 0.84, y: 0.8, w: 0.05, h: 0.09 }],
                ..Default::default()
            })
            .collect();
        assert_eq!(faces_hint(&frames), "[[0.84, 0.8, 0.05, 0.09]]");
        assert_eq!(faces_hint(&[]), "[]");
    }

    #[test]
    fn reply_parsing_tolerates_fences() {
        assert!(parse_reply("```json\n{\"layout\": \"screen\"}\n```").is_some());
        assert!(parse_reply("[1, 2]").is_none());
        assert!(parse_reply("nope").is_none());
    }

    #[test]
    fn availability_needs_images_and_a_key_for_openrouter() {
        let mut ai = AiSettings { provider: "openrouter".into(), ..Default::default() };
        assert!(!vision_available(&ai));
        ai.openrouter_api_key = Some("k".into());
        assert!(vision_available(&ai));
        ai.provider = "anthropic".into();
        assert!(vision_available(&ai));
        ai.provider = "cli:claude".into();
        assert!(!vision_available(&ai));
        // A local model sees images only when the user says it accepts them.
        ai.provider = "local".into();
        assert!(!vision_available(&ai));
        ai.local_images = true;
        assert!(vision_available(&ai));
    }

    #[test]
    fn a_setup_being_asked_about_is_asked_once() {
        let cache = VisionCache::new();
        let sig = ("screen".to_string(), 0);
        let (a, b) = (vec![1.0, 0.0], vec![0.0, 1.0]);
        let Claim::Ask(first) = cache.claim(&a, &sig) else { panic!("first caller asks") };
        let Claim::Wait(done) = cache.claim(&a, &sig) else { panic!("same setup waits") };
        assert!(matches!(cache.claim(&b, &sig), Claim::Ask(_)), "another look asks");
        assert!(matches!(cache.claim(&a, &("talking_head".into(), 1)), Claim::Ask(_)), "another layout asks");
        drop(first);
        assert!(done.has_changed().is_err(), "a failed ask wakes the waiters");
        let Claim::Ask(again) = cache.claim(&a, &sig) else { panic!("waiters claim it again") };
        again.answer(json!({"layout": "screen"}));
        assert!(matches!(cache.claim(&a, &sig), Claim::Answer(v) if v["layout"] == "screen"));
    }

    #[tokio::test]
    async fn shots_ask_once_per_setup_and_share_the_answer() {
        use crate::llm::test_server::{serve, Reply};
        let reply = |layout: &str| {
            let content = json!({"layout": layout}).to_string();
            Reply::json(200, &json!({"model": "m", "choices": [{"message": {"content": content}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1, "completion_tokens": 1}}))
        };
        let server = serve(vec![reply("screen"), reply("talking_head")]).await;
        let ai = AiSettings { provider: "local".into(), model: "m".into(), base_url: Some(server.url.clone()), local_images: true, ..Default::default() };
        let k = |hist: Vec<f32>| Keyframe { t_ms: 0, jpeg: vec![0], hist };
        let (a, b) = (k(vec![1.0, 0.0]), k(vec![0.0, 1.0]));
        let shot = ShotLayout { layout: "screen".into(), ..Default::default() };
        let asks: [VisionAsk; 4] = [(Some(&a), &[], &shot), (Some(&b), &[], &shot), (Some(&a), &[], &shot), (None, &[], &shot)];
        let got = vision_classify(&ai, &asks, &VisionCache::new(), &CancellationToken::new()).await.unwrap();
        assert_eq!(server.seen.lock().unwrap().len(), 2, "one call per setup");
        assert!(got[0].0.is_some() && got[0].0 == got[2].0 && got[0].0 != got[1].0);
        assert!(got[3].0.is_none());
    }

    #[test]
    fn keyframe_pick_is_nearest() {
        let k = |t| Keyframe { t_ms: t, jpeg: vec![], hist: vec![] };
        let frames = [k(0), k(1000), k(2000)];
        assert_eq!(pick_keyframe(&frames, 1400).map(|k| k.t_ms), Some(1000));
        assert_eq!(pick_keyframe(&frames, 1500).map(|k| k.t_ms), Some(1000));
        assert!(pick_keyframe(&[], 0).is_none());
    }
}
