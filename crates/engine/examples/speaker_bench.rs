//! Who the framing follows, from real video.
//! `cargo run --release -p clip_engine --example speaker_bench` scores the talk
//! signal against the speaker labels in layout_eval.json (optional name filter);
//! `... -- <video> <start_ms> <duration_ms>` prints one window's shots and turns
//! (SERIES=1 adds every mouth reading).
use clip_engine::layout::analysis::{
    classify_shot, follow_speaker, median, segment_shots, track_faces, FaceTrack, FrameInfo, ANALYSIS_FPS,
    FACE_NMS_THRESHOLD, FACE_SCORE_THRESHOLD, FACE_TOP_K, MIN_TALK_SAMPLES,
};
use clip_engine::layout::yunet::YuNet;
use clip_engine::layout::{analysis_dimensions, decode_args, decode_scale, frame_info};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The window's analyzed frames, as the analyzer sees them, and the source size.
fn analyze(video: &Path, start: i64, dur: i64) -> Option<(Vec<FrameInfo>, (u32, u32))> {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bin = engine.join("../../engine-bin");
    let probe = Command::new(bin.join("ffprobe"))
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(video)
        .output()
        .ok()?;
    let dims: Vec<u32> = String::from_utf8_lossy(&probe.stdout).trim().split(',').filter_map(|s| s.parse().ok()).collect();
    let &[src_w, src_h] = dims.as_slice() else { return None };
    let (w, h) = analysis_dimensions(src_w, src_h);
    let scale = decode_scale(src_w, src_h);
    let mut child = Command::new(bin.join("ffmpeg"))
        .args(decode_args(video, start, dur, w * scale, h * scale))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut raw = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut raw).unwrap();
    child.wait().unwrap();
    let model = engine.join("assets/models/face_detection_yunet_2023mar.onnx");
    let det = YuNet::new(&model, w as usize, h as usize, FACE_SCORE_THRESHOLD, FACE_NMS_THRESHOLD, FACE_TOP_K).unwrap();
    let frames = raw
        .chunks_exact((w * scale * h * scale * 3) as usize)
        .enumerate()
        .map(|(i, img)| frame_info(&det, img, w, h, scale, (i as f64 * 1000.0 / ANALYSIS_FPS) as i64).unwrap())
        .collect();
    Some((frames, (src_w, src_h)))
}

type Shot = ((i64, i64), Vec<FrameInfo>, Vec<FaceTrack>);

/// Each shot's frames and its tracks seen in a third of them, left to right.
fn shots(frames: &[FrameInfo], dur: i64) -> Vec<Shot> {
    segment_shots(frames, dur)
        .into_iter()
        .map(|(s, e)| {
            let shot: Vec<FrameInfo> = frames.iter().filter(|f| s <= f.t_ms && f.t_ms < e).cloned().collect();
            let mut tracks: Vec<FaceTrack> = track_faces(&shot).into_iter().filter(|t| t.samples.len() * 3 >= shot.len()).collect();
            tracks.sort_by(|a, b| a.median_box().cx().total_cmp(&b.median_box().cx()));
            ((s, e), shot, tracks)
        })
        .collect()
}

fn print_window(video: &Path, start: i64, dur: i64) {
    let (frames, (src_w, src_h)) = analyze(video, start, dur).expect("video not found");
    let crop_w_frac = (src_h as f64 * 9.0 / 16.0) / src_w as f64;
    for ((s, e), shot_frames, _) in shots(&frames, dur) {
        let tracks = track_faces(&shot_frames);
        let (mut shot, _) = classify_shot(&tracks, shot_frames.len(), src_w, src_h);
        println!("shot {:.1}-{:.1}s {} ({} frames)", (start + s) as f64 / 1e3, (start + e) as f64 / 1e3, shot.layout, shot_frames.len());
        for t in tracks.iter().filter(|t| t.samples.len() * 3 >= shot_frames.len()) {
            let m = t.median_box();
            let talk = median(t.talk.iter().map(|x| x.1));
            println!("  track cx={:.2} h={:.3} seen {} read {} median talk={talk:.3}", m.cx(), m.h, t.samples.len(), t.talk.len());
            if std::env::var("SERIES").is_ok() {
                println!("    {}", t.talk.iter().map(|x| format!("{:.0}", x.1 * 100.0)).collect::<Vec<_>>().join(" "));
            }
        }
        (shot.start_ms, shot.end_ms) = (s, e);
        for turn in follow_speaker(&shot, &tracks, shot_frames.len(), crop_w_frac) {
            println!(
                "  -> {:.1}-{:.1}s {} cx={:.2}",
                (start + turn.start_ms) as f64 / 1e3,
                (start + turn.end_ms) as f64 / 1e3,
                turn.layout,
                turn.people.first().map_or(0.5, |p| p.cx())
            );
        }
    }
}

/// Per speaker label: is the labelled person the one whose mouth moved most in
/// the 3 s around it, and by how much over the next person.
fn score_labels(filter: &str) {
    let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cases: Value = serde_json::from_str(&std::fs::read_to_string(engine.join("examples/layout_eval.json")).unwrap()).unwrap();
    let home = std::env::var("HOME").unwrap_or_default();
    let (mut total, mut right, mut margins) = (0, 0, Vec::new());
    for case in cases["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let labels: Vec<(i64, f64)> = case["labels"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|l| Some(((l[0].as_f64()? * 1000.0) as i64, l.get(3)?.as_f64()?)))
            .collect();
        if labels.is_empty() || !name.contains(filter) {
            continue;
        }
        let video = engine.join("../..").join(case["video"].as_str().unwrap().replacen('~', &home, 1));
        let start = (case["start"].as_f64().unwrap() * 1000.0) as i64;
        let Some((frames, _)) = analyze(&video, start, 32000) else {
            println!("SKIP {name}: {} not found", video.display());
            continue;
        };
        println!("{name}");
        for ((s, e), _, tracks) in shots(&frames, 32000) {
            for &(t, spk) in labels.iter().filter(|l| s <= l.0 && l.0 < e && tracks.len() >= 2) {
                let talk: Vec<f64> = tracks
                    .iter()
                    .map(|tr| {
                        let read: Vec<f64> = tr.talk.iter().filter(|x| (x.0 - t).abs() <= 1500).map(|x| x.1).collect();
                        if read.len() < MIN_TALK_SAMPLES { 0.0 } else { median(read) }
                    })
                    .collect();
                let off = |tr: &FaceTrack| (tr.median_box().cx() - spk).abs();
                let who = (0..tracks.len()).min_by(|a, b| off(&tracks[*a]).total_cmp(&off(&tracks[*b]))).unwrap();
                let other = talk.iter().enumerate().filter(|(i, _)| *i != who).map(|(_, a)| *a).fold(0.0, f64::max);
                let ok = talk[who] > other;
                total += 1;
                right += ok as usize;
                margins.push(talk[who] / other.max(0.01));
                let shown: Vec<String> = talk.iter().enumerate().map(|(i, a)| format!("{}{:.2}", if i == who { "*" } else { "" }, a)).collect();
                println!("  +{:>2}s {} [{}]", t / 1000, if ok { "ok  " } else { "MISS" }, shown.join(" "));
            }
        }
    }
    margins.sort_by(f64::total_cmp);
    let q = |p: f64| margins.get(((margins.len().max(1) - 1) as f64 * p) as usize).copied().unwrap_or(0.0);
    println!(
        "\nspeaker's mouth moves most: {right}/{total}; its lead over the next person: p10 {:.2}x, p25 {:.2}x, median {:.2}x",
        q(0.1),
        q(0.25),
        q(0.5)
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.as_slice() {
        [_, video, start, dur] => print_window(Path::new(video), start.parse().unwrap(), dur.parse().unwrap()),
        _ => score_labels(args.get(1).map_or("", String::as_str)),
    }
}
