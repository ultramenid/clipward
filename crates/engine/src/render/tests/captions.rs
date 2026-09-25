//! Ports of engine/tests/test_captions.py and test_caption_placement.py.
use super::support::*;
use crate::layout::types::{ClipLayoutPlan, ShotLayout, SCREEN, SCREEN_CAM, TALKING_HEAD, TWO_SHOT};
use crate::render::captions::{self, CaptionGenerator, CaptionStyle, CAPTION_PRESETS, MAX_LINGER_MS};
use crate::render::clip_editor::{remap_plan, TimeMap};
use crate::render::layout_renderer::{
    caption_anchor, face_rects, face_zones, shot_chain, shot_views, CaptionPlacer, FaceZone, CAPTION_BOTTOM_LIMIT,
    CAPTION_TOP_LIMIT,
};
use crate::render::service::RenderRequest;
use crate::types::TranscriptWord;
use std::collections::HashSet;

const SRC_W: i64 = 1920;
const SRC_H: i64 = 1080;
const OUT_W: i64 = 1080;
const OUT_H: i64 = 1920;

fn events(style: &CaptionStyle, words: &[TranscriptWord], emphasis: &[&str]) -> Vec<String> {
    let text = words.iter().map(|w| w.word.as_str()).collect::<Vec<_>>().join(" ");
    let seg = segment(words[0].start_time_ms, words[words.len() - 1].end_time_ms, &text, words.to_vec());
    let emphasis: Vec<String> = emphasis.iter().map(|s| s.to_string()).collect();
    let gen = CaptionGenerator::new(style, &emphasis, None);
    let ass = gen
        .build_ass(&[seg], 0, words[words.len() - 1].end_time_ms + 2000, 1080, 1920, None, None, None)
        .expect("captions");
    ass.lines().filter(|l| l.starts_with("Dialogue:")).map(str::to_string).collect()
}

fn face_layer(events: &[String]) -> Vec<String> {
    events.iter().filter(|e| e.starts_with("Dialogue: 4,")).cloned().collect()
}

fn words3() -> Vec<TranscriptWord> {
    vec![word("We", 0, 300), word("made", 300, 600), word("$14,500.", 600, 1200)]
}

#[test]
fn every_preset_renders_layered_events() {
    for preset in CAPTION_PRESETS {
        let ev = events(&captions::get_caption_preset(preset).unwrap(), &words3(), &[]);
        assert!(!ev.is_empty(), "{preset}");
        // The crisp face is always the top layer.
        assert!(ev.iter().any(|e| e.starts_with("Dialogue: 4,")), "{preset}");
    }
}

#[test]
fn hide_mode_hides_future_words_on_the_face() {
    let style = CaptionStyle { future_words: "hide".into(), uppercase: false, ..Default::default() };
    let face = face_layer(&events(&style, &words3(), &[]));
    let before_made = face[0].split("}made").next().unwrap();
    assert!(before_made.rsplit('{').next().unwrap().contains("\\alpha&HFF&"));
    assert!(!face[face.len() - 1].contains("\\alpha&HFF&"));
}

#[test]
fn group_pops_in_once() {
    let face = face_layer(&events(&CaptionStyle::default(), &words3(), &[]));
    assert!(face[0].contains("\\fscx82"));
    assert!(!face[1..].iter().any(|e| e.contains("\\fscx82")));
}

#[test]
fn group_clears_after_linger() {
    let ev = events(&CaptionStyle::default(), &words3(), &[]);
    let last_end = ev.iter().map(|e| CaptionGenerator::parse_ass_time(e.split(',').nth(2).unwrap()).unwrap()).max();
    assert_eq!(last_end, Some(1200 + MAX_LINGER_MS));
}

#[test]
fn trailing_punctuation_dropped() {
    let style = CaptionStyle::default();
    assert_eq!(CaptionGenerator::display_word("business.", &style), "BUSINESS");
    assert_eq!(CaptionGenerator::display_word("really?", &style), "REALLY?");
    assert_eq!(CaptionGenerator::display_word("{bad}", &style), "BAD");
}

#[test]
fn karaoke_sweep_spans_gaps_between_words() {
    let words = vec![word("one", 0, 200), word("two", 500, 700)];
    let face = face_layer(&events(&captions::get_caption_preset("sweep").unwrap(), &words, &[]));
    assert_eq!(face.len(), 1);
    assert!(face[0].contains("\\kf50") && face[0].contains("\\kf20"));
}

#[test]
fn emphasis_words_get_accent_color() {
    let style = captions::get_caption_preset("pop").unwrap();
    let face = face_layer(&events(&style, &words3(), &["14,500"]));
    let accent = "&H008BFF3D"; // #3DFF8B
    // Emphasized before it's spoken and while spoken, never for other words.
    assert!(face.iter().all(|e| e.contains(&format!("\\1c{accent}}}$14,500"))));
    assert!(!face.iter().any(|e| e.contains(&format!("{accent}}}WE")) || e.contains(&format!("{accent}}}MADE"))));
}

#[test]
fn caption_timebase_includes_audio_padding() {
    // Captions are timed against the padded render window and end with the edited output.
    let tmp = tempfile::tempdir().unwrap();
    let r = renderer(FakeRunner::new(|_, _| ok(b"")));
    let (start, duration) = (1000, 2000);
    let (window_start, window_ms) = crate::render::service::compute_padded_range(150, start, duration);
    assert_eq!((window_start, window_ms), (850, 2300));
    let request = RenderRequest {
        output_path: tmp.path().join("out.mp4"),
        start_time_ms: start,
        end_time_ms: start + duration,
        source_width: 1920,
        source_height: 1080,
        transcript_segments: vec![segment(1200, 1600, "hello", vec![word("hello", 1200, 1600)])],
        ..Default::default()
    };
    let p = plan(vec![shot(0, window_ms, SCREEN)], 1920, 1080);
    let path = r
        .generate_captions(&request, 1080, 1920, window_start, &TimeMap::new(vec![(0, window_ms)], None), &p, false, None)
        .unwrap()
        .unwrap();
    let ass = std::fs::read_to_string(path).unwrap();
    let first = ass.lines().find(|l| l.starts_with("Dialogue:")).unwrap();
    // 1200 ms in the source is 350 ms into the window that starts 150 ms early.
    assert!(first.starts_with("Dialogue: 0,0:00:00.35,") || first.contains(",0:00:00.35,"), "{first}");
}

// ---------------------------------------------------------------- placement

fn talking_head(face: crate::layout::types::Box, end_ms: i64) -> ShotLayout {
    ShotLayout {
        focus_path: vec![(0, face.cx(), face.cy())],
        people: vec![face],
        ..shot(0, end_ms, TALKING_HEAD)
    }
}

fn plan_with(s: ShotLayout, faces: Vec<crate::layout::types::Box>) -> ClipLayoutPlan {
    let samples = (s.start_ms..s.end_ms).step_by(250).map(|t| (t, faces.clone())).collect();
    ClipLayoutPlan { face_samples: samples, ..plan(vec![s], SRC_W as u32, SRC_H as u32) }
}

fn placer_for(p: &ClipLayoutPlan) -> CaptionPlacer {
    let tm = TimeMap::new(vec![(0, p.shots[p.shots.len() - 1].end_ms)], None);
    let mut anchors: Vec<(i64, i64, i64)> = p
        .shots
        .iter()
        .map(|s| {
            let (a, y) = caption_anchor(s, SRC_W, SRC_H, OUT_W, OUT_H);
            (s.end_ms, a, y)
        })
        .collect();
    let n = anchors.len() - 1;
    anchors[n].0 = 1_000_000_000;
    CaptionPlacer::new(anchors, face_zones(p, &tm, OUT_W, OUT_H), OUT_W, OUT_H)
}

fn block_span(alignment: i64, y: i64, h: i64) -> (f64, f64) {
    let (y, h) = (y as f64, h as f64);
    let top = match alignment {
        1..=3 => y - h,
        7..=9 => y,
        _ => y - h / 2.0,
    };
    (top, top + h)
}

const BLOCK: (i64, i64) = (830, 135);

#[test]
fn talking_head_face_lands_where_the_crop_puts_it() {
    let face = b(0.40, 0.20, 0.20, 0.35);
    let rects = face_rects(&talking_head(face, 10_000), &[face], 0, SRC_W, SRC_H, OUT_W, OUT_H);
    assert_eq!(rects.len(), 1);
    let rect = rects[0];
    let crop_w = 606.0;
    let crop_x = face.cx() * SRC_W as f64 - crop_w / 2.0;
    let scale = OUT_W as f64 / crop_w;
    assert_eq!(rect.1, (0.20 * OUT_H as f64).round() as i64);
    assert_eq!(rect.3, (0.55 * OUT_H as f64).round() as i64);
    assert_eq!(rect.0, ((0.40 * SRC_W as f64 - crop_x) * scale).round() as i64);
}

#[test]
fn panning_crop_is_followed_over_time() {
    let s = ShotLayout { focus_path: vec![(0, 0.3, 0.4), (2000, 0.7, 0.4)], ..shot(0, 4000, TALKING_HEAD) };
    let face = b(0.45, 0.25, 0.1, 0.2);
    let early = face_rects(&s, &[face], 0, SRC_W, SRC_H, OUT_W, OUT_H)[0];
    let late = face_rects(&s, &[face], 3000, SRC_W, SRC_H, OUT_W, OUT_H)[0];
    assert!(late.0 < early.0);
}

#[test]
fn views_match_the_filter_graph() {
    let two_shot = ShotLayout { people: vec![b(0.2, 0.25, 0.12, 0.25), b(0.68, 0.3, 0.12, 0.25)], ..shot(0, 1000, TWO_SHOT) };
    let small_cam = ShotLayout {
        cam_box: Some(b(0.80, 0.80, 0.12, 0.12)),
        cam_face: Some(b(0.84, 0.82, 0.04, 0.05)),
        screen_box: Some(b(0.0, 0.0, 1.0, 1.0)),
        ..shot(0, 1000, SCREEN_CAM)
    };
    for s in [&two_shot, &small_cam] {
        let chain = shot_chain(0, s, SRC_W, SRC_H, OUT_W, OUT_H, false);
        let views = shot_views(s, 0, SRC_W, SRC_H, OUT_W, OUT_H);
        assert_eq!(views.len(), 2);
        for ((x, y, w, h), _) in views {
            assert!(chain.contains(&format!("crop={w}:{h}:{x}:{y}")), "{chain}");
        }
    }
    let views = shot_views(&small_cam, 0, SRC_W, SRC_H, OUT_W, OUT_H);
    let (dx, dy, _, _) = views[1].1;
    let top_h = views[0].1 .3;
    assert!(shot_chain(0, &small_cam, SRC_W, SRC_H, OUT_W, OUT_H, false).contains(&format!("overlay={dx}:{}", dy - top_h)));
}

#[test]
fn each_two_shot_face_lands_in_its_own_panel() {
    let (left, right) = (b(0.2, 0.25, 0.12, 0.25), b(0.68, 0.3, 0.12, 0.25));
    let s = ShotLayout { people: vec![left, right], ..shot(0, 1000, TWO_SHOT) };
    let rects = face_rects(&s, &[left, right], 0, SRC_W, SRC_H, OUT_W, OUT_H);
    assert_eq!(rects.len(), 2);
    assert!(rects.iter().any(|r| r.3 <= 960) && rects.iter().any(|r| r.1 >= 960));
}

#[test]
fn letterbox_maps_into_the_video_band() {
    let face = b(0.4, 0.3, 0.2, 0.3);
    let rect = face_rects(&shot(0, 1000, SCREEN), &[face], 0, SRC_W, SRC_H, OUT_W, OUT_H)[0];
    assert!(656 <= rect.1 && rect.1 < rect.3 && rect.3 <= 656 + 608);
}

#[test]
fn background_faces_are_ignored() {
    let tiny = b(0.5, 0.5, 0.01, 0.02);
    assert!(face_rects(&talking_head(tiny, 10_000), &[tiny], 0, SRC_W, SRC_H, OUT_W, OUT_H).is_empty());
}

#[test]
fn zones_follow_the_edit() {
    let face = b(0.40, 0.35, 0.24, 0.35);
    let p = ClipLayoutPlan {
        face_samples: vec![(3000, vec![face]), (4100, vec![face])],
        ..plan(vec![talking_head(face, 6000)], SRC_W as u32, SRC_H as u32)
    };
    let zones = face_zones(&p, &TimeMap::new(vec![(0, 2000), (4000, 6000)], Some(6000)), OUT_W, OUT_H);
    assert_eq!(zones.iter().map(|z| (z.start_ms, z.end_ms)).collect::<Vec<_>>(), vec![(2000, 2475)]);
}

#[test]
fn shot_without_detections_uses_its_summary_boxes() {
    let face = b(0.40, 0.35, 0.24, 0.35);
    let p = ClipLayoutPlan {
        face_samples: (0..5000).step_by(250).map(|t| (t, vec![])).collect(),
        ..plan(vec![talking_head(face, 5000)], SRC_W as u32, SRC_H as u32)
    };
    let zones = face_zones(&p, &TimeMap::new(vec![(0, 5000)], None), OUT_W, OUT_H);
    assert_eq!(zones.iter().map(|z| (z.start_ms, z.end_ms)).collect::<Vec<_>>(), vec![(0, 5000)]);
}

#[test]
fn no_faces_means_no_zones() {
    let p = ClipLayoutPlan {
        face_samples: (0..5000).step_by(250).map(|t| (t, vec![])).collect(),
        ..plan(vec![shot(0, 5000, SCREEN)], SRC_W as u32, SRC_H as u32)
    };
    assert!(face_zones(&p, &TimeMap::new(vec![(0, 5000)], None), OUT_W, OUT_H).is_empty());
}

#[test]
fn keeps_the_usual_spot_when_the_face_is_clear() {
    let face = b(0.40, 0.15, 0.20, 0.35);
    let mut placer = placer_for(&plan_with(talking_head(face, 10_000), vec![face]));
    assert_eq!(placer.place(1000, 2500, BLOCK.0, BLOCK.1), (5, 1340));
}

#[test]
fn moves_below_the_chin_of_a_low_close_up() {
    let face = b(0.38, 0.35, 0.24, 0.35);
    let (a, y) = placer_for(&plan_with(talking_head(face, 10_000), vec![face])).place(1000, 2500, BLOCK.0, BLOCK.1);
    let (top, bottom) = block_span(a, y, BLOCK.1);
    assert!(top > 0.70 * OUT_H as f64);
    assert!(bottom <= CAPTION_BOTTOM_LIMIT * OUT_H as f64);
}

#[test]
fn moves_above_a_face_filling_the_lower_frame() {
    let face = b(0.35, 0.40, 0.30, 0.45);
    let (a, y) = placer_for(&plan_with(talking_head(face, 10_000), vec![face])).place(1000, 2500, BLOCK.0, BLOCK.1);
    let (top, bottom) = block_span(a, y, BLOCK.1);
    assert!(bottom < 0.40 * OUT_H as f64);
    assert!(top >= CAPTION_TOP_LIMIT * OUT_H as f64);
}

#[test]
fn never_covers_a_face_that_only_passes_by() {
    let high = b(0.40, 0.10, 0.20, 0.30);
    let lean = b(0.38, 0.35, 0.24, 0.35);
    let p = ClipLayoutPlan {
        face_samples: (0..10_000)
            .step_by(250)
            .map(|t| (t, vec![if (1500..2000).contains(&t) { lean } else { high }]))
            .collect(),
        ..plan(vec![talking_head(high, 10_000)], SRC_W as u32, SRC_H as u32)
    };
    let (a, y) = placer_for(&p).place(1000, 2500, BLOCK.0, BLOCK.1);
    assert!(block_span(a, y, BLOCK.1).0 > 0.70 * OUT_H as f64);
}

#[test]
fn moved_captions_stay_put_within_a_shot() {
    let face = b(0.38, 0.35, 0.24, 0.35);
    let high = b(0.40, 0.10, 0.20, 0.30);
    let p = ClipLayoutPlan {
        face_samples: (0..10_000).step_by(250).map(|t| (t, vec![if t < 3000 { face } else { high }])).collect(),
        ..plan(vec![talking_head(face, 10_000)], SRC_W as u32, SRC_H as u32)
    };
    let mut placer = placer_for(&p);
    let moved = placer.place(1000, 2500, BLOCK.0, BLOCK.1);
    assert_ne!(moved, (5, 1340));
    assert_eq!(placer.place(4000, 5000, BLOCK.0, BLOCK.1), moved);
}

#[test]
fn new_shot_starts_from_its_own_anchor() {
    let close = b(0.38, 0.35, 0.24, 0.35);
    let wide = b(0.40, 0.10, 0.20, 0.30);
    let second = ShotLayout { focus_path: vec![(0, wide.cx(), wide.cy())], people: vec![wide], ..shot(5000, 10_000, TALKING_HEAD) };
    let p = ClipLayoutPlan {
        face_samples: (0..10_000).step_by(250).map(|t| (t, vec![if t < 5000 { close } else { wide }])).collect(),
        ..plan(vec![talking_head(close, 5000), second], SRC_W as u32, SRC_H as u32)
    };
    let mut placer = placer_for(&p);
    assert_ne!(placer.place(1000, 2500, BLOCK.0, BLOCK.1), (5, 1340));
    assert_eq!(placer.place(6000, 7000, BLOCK.0, BLOCK.1), (5, 1340));
}

#[test]
fn no_zones_keeps_every_anchor() {
    let mut placer = CaptionPlacer::new(vec![(1_000_000_000, 2, 1560)], vec![], OUT_W, OUT_H);
    assert_eq!(placer.place(0, 1000, BLOCK.0, BLOCK.1), (2, 1560));
}

#[test]
fn face_beside_a_narrow_caption_does_not_move_it() {
    let zone = FaceZone { start_ms: 0, end_ms: 5000, rects: vec![(0, 1250, 200, 1450)] };
    let mut placer = CaptionPlacer::new(vec![(1_000_000_000, 5, 1340)], vec![zone], OUT_W, OUT_H);
    assert_eq!(placer.place(1000, 2000, 400, 135), (5, 1340));
    assert_ne!(placer.place(3000, 4000, 1000, 135), (5, 1340));
}

fn pos(line: &str) -> Option<(String, String, String)> {
    let re = regex::Regex::new(r"\\an(\d)\\pos\((\d+),(\d+)\)").unwrap();
    re.captures(line).map(|c| (c[1].to_string(), c[2].to_string(), c[3].to_string()))
}

#[test]
fn every_event_of_a_group_shares_one_position() {
    let words = vec![
        word("This", 0, 300), word("is", 300, 500), word("huge.", 500, 900),
        word("Next", 3000, 3300), word("phrase", 3300, 3700), word("here.", 3700, 4000),
    ];
    let seg = segment(0, 4000, "This is huge. Next phrase here.", words);
    let style = captions::get_caption_preset("pop").unwrap();
    let dir = fonts_dir();
    let gen = CaptionGenerator::new(&style, &[], Some(&dir));
    let mut calls = Vec::new();
    let mut placer = |s: i64, e: i64, w: i64, h: i64| {
        calls.push((s, e, w, h));
        if s < 2000 {
            (5, 400)
        } else {
            (5, 1400)
        }
    };
    let ass = gen.build_ass(&[seg], 0, 6000, 1080, 1920, None, Some(&[(1_000_000_000, 5, 1340)]), Some(&mut placer)).unwrap();
    let events: Vec<&str> = ass.lines().filter(|l| l.starts_with("Dialogue:")).collect();
    assert_eq!(calls.len(), 2); // once per word group, not per word or layer
    assert!(calls[0].0 == 0 && calls[0].1 <= 3000 && calls[1].0 == 3000);
    assert!(calls.iter().all(|c| c.2 > 0 && c.3 > 0));
    assert!(events.len() > 6);
    let mut early = HashSet::new();
    let mut late = HashSet::new();
    for e in &events {
        let p = pos(e).expect("pinned");
        if e.split(',').nth(1).unwrap() < "0:00:02.00" {
            early.insert(p);
        } else {
            late.insert(p);
        }
    }
    assert_eq!(early, HashSet::from([("5".into(), "540".into(), "400".into())]));
    assert_eq!(late, HashSet::from([("5".into(), "540".into(), "1400".into())]));
}

#[test]
fn long_words_are_sized_as_two_lines() {
    let style = captions::get_caption_preset("pop").unwrap();
    let dir = fonts_dir();
    let gen = CaptionGenerator::new(&style, &[], Some(&dir));
    let s = |w: &[&str]| w.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let (_, short_h) = gen.block_size(&s(&["We", "made", "it"]), OUT_W);
    let (width, long_h) = gen.block_size(&s(&["INCREDIBLE", "INTERNATIONAL", "OPPORTUNITIES"]), OUT_W);
    assert!(long_h as f64 > short_h as f64 + style.font_size as f64 * 0.9);
    assert!(width <= OUT_W);
    // Without the font files the width falls back to an estimate that still wraps.
    let fallback = CaptionGenerator::new(&style, &[], None);
    let (_, fallback_h) = fallback.block_size(&s(&["INCREDIBLE", "INTERNATIONAL", "OPPORTUNITIES"]), OUT_W);
    assert!(fallback_h > short_h);
}

fn integration_positions(p: &ClipLayoutPlan) -> HashSet<(String, String, String)> {
    let tmp = tempfile::tempdir().unwrap();
    let r = renderer(FakeRunner::new(|_, _| ok(b"")));
    let words = vec![word("Look", 1000, 1300), word("at", 1300, 1500), word("this.", 1500, 1900)];
    let request = RenderRequest {
        output_path: tmp.path().join("out.mp4"),
        start_time_ms: 0,
        end_time_ms: 10_000,
        source_width: SRC_W,
        source_height: SRC_H,
        apply_padding: false,
        transcript_segments: vec![segment(1000, 1900, "Look at this.", words)],
        ..Default::default()
    };
    let tm = TimeMap::new(vec![(0, 10_000)], None);
    let path = r
        .generate_captions(&request, OUT_W, OUT_H, 0, &tm, &remap_plan(p, &tm), false, Some(p))
        .unwrap()
        .unwrap();
    std::fs::read_to_string(path).unwrap().lines().filter(|l| l.starts_with("Dialogue:")).filter_map(pos).collect()
}

#[test]
fn captions_move_off_a_low_face() {
    let face = b(0.38, 0.35, 0.24, 0.35);
    let positions = integration_positions(&plan_with(talking_head(face, 10_000), vec![face]));
    assert_eq!(positions.len(), 1);
    assert!(!positions.contains(&("5".into(), "540".into(), "1340".into())));
}

#[test]
fn captions_unchanged_without_faces() {
    let p = plan(vec![shot(0, 10_000, SCREEN)], SRC_W as u32, SRC_H as u32);
    assert_eq!(integration_positions(&p), HashSet::from([("2".into(), "540".into(), "1560".into())]));
}
