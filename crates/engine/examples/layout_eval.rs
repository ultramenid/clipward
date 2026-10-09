//! Scores Full frame framing against hand-labelled real videos (layout_eval.json).
//! `cargo run --release -p clip_engine --example layout_eval -- [cases.json] [name filter]`
//! STYLE=auto scores Smart framing instead (it cuts to whoever talks in a group).
//!
//! Per label: scene (did the analyzer see what the shot is), view (is at least one
//! person in the 9:16 crop, when there are people), speaker (is the labelled
//! speaker in the crop; "cut to" counts the crops that close in on them rather
//! than show the whole frame). Videos are local files; missing ones are skipped.
use clip_engine::layout::{analyze_layout, LayoutRequest, ShotLayout, MEDIA_SOURCE};
use clip_engine::render::layout_renderer::PERSON_CROP_FACE_MULT;
use clip_engine::types::EngineConfig;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use tokio_util::sync::CancellationToken;

const PANEL_ASPECT: f64 = 1080.0 / 960.0;

/// The source x-ranges the 9:16 render shows for this shot at `t` (window ms).
fn visible(shot: &ShotLayout, t: i64, src_aspect: f64) -> Vec<(f64, f64)> {
    match shot.layout.as_str() {
        "talking_head" => {
            let w = (9.0 / 16.0 / src_aspect).min(1.0);
            let rel = t - shot.start_ms;
            let cx = shot.focus_path.iter().take_while(|k| k.0 <= rel).last().or(shot.focus_path.first()).map_or(0.5, |k| k.1);
            let cx = cx.clamp(w / 2.0, 1.0 - w / 2.0);
            vec![(cx - w / 2.0, cx + w / 2.0)]
        }
        "two_shot" => shot
            .people
            .iter()
            .map(|p| {
                let half = (PERSON_CROP_FACE_MULT * p.h).min(1.0) * PANEL_ASPECT / src_aspect / 2.0;
                (p.cx() - half, p.cx() + half)
            })
            .collect(),
        _ => vec![(0.0, 1.0)],
    }
}

/// What the analyzer thought the shot was, in the label vocabulary.
fn scene(shot: &ShotLayout) -> String {
    if shot.source == MEDIA_SOURCE {
        return "media".into();
    }
    match shot.detected_layout.as_deref().unwrap_or(&shot.layout) {
        "screen" if !shot.people.is_empty() => "group".into(),
        other => other.into(),
    }
}

fn probe(cfg: &EngineConfig, video: &PathBuf) -> Option<(u32, u32)> {
    let out = Command::new(&cfg.ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(video)
        .output()
        .ok()?;
    let dims: Vec<u32> = String::from_utf8_lossy(&out.stdout).trim().split(',').filter_map(|s| s.parse().ok()).collect();
    (dims.len() == 2).then(|| (dims[0], dims[1]))
}

#[tokio::main]
async fn main() {
    let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let args: Vec<String> = std::env::args().collect();
    let manifest = args.get(1).map(PathBuf::from).unwrap_or(engine.join("examples/layout_eval.json"));
    let filter = args.get(2).cloned().unwrap_or_default();
    let cfg = EngineConfig {
        ffmpeg: engine.join("../../engine-bin/ffmpeg"),
        ffprobe: engine.join("../../engine-bin/ffprobe"),
        ytdlp: None,
        assets_dir: engine.join("assets"),
        whisper_model_path: PathBuf::new(),
        ai: Default::default(),
        custom_vocabulary: vec![],
        work_dir: std::env::temp_dir(),
        output_dir: std::env::temp_dir(),
    };
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
    let home = std::env::var("HOME").unwrap_or_default();
    let style = std::env::var("STYLE").unwrap_or("fill".into());
    let (mut total, mut scene_ok, mut view_n, mut view_ok, mut spk_n, mut spk_ok, mut spk_cut) = (0, 0, 0, 0, 0, 0, 0);
    for case in cases["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        if !name.contains(filter.as_str()) {
            continue;
        }
        let video = engine.join("../..").join(case["video"].as_str().unwrap().replacen('~', &home, 1));
        let Some((src_w, src_h)) = probe(&cfg, &video) else {
            println!("SKIP {name}: {} not found", video.display());
            continue;
        };
        let labels = case["labels"].as_array().unwrap();
        let last = labels.iter().map(|l| l[0].as_f64().unwrap()).fold(0.0, f64::max);
        let req = LayoutRequest {
            video_path: video,
            start_ms: (case["start"].as_f64().unwrap() * 1000.0) as i64,
            duration_ms: ((last + 2.0) * 1000.0) as i64,
            src_w,
            src_h,
            style: style.clone(),
            vision: false,
            vision_model: None,
        };
        let plan = analyze_layout(&cfg, &req, None, &CancellationToken::new()).await.unwrap().unwrap();
        let aspect = src_w as f64 / src_h as f64;
        let (mut c_total, mut c_ok) = (0, 0);
        let mut misses = Vec::new();
        for l in labels {
            let t = (l[0].as_f64().unwrap() * 1000.0) as i64;
            let want = l[1].as_str().unwrap();
            let people: Vec<f64> = l[2].as_array().unwrap().iter().filter_map(Value::as_f64).collect();
            let speaker = l.get(3).and_then(Value::as_f64);
            let Some(shot) = plan.shots.iter().find(|s| s.start_ms <= t && t < s.end_ms).or(plan.shots.last()) else { continue };
            let view = visible(shot, t, aspect);
            let seen = |x: f64| view.iter().any(|(a, b)| (*a..=*b).contains(&x));
            let got = scene(shot);
            let s_ok = got == want;
            let v_ok = people.is_empty() || people.iter().any(|x| seen(*x));
            let k_ok = speaker.is_none_or(seen);
            total += 1;
            c_total += 1;
            scene_ok += s_ok as usize;
            if !people.is_empty() {
                view_n += 1;
                view_ok += v_ok as usize;
            }
            if speaker.is_some() {
                spk_n += 1;
                spk_ok += k_ok as usize;
                spk_cut += (k_ok && view != [(0.0, 1.0)]) as usize;
            }
            if s_ok && v_ok && k_ok {
                c_ok += 1;
            } else {
                let shown: Vec<String> = view.iter().map(|(a, b)| format!("{a:.2}-{b:.2}")).collect();
                misses.push(format!(
                    "    +{:>2}s want {want} got {got}->{} view [{}]{}{}",
                    t / 1000,
                    shot.layout,
                    shown.join(" "),
                    if v_ok { "" } else { "  NOBODY IN VIEW" },
                    if k_ok { "" } else { "  SPEAKER OUT" },
                ));
            }
        }
        println!("{c_ok:>2}/{c_total:<2} {name}");
        misses.iter().for_each(|m| println!("{m}"));
    }
    let pct = |a: usize, b: usize| if b == 0 { 100.0 } else { a as f64 * 100.0 / b as f64 };
    println!(
        "\n{style}: scene {:.0}% ({scene_ok}/{total})  person in view {:.0}% ({view_ok}/{view_n})  speaker in view {:.0}% ({spk_ok}/{spk_n}, cut to {spk_cut})",
        pct(scene_ok, total),
        pct(view_ok, view_n),
        pct(spk_ok, spk_n)
    );
}
