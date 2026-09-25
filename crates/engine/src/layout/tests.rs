//! Ports of engine/tests/test_layout.py (analyzer parts) and
//! test_framing_quality.py (classification golden cases). Synthetic data only;
//! the FFmpeg end-to-end test skips when ffmpeg or the model is missing.
use super::analysis::*;
use super::imgproc::{normalize_l2, HIST_LEN, HIST_S_BINS};
use super::*;
use serde_json::json;

const SRC_W: u32 = 1920;
const SRC_H: u32 = 1080;

fn b(x: f64, y: f64, w: f64, h: f64) -> Box {
    Box { x, y, w, h }
}

/// A normalized 2D histogram; different seeds look like different shots.
fn hist(seed: usize) -> Vec<f32> {
    let mut h = vec![0f32; HIST_LEN];
    h[(seed % 24) * HIST_S_BINS + seed % 16] = 1.0;
    h[((seed * 7) % 24) * HIST_S_BINS + (seed * 3) % 16] = 0.5;
    normalize_l2(&mut h);
    h
}

fn frames_with(duration_ms: i64, faces_at: impl Fn(i64) -> Vec<Box>, hist_at: impl Fn(i64) -> Vec<f32>) -> Vec<FrameInfo> {
    (0..duration_ms).step_by(250).map(|t| FrameInfo { t_ms: t, faces: faces_at(t), hist: hist_at(t) }).collect()
}

fn frames(duration_ms: i64, faces_at: impl Fn(i64) -> Vec<Box>) -> Vec<FrameInfo> {
    frames_with(duration_ms, faces_at, |_| hist(1))
}

const HEAD: Box = Box { x: 0.43, y: 0.2, w: 0.14, h: 0.3 }; // on-camera speaker
const CAM_FACE: Box = Box { x: 0.84, y: 0.80, w: 0.05, h: 0.09 }; // webcam face, bottom-right
const CAM_FACE_SMALL: Box = Box { x: 0.88, y: 0.86, w: 0.03, h: 0.055 }; // same webcam after shrinking

fn approx(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * b.abs().max(1e-12)
}

// -- shot segmentation -------------------------------------------------------

#[test]
fn detects_cut_and_merges_blips() {
    // Cut at ~4s; a 0.5s flash at ~8s is too short to be its own shot.
    let hist_at = |t: i64| {
        if (8000..8500).contains(&t) {
            hist(9)
        } else if t < 4000 {
            hist(1)
        } else {
            hist(5)
        }
    };
    let shots = segment_shots(&frames_with(12000, |_| vec![], hist_at), 12000);
    assert_eq!(shots[0].0, 0);
    assert_eq!(shots.last().unwrap().1, 12000);
    assert!(shots[1..].iter().any(|(s, _)| (s - 4000).abs() <= 250));
    assert!(shots.iter().all(|(s, e)| e - s >= 1200));
}

#[test]
fn single_shot() {
    assert_eq!(segment_shots(&frames(5000, |_| vec![]), 5000), vec![(0, 5000)]);
    assert_eq!(segment_shots(&[], 5000), vec![(0, 5000)]);
}

// -- classification ----------------------------------------------------------

fn classify(faces: Vec<Box>) -> (ShotLayout, Option<FaceTrack>) {
    let fr = frames(4000, |_| faces.clone());
    classify_shot(&track_faces(&fr), fr.len(), SRC_W, SRC_H)
}

#[test]
fn talking_head() {
    let (shot, track) = classify(vec![HEAD]);
    assert_eq!(shot.layout, TALKING_HEAD);
    assert_eq!(shot.source, "heuristic");
    assert!(track.is_some());
}

#[test]
fn two_shot_orders_left_to_right() {
    let (shot, _) = classify(vec![b(0.65, 0.25, 0.12, 0.25), b(0.2, 0.25, 0.12, 0.25)]);
    assert_eq!(shot.layout, TWO_SHOT);
    assert!(shot.people[0].cx() < shot.people[1].cx());
}

#[test]
fn screen_with_webcam() {
    let (shot, _) = classify(vec![CAM_FACE]);
    assert_eq!(shot.layout, SCREEN_CAM);
    assert!(shot.cam_box.unwrap().contains(CAM_FACE.cx(), CAM_FACE.cy()));
}

#[test]
fn no_faces_is_screen() {
    assert_eq!(classify(vec![]).0.layout, SCREEN);
}

#[test]
fn crowd_falls_back_to_screen() {
    let crowd = (0..3).map(|i| b(0.1 + 0.3 * i as f64, 0.3, 0.1, 0.2)).collect();
    assert_eq!(classify(crowd).0.layout, SCREEN);
}

#[test]
fn moving_webcam_still_counts_as_overlay() {
    // Webcam jumps between two corner positions: two short tracks.
    let fr = frames(4000, |t| vec![if (t / 1000) % 2 == 1 { CAM_FACE } else { b(0.08, 0.08, 0.05, 0.09) }]);
    let (shot, _) = classify_shot(&track_faces(&fr), fr.len(), SRC_W, SRC_H);
    assert_eq!(shot.layout, SCREEN_CAM);
}

// -- webcam following ----------------------------------------------------------

#[test]
fn splits_when_overlay_resizes() {
    let fr = frames(8000, |t| vec![if t < 4000 { CAM_FACE } else { CAM_FACE_SMALL }]);
    let segments = split_overlay_segments(&fr, 0, 8000, CAM_FACE, None);
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].0, 0);
    assert_eq!(segments.last().unwrap().1, 8000);
    assert!((segments[1].0 - 4000).abs() <= 250);
    assert!(segments[1].2.unwrap().h < segments[0].2.unwrap().h);
}

#[test]
fn long_dropout_becomes_no_webcam_segment() {
    let fr = frames(9000, |t| if (3000..6000).contains(&t) { vec![] } else { vec![CAM_FACE] });
    let segments = split_overlay_segments(&fr, 0, 9000, CAM_FACE, None);
    assert_eq!(segments.iter().map(|s| s.2.is_none()).collect::<Vec<_>>(), vec![false, true, false]);
}

#[test]
fn brief_dropout_is_ignored() {
    let fr = frames(6000, |t| if (2000..2500).contains(&t) { vec![] } else { vec![CAM_FACE] });
    assert_eq!(split_overlay_segments(&fr, 0, 6000, CAM_FACE, None).len(), 1);
}

#[test]
fn large_face_inside_vision_cam_box_counts() {
    // A close webcam: too big for the overlay heuristic, but inside the vision webcam box.
    let cam = b(0.62, 0.55, 0.36, 0.43);
    let big = b(0.74, 0.62, 0.12, 0.25);
    let fr = frames(6000, |_| vec![big]);
    assert!(split_overlay_segments(&fr, 0, 6000, big, None)[0].2.is_none());
    let segments = split_overlay_segments(&fr, 0, 6000, big, Some(cam));
    assert_eq!(segments.len(), 1);
    assert!(segments[0].2.is_some());
}

#[test]
fn vision_screen_cam_survives_missed_face() {
    let cam = b(0.62, 0.55, 0.36, 0.43);
    let shot = ShotLayout {
        source: "vision".into(),
        cam_box: Some(cam),
        screen_box: Some(b(0.0, 0.0, 1.0, 1.0)),
        ..new_shot(0, 6000, SCREEN_CAM)
    };
    let subs = follow_webcam(&shot, &frames(6000, |_| vec![]), SRC_W, SRC_H);
    assert_eq!(subs.iter().map(|s| s.layout.as_str()).collect::<Vec<_>>(), vec![SCREEN_CAM]);
    assert_eq!(subs[0].cam_box, Some(cam));
}

#[test]
fn heuristic_screen_cam_without_face_is_screen() {
    let shot = ShotLayout {
        cam_box: Some(estimate_cam_box(&CAM_FACE, SRC_W, SRC_H)),
        cam_face: Some(CAM_FACE),
        ..new_shot(0, 6000, SCREEN_CAM)
    };
    let subs = follow_webcam(&shot, &frames(6000, |_| vec![]), SRC_W, SRC_H);
    assert_eq!(subs.iter().map(|s| s.layout.as_str()).collect::<Vec<_>>(), vec![SCREEN]);
}

#[test]
fn estimated_cam_box_covers_the_overlay() {
    // Face of a real stream's rounded bottom-right webcam (overlay ~0.33 x 0.35).
    let face = b(0.8185, 0.7976, 0.047, 0.084);
    let cam = estimate_cam_box(&face, SRC_W, SRC_H);
    assert!(cam.contains(face.cx(), face.cy()));
    assert!(cam.x >= 0.5 && cam.y >= 0.5);
    assert!(cam.x + cam.w <= 1.0 && cam.y + cam.h <= 1.0);
    assert!(cam.h >= 0.28 && cam.area() >= 2.5 * 0.168 * 0.168);
    assert!(cam.w <= 0.4 && cam.h <= 0.45);
}

#[test]
fn estimated_cam_box_never_crosses_center() {
    let face = b(0.66, 0.6, 0.06, 0.16); // big face near the middle
    let cam = estimate_cam_box(&face, SRC_W, SRC_H);
    assert!(cam.x >= 0.5 - 1e-9 && cam.y >= 0.5 - 1e-9);
    assert!(cam.x <= face.x && cam.x + cam.w >= face.x + face.w);
}

#[test]
fn transfer_cam_box_scales_with_face() {
    let ref_cam = b(0.75, 0.72, 0.22, 0.22);
    let moved = transfer_cam_box(&ref_cam, &CAM_FACE, &CAM_FACE_SMALL);
    let scale = CAM_FACE_SMALL.h / CAM_FACE.h;
    assert!(approx(moved.h, ref_cam.h * scale, 1e-3));
    assert!(moved.contains(CAM_FACE_SMALL.cx(), CAM_FACE_SMALL.cy()));
}

// -- vision merge --------------------------------------------------------------

#[test]
fn screen_cam_uses_vision_boxes() {
    let heuristic = ShotLayout { cam_face: Some(CAM_FACE), ..new_shot(0, 0, SCREEN_CAM) };
    let merged = merge_vision_result(
        &heuristic,
        &json!({
            "layout": "screen_cam",
            "cam_box": [720, 750, 950, 980],
            "screen_box": [0, 0, 1000, 1000],
            "screen_focus": [200, 200, 550, 700],
            "people": [],
        }),
    );
    assert_eq!(merged.source, "vision");
    assert!(approx(merged.cam_box.unwrap().x, 0.75, 1e-6));
    assert!(merged.screen_focus.is_some());
    assert_eq!(merged.cam_face, Some(CAM_FACE));
}

#[test]
fn screen_cam_without_any_cam_box_degrades_to_screen() {
    let merged = merge_vision_result(
        &new_shot(0, 0, SCREEN),
        &json!({"layout": "screen_cam", "cam_box": [], "screen_box": [], "screen_focus": [], "people": []}),
    );
    assert_eq!(merged.layout, SCREEN);
}

#[test]
fn invalid_layout_keeps_heuristic() {
    let heuristic = ShotLayout { people: vec![HEAD], ..new_shot(0, 0, TALKING_HEAD) };
    let merged = merge_vision_result(&heuristic, &json!({"layout": "banana"}));
    assert_eq!(merged.layout, TALKING_HEAD);
    assert_eq!(merged.source, "heuristic");
    assert_eq!(merged.people, heuristic.people);
    assert_eq!(merge_vision_result(&heuristic, &json!([1, 2])).source, "heuristic");
}

#[test]
fn two_shot_from_vision_people() {
    let merged = merge_vision_result(
        &new_shot(0, 0, SCREEN),
        &json!({
            "layout": "two_shot", "cam_box": [], "screen_box": [], "screen_focus": [],
            "people": [[200, 550, 900, 900], [200, 100, 900, 450]],
        }),
    );
    assert_eq!(merged.layout, TWO_SHOT);
    assert!(merged.people[0].cx() < merged.people[1].cx());
    // Person boxes are converted to face-sized boxes for crop sizing.
    assert!(merged.people[0].h < 0.7 * 0.5);
}

#[test]
fn box_from_1000_rejects_malformed() {
    assert!(box_from_1000(Some(&json!([1, 2, 3]))).is_none());
    assert!(box_from_1000(Some(&json!([0, 0, 10, 10]))).is_none()); // < 2% wide
    assert!(box_from_1000(Some(&json!([0, 0, "x", 500]))).is_none());
    assert!(box_from_1000(None).is_none());
    let clamped = box_from_1000(Some(&json!([-50, 100.7, 1500, "600"]))).unwrap();
    assert_eq!(clamped.to_list(), [0.1, 0.0, 0.5, 1.0]);
}

// -- focus path ------------------------------------------------------------------

#[test]
fn still_subject_holds_camera() {
    let samples: Vec<(i64, Box)> = (0..10000).step_by(250).map(|t| (t, HEAD)).collect();
    let path = smooth_focus_path(&samples, 10000, 0.316);
    let first = path[0].1;
    assert!(path.iter().all(|p| (p.1 - first).abs() < 5e-5));
}

#[test]
fn moving_subject_pans_with_capped_speed() {
    let samples: Vec<(i64, Box)> =
        (0..10000).step_by(250).map(|t| (t, b(0.2 + 0.5 * t as f64 / 10000.0, 0.2, 0.14, 0.3))).collect();
    let path = smooth_focus_path(&samples, 10000, 0.316);
    assert!(path.last().unwrap().1 > path[0].1 + 0.2);
    for w in path.windows(2) {
        let ((t0, x0, _), (t1, x1, _)) = (w[0], w[1]);
        assert!((x1 - x0).abs() / ((t1 - t0) as f64 / 1000.0).max(1e-3) <= 0.9 * 0.316 + 1e-6);
    }
    assert!(path.len() <= MAX_PATH_KEYFRAMES + 1);
}

// -- styles ----------------------------------------------------------------------

#[test]
fn fill_turns_split_layouts_into_full_frame() {
    let shot = apply_style(ShotLayout { people: vec![HEAD, HEAD], ..new_shot(0, 1000, TWO_SHOT) }, STYLE_FILL);
    assert_eq!(shot.layout, TALKING_HEAD);
    assert_eq!(shot.detected_layout.as_deref(), Some(TWO_SHOT));
    assert!(!shot.focus_path.is_empty());
}

#[test]
fn fit_forces_letterbox() {
    let shot = apply_style(new_shot(0, 1000, TALKING_HEAD), STYLE_FIT);
    assert_eq!(shot.layout, SCREEN);
    assert_eq!(shot.source, "style");
}

// -- shot post-processing / plumbing ---------------------------------------------

#[test]
fn merge_adjacent_joins_static_shots_only() {
    let cam = b(0.75, 0.72, 0.22, 0.22);
    let shots = vec![
        new_shot(0, 1000, SCREEN),
        new_shot(1000, 2000, SCREEN),
        ShotLayout { cam_box: Some(cam), ..new_shot(2000, 3000, SCREEN_CAM) },
        ShotLayout { cam_box: Some(b(0.76, 0.72, 0.22, 0.23)), ..new_shot(3000, 4000, SCREEN_CAM) },
        ShotLayout { cam_box: Some(b(0.05, 0.05, 0.2, 0.2)), ..new_shot(4000, 5000, SCREEN_CAM) },
        new_shot(5000, 6000, TALKING_HEAD),
        new_shot(6000, 7000, TALKING_HEAD),
    ];
    let merged = merge_adjacent(shots);
    let spans: Vec<(i64, i64)> = merged.iter().map(|s| (s.start_ms, s.end_ms)).collect();
    assert_eq!(spans, vec![(0, 2000), (2000, 4000), (4000, 5000), (5000, 6000), (6000, 7000)]);
}

#[test]
fn analysis_dimensions_match_python() {
    assert_eq!(analysis_dimensions(1920, 1080), (640, 360));
    assert_eq!(analysis_dimensions(1080, 1920), (640, 1138));
    assert_eq!(analysis_dimensions(640, 4000), (204, 1280));
    assert_eq!(analysis_dimensions(3, 3), (640, 640));
    assert_eq!(MAX_ANALYSIS_FRAMES, 14401);
}

#[test]
fn decode_args_whitelist_inputs() {
    let args = decode_args(Path::new("/v/in.mp4"), 1500, 30000, 640, 360);
    let joined = args.join(" ");
    assert!(joined.starts_with("-nostdin -v error -ss 1.500 -protocol_whitelist file,pipe,fd -format_whitelist"));
    assert!(joined.contains("-i /v/in.mp4 -t 30.000 -vf fps=4.0,scale=640:360 -f rawvideo -pix_fmt bgr24 -"));
}

fn test_config(assets: &Path) -> EngineConfig {
    EngineConfig {
        ffmpeg: which::which("ffmpeg").unwrap_or_else(|_| PathBuf::from("ffmpeg")),
        ffprobe: PathBuf::from("ffprobe"),
        ytdlp: None,
        assets_dir: assets.to_path_buf(),
        whisper_model_path: PathBuf::new(),
        ai: Default::default(),
        custom_vocabulary: vec![],
        work_dir: std::env::temp_dir(),
        output_dir: std::env::temp_dir(),
    }
}

fn request(path: PathBuf, style: &str) -> LayoutRequest {
    LayoutRequest {
        video_path: path,
        start_ms: 0,
        duration_ms: 3000,
        src_w: 640,
        src_h: 360,
        style: style.into(),
        vision: false,
        vision_model: None,
    }
}

#[tokio::test]
async fn fit_style_and_missing_model_skip_analysis() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    let cancel = CancellationToken::new();
    let plan = analyze_layout(&cfg, &request("x.mp4".into(), STYLE_FIT), None, &cancel).await.unwrap().unwrap();
    assert_eq!(plan.shots.len(), 1);
    assert_eq!(plan.shots[0].source, "style");
    assert!(plan.is_letterbox_only());
    assert!(!layout_available(&cfg));
    assert!(analyze_layout(&cfg, &request("x.mp4".into(), STYLE_AUTO), None, &cancel).await.unwrap().is_none());
}

fn repo_assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")
}

#[tokio::test]
async fn ffmpeg_end_to_end_on_synthetic_video() {
    let assets = repo_assets();
    let Ok(ffmpeg) = which::which("ffmpeg") else {
        eprintln!("ffmpeg not installed; skipping");
        return;
    };
    if !assets.join("models").join(YUNET_MODEL_FILE).is_file() {
        eprintln!("YuNet model missing; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("src.mkv");
    let status = std::process::Command::new(&ffmpeg)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=12:duration=3"])
        .arg(&video)
        .status()
        .unwrap();
    assert!(status.success());
    let mut cfg = test_config(&assets);
    cfg.ffmpeg = ffmpeg;
    let plan = analyze_layout(&cfg, &request(video, STYLE_AUTO), None, &CancellationToken::new())
        .await
        .unwrap()
        .expect("plan");
    assert!((12..=13).contains(&plan.face_samples.len()));
    assert_eq!(plan.shots.first().unwrap().start_ms, 0);
    assert_eq!(plan.shots.last().unwrap().end_ms, 3000);
}

// -- framing quality golden cases (analyzer side) ----------------------------------

const DURATION_MS: i64 = 3000;

struct GoldenCase {
    name: &'static str,
    width: u32,
    height: u32,
    faces: Vec<Box>,
    expected_layout: &'static str,
    vision_result: Option<serde_json::Value>,
}

fn golden_cases() -> Vec<GoldenCase> {
    vec![
        GoldenCase { name: "talking_head", width: 1920, height: 1080, faces: vec![b(0.43, 0.20, 0.14, 0.30)], expected_layout: TALKING_HEAD, vision_result: None },
        GoldenCase {
            name: "two_shot", width: 1920, height: 1080,
            faces: vec![b(0.20, 0.25, 0.12, 0.25), b(0.68, 0.25, 0.12, 0.25)],
            expected_layout: TWO_SHOT, vision_result: None,
        },
        GoldenCase { name: "corner_cam", width: 1920, height: 1080, faces: vec![b(0.84, 0.80, 0.05, 0.09)], expected_layout: SCREEN_CAM, vision_result: None },
        GoldenCase {
            name: "bottom_center_cam", width: 1920, height: 1080, faces: vec![b(0.47, 0.78, 0.06, 0.09)],
            expected_layout: SCREEN_CAM,
            // A centered overlay is ambiguous from face position alone; a deterministic
            // stand-in for the vision model's full overlay rectangle.
            vision_result: Some(json!({
                "layout": "screen_cam", "cam_box": [700, 300, 980, 700],
                "screen_box": [0, 0, 1000, 1000], "screen_focus": [], "people": [],
            })),
        },
        GoldenCase { name: "already_vertical", width: 1080, height: 1920, faces: vec![b(0.43, 0.20, 0.14, 0.24)], expected_layout: TALKING_HEAD, vision_result: None },
    ]
}

/// Face tracking, classification, vision merge and focus path, as the analyzer runs them.
fn plan_case(case: &GoldenCase) -> ShotLayout {
    let fr: Vec<FrameInfo> =
        (0..DURATION_MS).step_by(250).map(|t| FrameInfo { t_ms: t, faces: case.faces.clone(), hist: vec![] }).collect();
    let (mut shot, main_track) = classify_shot(&track_faces(&fr), fr.len(), case.width, case.height);
    if let Some(track) = main_track {
        let crop_w_frac = f64::min(1.0, (case.height as f64 * 9.0 / 16.0) / case.width as f64);
        shot.focus_path = smooth_focus_path(&track.samples, DURATION_MS, crop_w_frac);
    }
    if let Some(result) = &case.vision_result {
        shot = merge_vision_result(&shot, result);
    }
    shot.start_ms = 0;
    shot.end_ms = DURATION_MS;
    shot
}

#[test]
fn golden_auto_framing() {
    for case in golden_cases() {
        let shot = plan_case(&case);
        assert_eq!(shot.layout, case.expected_layout, "{}", case.name);
        match shot.layout.as_str() {
            TALKING_HEAD => {
                let face = case.faces[0];
                let (_, cx, cy) = shot.focus_path[0];
                assert!((cx - face.cx()).abs() < 1e-9 && (cy - face.cy()).abs() < 1e-9, "{}", case.name);
                // A still subject yields a single static focus.
                assert!(shot.focus_path.iter().all(|p| (p.1 - cx).abs() < 1e-9), "{}", case.name);
            }
            TWO_SHOT => {
                assert_eq!(shot.people.len(), 2);
                assert!(shot.people[0].cx() < shot.people[1].cx());
            }
            SCREEN_CAM => {
                let cam = shot.cam_box.expect("cam box");
                assert!(cam.contains(case.faces[0].cx(), case.faces[0].cy()), "{}", case.name);
                assert!(shot.screen_box.is_some());
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn golden_set_fallback_rate() {
    assert!(golden_cases().iter().all(|c| plan_case(c).layout != SCREEN));
}

#[test]
fn no_face_input_uses_safe_screen_fallback() {
    let case =
        GoldenCase { name: "no_faces", width: 1920, height: 1080, faces: vec![], expected_layout: SCREEN, vision_result: None };
    assert_eq!(plan_case(&case).layout, case.expected_layout);
}
