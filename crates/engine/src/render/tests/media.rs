//! Real FFmpeg checks (ports of test_av_sync.py, the media parts of test_video_speed.py
//! and test_clip_editor.py). Skipped when ffmpeg/ffprobe are not installed; point
//! TEST_FFMPEG / TEST_FFPROBE at the bundled binaries to test those.
//!
//! Fixtures carry a white flash and a 1 kHz beep at .5 s of every second; the tests
//! measure decoded flash/beep timing, not just container durations.
use super::support::*;
use crate::layout::types::{ClipLayoutPlan, ShotLayout, TALKING_HEAD};
use crate::layout::LayoutRequest;
use crate::render::layout_renderer::{build_layout_graph, measured_loudness_filter, GraphOptions};
use crate::render::process::SystemRunner;
use crate::render::pyfmt::Rational;
use crate::render::service::{FfmpegJob, RenderRequest, Renderer};
use crate::render::{scaled_duration_ms, AnalyzeFn};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

macro_rules! tools_or_skip {
    () => {
        match ffmpeg_tools() {
            Some(t) => t,
            None => {
                eprintln!("skipped: FFmpeg and FFprobe required");
                return;
            }
        }
    };
}

fn run(program: &Path, args: &[String]) -> Vec<u8> {
    let out = Command::new(program).args(args).output().expect("spawn");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[derive(Clone)]
struct Source {
    rate: &'static str,
    duration: f64,
    audio_delay: f64,
    video_delay: f64,
    sparse: bool,
    audio_gap: bool,
    sample_rate: u32,
    origin: f64,
    audio_end: Option<f64>,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            rate: "30",
            duration: 12.0,
            audio_delay: 0.0,
            video_delay: 0.0,
            sparse: false,
            audio_gap: false,
            sample_rate: 48000,
            origin: 0.0,
            audio_end: None,
        }
    }
}

fn source(ffmpeg: &Path, dir: &Path, o: &Source) -> PathBuf {
    let path = dir.join("source.mkv");
    let mut video = "drawbox=color=white:t=fill:enable='between(mod(t,1),0.5,0.7)'".to_string();
    if o.sparse {
        video += ",select='if(lt(t,6),1,not(mod(n,3)))'";
    }
    if o.video_delay > 0.0 {
        video += &format!(",trim=start={}", o.video_delay);
    }
    let mut audio = format!("atrim=start={}", o.audio_delay);
    if let Some(end) = o.audio_end {
        audio += &format!(":end={end}");
    }
    if o.audio_gap {
        audio += ",aselect='not(between(t,3,3.5))'";
    }
    let mut args = s(&["-v", "error", "-y", "-f", "lavfi", "-i"]);
    args.push(format!("color=black:s=64x64:r={}:d={}", o.rate, o.duration));
    args.extend(s(&["-f", "lavfi", "-i"]));
    args.push(format!(
        "aevalsrc=0.6*sin(2*PI*1000*t)*between(mod(t\\,1)\\,0.5\\,0.7):s={}:d={}",
        o.sample_rate, o.duration
    ));
    args.extend(s(&["-filter_complex"]));
    args.push(format!("[0:v]{video}[v];[1:a]{audio}[a]"));
    args.extend(s(&["-map", "[v]", "-map", "[a]", "-c:v", "ffv1", "-c:a", "pcm_s16le", "-output_ts_offset"]));
    args.push(o.origin.to_string());
    args.push(path.to_string_lossy().into_owned());
    run(ffmpeg, &args);
    path
}

fn probe(ffprobe: &Path, path: &Path) -> Value {
    let mut args = s(&["-v", "error", "-show_streams", "-show_format", "-of", "json"]);
    args.push(path.to_string_lossy().into_owned());
    serde_json::from_slice(&run(ffprobe, &args)).unwrap()
}

fn onsets(active: &[bool], rate: f64) -> Vec<f64> {
    (0..active.len()).filter(|&i| active[i] && (i == 0 || !active[i - 1])).map(|i| i as f64 / rate).collect()
}

fn audio_samples(ffmpeg: &Path, path: &Path, extra: &[&str]) -> Vec<f32> {
    let mut args = s(&["-v", "error", "-i"]);
    args.push(path.to_string_lossy().into_owned());
    args.extend(s(extra));
    args.extend(s(&["-f", "f32le", "-"]));
    run(ffmpeg, &args).chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// (flash onsets, beep onsets) in seconds.
fn events(ffmpeg: &Path, ffprobe: &Path, path: &Path) -> (Vec<f64>, Vec<f64>) {
    let info = probe(ffprobe, path);
    let video = info["streams"].as_array().unwrap().iter().find(|s| s["codec_type"] == "video").unwrap().clone();
    let mut args = s(&["-v", "error", "-i"]);
    args.push(path.to_string_lossy().into_owned());
    args.extend(s(&["-map", "0:v:0", "-vf", "scale=8:8", "-pix_fmt", "gray", "-f", "rawvideo", "-"]));
    let frames = run(ffmpeg, &args);
    let bright: Vec<bool> =
        frames.chunks_exact(64).map(|f| f.iter().map(|v| *v as f64).sum::<f64>() / 64.0 > 150.0).collect();
    let fps = Rational::parse(video["avg_frame_rate"].as_str().unwrap()).unwrap().to_f64();
    let start: f64 = video.get("start_time").and_then(|v| v.as_str()).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let flashes: Vec<f64> = onsets(&bright, fps).into_iter().map(|t| t + start).collect();

    let samples = if info["streams"].as_array().unwrap().iter().any(|s| s["codec_type"] == "audio") {
        audio_samples(
            ffmpeg,
            path,
            &["-map", "0:a:0", "-af", "aresample=48000:async=1:first_pts=0:min_hard_comp=0.001", "-ac", "1", "-ar", "48000"],
        )
    } else {
        Vec::new()
    };
    // 5 ms RMS windows suppress individual zero crossings and AAC ringing.
    let rms: Vec<f64> = samples
        .chunks_exact(240)
        .map(|w| (w.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / 240.0).sqrt())
        .collect();
    let peak = rms.iter().copied().fold(0.0, f64::max);
    let loud: Vec<bool> = rms.iter().map(|v| *v > (peak * 0.2).max(0.02)).collect();
    (flashes, onsets(&loud, 200.0))
}

fn max_diff(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f64::max)
}

fn media_renderer(ffmpeg: &Path, ffprobe: &Path, encoder: Option<Vec<String>>) -> Renderer {
    let mut r = renderer(Arc::new(SystemRunner));
    r.ffmpeg = ffmpeg.to_path_buf();
    r.ffprobe = ffprobe.to_path_buf();
    // MPEG-4 output runs on both the LGPL bundle and local builds.
    r.video_codec_override = Some(encoder.unwrap_or_else(|| s(&["-c:v", "mpeg4", "-q:v", "2", "-bf", "2"])));
    r
}

#[derive(Default)]
struct RenderOpts {
    duration: f64,
    start: f64,
    keeps: Option<Vec<(i64, i64)>>,
    shots: Option<Vec<ShotLayout>>,
    fps: Option<&'static str>,
    no_audio: bool,
    encoder: Option<Vec<String>>,
    overlay: bool,
    loudness: Option<String>,
}

fn render(tools: &(PathBuf, PathBuf), path: &Path, dir: &Path, o: RenderOpts) -> PathBuf {
    let (ffmpeg, ffprobe) = tools;
    let r = media_renderer(ffmpeg, ffprobe, o.encoder.clone());
    let duration_ms = (o.duration * 1000.0).round() as i64;
    let fps = o.fps.unwrap_or("30");
    let p = ClipLayoutPlan {
        shots: o.shots.clone().unwrap_or_else(|| vec![shot(0, duration_ms, TALKING_HEAD)]),
        source_width: 64,
        source_height: 64,
        ..Default::default()
    };
    let mut graph = build_layout_graph(
        &p,
        64,
        64,
        &GraphOptions {
            keeps: o.keeps.as_deref(),
            with_audio: !o.no_audio,
            fps,
            loudness_filter: o.loudness.as_deref(),
            ..Default::default()
        },
    )
    .unwrap();
    let mut extra = Vec::new();
    if o.overlay {
        let image = dir.join("overlay.png");
        let mut canvas = crate::render::fonts::Canvas::new(2, 2);
        canvas.rounded_rect(0.0, [255, 255, 255, 255]);
        std::fs::write(&image, canvas.encode_png().unwrap()).unwrap();
        graph += ";[base][1:v]overlay=shortest=1[out]";
        extra.push(image);
    } else {
        graph += ";[base]null[out]";
    }
    let out = dir.join("render.mp4");
    let job = FfmpegJob {
        input_path: path,
        more_sources: &[],
        audio_inputs: &[],
        output_path: &out,
        start_time_ms: (o.start * 1000.0).round() as i64,
        duration_ms,
        filter_complex: &graph,
        audio_label: (!o.no_audio).then_some("[aout]"),
        extra_inputs: &extra,
        fps,
        output_size: (64, 64),
        output_duration_ms: Some(o.keeps.as_ref().map(|k| k.iter().map(|(a, b)| b - a).sum()).unwrap_or(duration_ms)),
        progress_path: None,
    };
    block_on(r.run_ffmpeg_complex(&job, &CancellationToken::new())).unwrap();
    out
}

fn fps_f(fps: &str) -> f64 {
    Rational::parse(fps).unwrap().to_f64()
}

#[test]
fn content_stays_synchronized() {
    let tools = tools_or_skip!();
    let variants = [
        Source { audio_delay: 0.3, ..Default::default() },
        Source { video_delay: 0.3, ..Default::default() },
        Source { audio_gap: true, ..Default::default() },
        Source { sparse: true, ..Default::default() },
        Source { rate: "24000/1001", sample_rate: 44100, ..Default::default() },
        Source { origin: 7.0, audio_delay: 0.3, ..Default::default() },
    ];
    for o in variants {
        let tmp = tempfile::tempdir().unwrap();
        let path = source(&tools.0, tmp.path(), &o);
        let out = render(&tools, &path, tmp.path(), RenderOpts { duration: 12.0, ..Default::default() });
        let (flashes, beeps) = events(&tools.0, &tools.1, &out);
        assert!(flashes.len() == 12 && beeps.len() == 12, "{flashes:?} {beeps:?}");
        // Sparse source frames limit visual precision to 100 ms.
        let tolerance = if o.sparse { 0.105 } else { 1.0 / 30.0 + 0.006 };
        assert!(max_diff(&flashes, &beeps) <= tolerance, "{flashes:?} {beeps:?}");
    }
}

#[test]
fn many_shots_do_not_shift_the_edit_timeline() {
    let tools = tools_or_skip!();
    for fps in ["24", "30000/1001", "60"] {
        let tmp = tempfile::tempdir().unwrap();
        let path = source(&tools.0, tmp.path(), &Source { rate: fps, duration: 20.0, ..Default::default() });
        // 40 off-grid framing changes used to lengthen the concatenated timeline.
        let mut bounds = vec![0];
        bounds.extend((413..20000).step_by(487));
        bounds.push(20000);
        let shots = bounds.windows(2).map(|w| shot(w[0], w[1], TALKING_HEAD)).collect();
        let out = render(&tools, &path, tmp.path(), RenderOpts { duration: 20.0, shots: Some(shots), fps: Some(fps), ..Default::default() });
        let (flashes, beeps) = events(&tools.0, &tools.1, &out);
        let expected: Vec<f64> = (0..20).map(|k| k as f64 + 0.5).collect();
        assert!(flashes.len() == 20 && beeps.len() == 20, "{fps}: {flashes:?} {beeps:?}");
        let tolerance = 1.0 / fps_f(fps) + 0.006;
        assert!(max_diff(&flashes, &beeps) <= tolerance);
        assert!(max_diff(&beeps, &expected) <= 0.012, "{beeps:?}");
        assert!(max_diff(&flashes, &expected) <= tolerance, "{flashes:?}");
        let duration: f64 = probe(&tools.1, &out)["format"]["duration"].as_str().unwrap().parse().unwrap();
        assert!((duration - 20.0).abs() <= tolerance, "{duration}");
    }
}

#[test]
fn many_cuts_and_seek_share_caption_timeline() {
    let tools = tools_or_skip!();
    for fps in ["25", "30000/1001"] {
        let tmp = tempfile::tempdir().unwrap();
        let path = source(&tools.0, tmp.path(), &Source { rate: fps, duration: 24.0, audio_delay: 0.3, ..Default::default() });
        let (source_flashes, source_beeps) = events(&tools.0, &tools.1, &path);
        // Every beep survives, while 23 non-frame-aligned gaps are removed.
        let keeps: Vec<(i64, i64)> = (0..22).map(|k| (k * 1000 + 101, k * 1000 + 731)).collect();
        let shots = vec![shot(0, 11113, TALKING_HEAD), shot(11113, 22000, TALKING_HEAD)];
        let out = render(
            &tools,
            &path,
            tmp.path(),
            RenderOpts { duration: 22.0, start: 1.137, keeps: Some(keeps), shots: Some(shots), fps: Some(fps), overlay: true, ..Default::default() },
        );
        let (flashes, beeps) = events(&tools.0, &tools.1, &out);
        let expected: Vec<f64> = (0..22).map(|k| k as f64 * 0.630 + (0.5 - 0.137 - 0.101)).collect();
        let tolerance = 1.0 / fps_f(fps) + 0.006;
        assert!(flashes.len() == 22 && beeps.len() == 22, "{flashes:?} {beeps:?}");
        let mapped: Vec<f64> =
            (0..22).map(|k| source_flashes[k + 1] - (k as f64 + 1.0 + 0.137 + 0.101) + k as f64 * 0.630).collect();
        assert!(max_diff(&flashes, &mapped) <= tolerance, "{flashes:?} {mapped:?}");
        let quantization = max_diff(&source_flashes, &source_beeps);
        assert!(max_diff(&flashes, &beeps) <= quantization + tolerance);
        assert!(max_diff(&beeps, &expected) <= 0.012, "{beeps:?}");
        let duration: f64 = probe(&tools.1, &out)["format"]["duration"].as_str().unwrap().parse().unwrap();
        assert!((duration - 13.860).abs() <= tolerance, "{duration}");
    }
}

#[test]
fn missing_audio_at_edges_is_silence() {
    let tools = tools_or_skip!();
    for (delay, end, count) in [(3.0, Some(8.0), 5usize), (8.0, None, 4)] {
        let tmp = tempfile::tempdir().unwrap();
        let path = source(&tools.0, tmp.path(), &Source { audio_delay: delay, audio_end: end, ..Default::default() });
        let out = render(&tools, &path, tmp.path(), RenderOpts { duration: 12.0, ..Default::default() });
        let (flashes, beeps) = events(&tools.0, &tools.1, &out);
        assert_eq!(flashes.len(), 12);
        assert_eq!(beeps.len(), count, "{beeps:?}");
        let expected: Vec<f64> = (0..count).map(|k| k as f64 + delay + 0.5).collect();
        assert!(max_diff(&beeps, &expected) <= 0.012);
        for s in probe(&tools.1, &out)["streams"].as_array().unwrap() {
            let d: f64 = s["duration"].as_str().unwrap().parse().unwrap();
            assert!((d - 12.0).abs() <= 0.04, "{d}");
        }
    }
}

#[test]
fn window_before_first_audio_packet_renders_silence() {
    let tools = tools_or_skip!();
    let tmp = tempfile::tempdir().unwrap();
    let path = source(&tools.0, tmp.path(), &Source { audio_delay: 8.0, ..Default::default() });
    let out = render(&tools, &path, tmp.path(), RenderOpts { duration: 2.0, ..Default::default() });
    let (flashes, beeps) = events(&tools.0, &tools.1, &out);
    assert_eq!(flashes.len(), 2);
    assert!(beeps.is_empty());
}

#[test]
fn video_only_preserves_duration_across_off_grid_shots() {
    let tools = tools_or_skip!();
    let tmp = tempfile::tempdir().unwrap();
    let path = source(&tools.0, tmp.path(), &Source { rate: "24000/1001", ..Default::default() });
    let shots = vec![shot(0, 413, TALKING_HEAD), shot(413, 1477, TALKING_HEAD), shot(1477, 12000, TALKING_HEAD)];
    let out = render(&tools, &path, tmp.path(), RenderOpts { duration: 12.0, shots: Some(shots), no_audio: true, ..Default::default() });
    let data = probe(&tools.1, &out);
    let kinds: Vec<&str> = data["streams"].as_array().unwrap().iter().map(|s| s["codec_type"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["video"]);
    let d: f64 = data["format"]["duration"].as_str().unwrap().parse().unwrap();
    assert!((d - 12.0).abs() <= 0.034, "{d}");
}

#[test]
fn h264_aac_encoder_delay_does_not_move_presentation_start() {
    let tools = tools_or_skip!();
    let encoders = String::from_utf8_lossy(&run(&tools.0, &s(&["-hide_banner", "-encoders"]))).into_owned();
    for name in ["libx264", "h264_videotoolbox"] {
        if !encoders.contains(name) {
            eprintln!("skipped {name}: unavailable");
            continue;
        }
        let mut encoder = s(&["-c:v", name]);
        encoder.extend(if name == "libx264" { s(&["-preset", "ultrafast"]) } else { s(&["-b:v", "1M"]) });
        let tmp = tempfile::tempdir().unwrap();
        let path = source(&tools.0, tmp.path(), &Source { audio_delay: 0.3, ..Default::default() });
        let out = render(&tools, &path, tmp.path(), RenderOpts { duration: 8.0, start: 1.137, encoder: Some(encoder), ..Default::default() });
        let (flashes, beeps) = events(&tools.0, &tools.1, &out);
        assert!(flashes.len() == 8 && beeps.len() == 8);
        assert!(max_diff(&flashes, &beeps) <= 1.0 / 30.0 + 0.006);
        for st in probe(&tools.1, &out)["streams"].as_array().unwrap() {
            let start: f64 = st["start_time"].as_str().unwrap().parse().unwrap();
            assert!(start.abs() <= 0.001, "{start}");
        }
    }
}

#[test]
fn stereo_channels_are_not_collapsed_by_audio_polish() {
    let tools = tools_or_skip!();
    let tmp = tempfile::tempdir().unwrap();
    let path = source(&tools.0, tmp.path(), &Source::default());
    let stereo = tmp.path().join("stereo.mkv");
    let mut args = s(&["-v", "error", "-y", "-i"]);
    args.push(path.to_string_lossy().into_owned());
    args.extend(s(&["-map", "0:v", "-map", "0:a", "-c:v", "copy", "-af", "pan=stereo|c0=c0|c1=0*c0", "-c:a", "pcm_s16le"]));
    args.push(stereo.to_string_lossy().into_owned());
    run(&tools.0, &args);
    let out = render(&tools, &stereo, tmp.path(), RenderOpts { duration: 12.0, ..Default::default() });
    let samples = audio_samples(&tools.0, &out, &["-map", "0:a:0"]);
    let left = samples.iter().step_by(2).fold(0f32, |m, v| m.max(v.abs()));
    let right = samples.iter().skip(1).step_by(2).fold(0f32, |m, v| m.max(v.abs()));
    assert!(left > 0.1 && right < 0.001, "{left} {right}");
}

#[test]
fn aac_source_priming_and_seek_preserve_content_clock() {
    let tools = tools_or_skip!();
    for origin in ["0", "7"] {
        let tmp = tempfile::tempdir().unwrap();
        let path = source(&tools.0, tmp.path(), &Source { audio_delay: 0.3, ..Default::default() });
        let compressed = tmp.path().join("aac.mkv");
        let mut args = s(&["-v", "error", "-y", "-i"]);
        args.push(path.to_string_lossy().into_owned());
        args.extend(s(&["-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-output_ts_offset", origin]));
        args.push(compressed.to_string_lossy().into_owned());
        run(&tools.0, &args);
        let out = render(&tools, &compressed, tmp.path(), RenderOpts { duration: 8.0, start: 1.137, ..Default::default() });
        let (flashes, beeps) = events(&tools.0, &tools.1, &out);
        assert!(flashes.len() == 8 && beeps.len() == 8);
        assert!(max_diff(&flashes, &beeps) <= 1.0 / 30.0 + 0.006);
        let expected: Vec<f64> = (0..8).map(|k| k as f64 + 0.363).collect();
        assert!(max_diff(&beeps, &expected) <= 0.012, "{beeps:?}");
    }
}

#[test]
fn fractional_duration_preserves_audio_playback_clock() {
    let tools = tools_or_skip!();
    for (duration_ms, overlay) in [(12001, false), (12051, true), (12099, false), (12020, true)] {
        let tmp = tempfile::tempdir().unwrap();
        let path = source(&tools.0, tmp.path(), &Source { duration: 14.0, ..Default::default() });
        let out = render(&tools, &path, tmp.path(), RenderOpts { duration: duration_ms as f64 / 1000.0, overlay, ..Default::default() });
        let (flashes, beeps) = events(&tools.0, &tools.1, &out);
        assert!(flashes.len() == 12 && beeps.len() == 12);
        let expected: Vec<f64> = (0..12).map(|k| k as f64 + 0.5).collect();
        assert!(max_diff(&beeps, &expected) <= 0.012, "{beeps:?}");
        assert!(max_diff(&flashes, &beeps) <= 1.0 / 30.0 + 0.006);
    }
}

#[test]
fn fractional_cut_timeline_with_dynamic_linear_and_fallback_normalization() {
    let tools = tools_or_skip!();
    for peak in [None, Some(-10.0), Some(-1.0)] {
        let tmp = tempfile::tempdir().unwrap();
        let path = source(&tools.0, tmp.path(), &Source { duration: 14.0, ..Default::default() });
        let loudness = peak.and_then(|p: f64| {
            measured_loudness_filter(&json!({"input_i": -20, "input_tp": p, "input_lra": 6, "input_thresh": -30, "target_offset": 0}))
        });
        let out = render(
            &tools,
            &path,
            tmp.path(),
            RenderOpts { duration: 12.0, keeps: Some(vec![(100, 4013), (5013, 11120)]), overlay: true, loudness, ..Default::default() },
        );
        let (flashes, beeps) = events(&tools.0, &tools.1, &out);
        assert!(flashes.len() == 10 && beeps.len() == 10, "{flashes:?} {beeps:?}");
        let expected: Vec<f64> = (0..10).map(|k| k as f64 + 0.4).collect();
        assert!(max_diff(&beeps, &expected) <= 0.012, "{beeps:?}");
        assert!(max_diff(&flashes, &beeps) <= 1.0 / 30.0 + 0.006);
    }
}

// ---------------------------------------------------------------- full render_clip exports

fn talking_head_analyzer() -> AnalyzeFn {
    Arc::new(|req: LayoutRequest, _c: CancellationToken| {
        let p = ClipLayoutPlan {
            shots: vec![shot(0, req.duration_ms, TALKING_HEAD)],
            source_width: req.src_w,
            source_height: req.src_h,
            ..Default::default()
        };
        Box::pin(async move { Ok(Some(p)) })
    })
}

/// Beep pitch: zero crossings inside loud 5 ms windows (48 kHz mono).
fn beep_frequency(samples: &[f32]) -> f64 {
    let (mut crossings, mut windows) = (0usize, 0usize);
    for w in samples.chunks_exact(240) {
        let rms = (w.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / 240.0).sqrt();
        if rms < 0.1 {
            continue;
        }
        windows += 1;
        crossings += w.windows(2).filter(|p| (p[0] < 0.0) != (p[1] < 0.0)).count();
    }
    crossings as f64 / 2.0 / (windows as f64 * 0.005)
}

#[test]
fn real_exports_preserve_sync_and_pitch_after_seek_and_cuts() {
    let tools = tools_or_skip!();
    for speed in [1.0, 1.25, 2.0] {
        let tmp = tempfile::tempdir().unwrap();
        let path = source(&tools.0, tmp.path(), &Source { duration: 14.0, audio_delay: 0.3, audio_gap: true, ..Default::default() });
        let mut r = media_renderer(&tools.0, &tools.1, None);
        r.analyzer = Some(talking_head_analyzer());
        let request = RenderRequest {
            video_path: path,
            output_path: tmp.path().join("clip.mp4"),
            start_time_ms: 1137,
            end_time_ms: 13137,
            source_width: 64,
            source_height: 64,
            video_speed: speed,
            pacing: "natural".into(),
            aspect_ratio: "16:9".into(),
            apply_padding: false,
            include_captions: false,
            skip_ranges_ms: vec![(3137, 4137)],
            title_text: Some("Faster clips".into()),
            banner_platform: Some("youtube".into()),
            banner_channel_url: Some("example.com".into()),
            ..Default::default()
        };
        // 16:9 renders at 1920x1080 here; the timing checks don't depend on size.
        let result = block_on(r.render_clip(&request, &CancellationToken::new())).unwrap();
        assert_eq!(result.render_fallback, None);
        assert_eq!(result.duration_ms, scaled_duration_ms(11_000, speed).unwrap());
        let (flashes, beeps) = events(&tools.0, &tools.1, &result.output_path);
        let expected: Vec<f64> = (0..12)
            .map(|k| 1.5 + k as f64)
            .filter(|t| !(3.137..4.137).contains(t))
            .map(|t| (t - 1.137 - if t >= 4.137 { 1.0 } else { 0.0 }) / speed)
            .collect();
        assert!(flashes.len() == expected.len() && beeps.len() == expected.len(), "{flashes:?} {beeps:?} {expected:?}");
        assert!(max_diff(&beeps, &expected) < 0.03, "{beeps:?} {expected:?}");
        assert!(max_diff(&flashes, &beeps) < 1.0 / 30.0 + 0.02);
        let d: f64 = probe(&tools.1, &result.output_path)["format"]["duration"].as_str().unwrap().parse().unwrap();
        assert!((d - 11.0 / speed).abs() <= 0.05, "{d}");
        let samples = audio_samples(&tools.0, &result.output_path, &["-ac", "1", "-ar", "48000"]);
        let freq = beep_frequency(&samples);
        assert!((freq - 1000.0).abs() < 60.0, "pitch {freq}");
    }
}

#[test]
fn burned_captions_speed_up_with_the_picture() {
    let tools = tools_or_skip!();
    let filters = String::from_utf8_lossy(&run(&tools.0, &s(&["-hide_banner", "-filters"]))).into_owned();
    if !filters.contains(" ass ") {
        eprintln!("skipped: FFmpeg without libass");
        return;
    }
    for with_audio in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let mut path = source(&tools.0, tmp.path(), &Source { duration: 6.0, ..Default::default() });
        if !with_audio {
            let silent = tmp.path().join("silent.mkv");
            let mut args = s(&["-v", "error", "-y", "-i"]);
            args.push(path.to_string_lossy().into_owned());
            args.extend(s(&["-map", "0:v:0", "-c:v", "copy", "-an"]));
            args.push(silent.to_string_lossy().into_owned());
            run(&tools.0, &args);
            path = silent;
        }
        let transcript = vec![segment(2100, 2400, "SPEED", vec![word("SPEED", 2100, 2400)])];
        let mut measurements = Vec::new();
        for speed in [1.0, 2.0] {
            let mut r = media_renderer(&tools.0, &tools.1, None);
            r.analyzer = Some(talking_head_analyzer());
            let request = RenderRequest {
                video_path: path.clone(),
                output_path: tmp.path().join(format!("caption-{speed}.mp4")),
                start_time_ms: 0,
                end_time_ms: 6000,
                source_width: 64,
                source_height: 64,
                video_speed: speed,
                pacing: "natural".into(),
                apply_padding: false,
                transcript_segments: transcript.clone(),
                include_captions: true,
                ..Default::default()
            };
            let result = block_on(r.render_clip(&request, &CancellationToken::new())).unwrap();
            assert_eq!(result.duration_ms as f64, 6000.0 / speed);
            let has_audio = probe(&tools.1, &result.output_path)["streams"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["codec_type"] == "audio");
            assert_eq!(has_audio, with_audio);
            let mut means = Vec::new();
            for timestamp in [2.2 / speed, 3.2 / speed] {
                let mut args = s(&["-v", "error", "-ss"]);
                args.push(timestamp.to_string());
                args.push("-i".into());
                args.push(result.output_path.to_string_lossy().into_owned());
                // The caption band of the 1080x1920 frame (talking-head anchor at y=1340).
                args.extend(s(&["-frames:v", "1", "-vf", "crop=1080:300:0:1190", "-pix_fmt", "gray", "-f", "rawvideo", "-"]));
                let frame = run(&tools.0, &args);
                means.push(frame.iter().map(|v| *v as f64).sum::<f64>() / frame.len() as f64);
            }
            assert!(means[0] > means[1] + 1.0, "{means:?}");
            measurements.push(means);
        }
        assert!((measurements[0][0] - measurements[1][0]).abs() <= 2.0, "{measurements:?}");
    }
}

#[test]
fn longform_landscape_render_with_two_pass_loudness_and_sidecar() {
    let tools = tools_or_skip!();
    let tmp = tempfile::tempdir().unwrap();
    let path = source(&tools.0, tmp.path(), &Source { duration: 14.0, ..Default::default() });
    let r = media_renderer(&tools.0, &tools.1, None);
    let words: Vec<_> = (0..12).map(|k| word(&format!("w{k}."), k * 1000 + 400, k * 1000 + 800)).collect();
    let request = RenderRequest {
        video_path: path,
        output_path: tmp.path().join("episode.mp4"),
        start_time_ms: 500,
        end_time_ms: 12_500,
        source_width: 64,
        source_height: 64,
        aspect_ratio: "16:9".into(),
        longform: true,
        transcript_segments: vec![segment(400, 11_800, "words", words)],
        chapters: vec![(500, "Intro".into())],
        ..Default::default()
    };
    let result = block_on(r.render_clip(&request, &CancellationToken::new())).unwrap();
    assert_eq!((result.output_width, result.output_height), (1920, 1080));
    let srt = std::fs::read_to_string(result.subtitle_path.expect("srt")).unwrap();
    assert!(srt.starts_with("1\n00:00:00,"), "{srt}");
    assert!(result.output_path.is_file());
}

#[test]
fn ffmpeg_cuts_keep_audio_video_in_sync() {
    let tools = tools_or_skip!();
    let mut p = ClipLayoutPlan {
        shots: vec![shot(0, 3000, TALKING_HEAD), shot(3000, 6000, crate::layout::types::SCREEN)],
        source_width: 1920,
        source_height: 1080,
        ..Default::default()
    };
    p.shots[0].focus_path = vec![(0, 0.4, 0.4)];
    let keeps = vec![(0, 700), (1300, 2200), (2600, 4100), (4500, 5300), (5600, 6000)];
    let graph = build_layout_graph(&p, 1080, 1920, &GraphOptions { keeps: Some(&keeps), with_audio: true, ..Default::default() })
        .unwrap()
        .replace("[0:a:0]", "[1:a:0]");
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("cut.mp4");
    let mut args = s(&["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30:duration=6", "-f", "lavfi", "-i", "sine=frequency=440:duration=6", "-filter_complex"]);
    args.push(graph);
    args.extend(s(&["-map", "[base]", "-map", "[aout]", "-c:v", "mpeg4", "-c:a", "aac"]));
    args.push(out.to_string_lossy().into_owned());
    run(&tools.0, &args);
    let info = probe(&tools.1, &out);
    let expected = keeps.iter().map(|(a, b)| b - a).sum::<i64>() as f64 / 1000.0;
    let dur = |kind: &str| -> f64 {
        info["streams"].as_array().unwrap().iter().find(|s| s["codec_type"] == kind).unwrap()["duration"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    };
    assert!((dur("video") - expected).abs() <= 0.07);
    assert!((dur("audio") - dur("video")).abs() <= 0.07);
}

#[test]
fn logos_are_stamped_in_their_corners_and_the_originals_kept() {
    let tools = tools_or_skip!();
    let tmp = tempfile::tempdir().unwrap();
    let path = source(&tools.0, tmp.path(), &Source { duration: 4.0, ..Default::default() });
    let mut logos = Vec::new();
    for (name, position) in [("logo.png", "top-left"), ("logo.jpg", "bottom-right")] {
        let logo = tmp.path().join(name);
        let mut args = s(&["-v", "error", "-y", "-f", "lavfi", "-i", "color=white:s=200x100", "-frames:v", "1"]);
        args.push(logo.to_string_lossy().into_owned());
        run(&tools.0, &args);
        logos.push(crate::types::LogoOverlay { path: logo.to_string_lossy().into_owned(), position: position.into(), size: 0.2 });
    }
    let mut r = media_renderer(&tools.0, &tools.1, None);
    r.analyzer = Some(talking_head_analyzer());
    let request = RenderRequest {
        video_path: path,
        output_path: tmp.path().join("clip.mp4"),
        start_time_ms: 0,
        end_time_ms: 4000,
        source_width: 64,
        source_height: 64,
        pacing: "natural".into(),
        apply_padding: false,
        include_captions: false,
        logos: logos.clone(),
        ..Default::default()
    };
    let result = block_on(r.render_clip(&request, &CancellationToken::new())).unwrap();
    // 1080x1920 frame: 0.2 x 1080 = 216 px wide logos, 43 px from the edges.
    let mean = |crop: &str| {
        let mut args = s(&["-v", "error", "-ss", "1.0", "-i"]);
        args.push(result.output_path.to_string_lossy().into_owned());
        args.extend(s(&["-frames:v", "1", "-vf", crop, "-pix_fmt", "gray", "-f", "rawvideo", "-"]));
        let frame = run(&tools.0, &args);
        frame.iter().map(|v| *v as f64).sum::<f64>() / frame.len() as f64
    };
    let (top_left, bottom_right) = (mean("crop=160:60:70:70"), mean("crop=160:60:850:1790"));
    let (top_right, bottom_left) = (mean("crop=160:60:850:70"), mean("crop=160:60:70:1790"));
    assert!(top_left > 230.0 && bottom_right > 230.0, "{top_left} {bottom_right}");
    assert!(top_right < 200.0 && bottom_left < 200.0, "{top_right} {bottom_left}");
    assert!(logos.iter().all(|l| Path::new(&l.path).is_file()), "the user's logo files are never deleted");
}

#[test]
fn renders_in_software_when_the_gpu_encoder_fails() {
    // The bundles are LGPL (no libx264): the retry must use an encoder they have.
    let tools = tools_or_skip!();
    let tmp = tempfile::tempdir().unwrap();
    let path = source(&tools.0, tmp.path(), &Source::default());
    let mut r = media_renderer(&tools.0, &tools.1, None);
    r.video_codec_override = None;
    r.x264 = tokio::sync::OnceCell::new();
    r.hw_encoder = tokio::sync::OnceCell::new_with(Some(Some(crate::render::service::HwEncoder::Vaapi)));
    let out = tmp.path().join("render.mp4");
    let job = FfmpegJob {
        input_path: &path,
        more_sources: &[],
        audio_inputs: &[],
        output_path: &out,
        start_time_ms: 0,
        duration_ms: 2000,
        filter_complex: "[0:v]null[out]",
        audio_label: None,
        extra_inputs: &[],
        fps: "30",
        output_size: (64, 64),
        output_duration_ms: None,
        progress_path: None,
    };
    block_on(r.run_ffmpeg_complex(&job, &CancellationToken::new())).unwrap();
    assert_eq!(probe(&tools.1, &out)["streams"][0]["codec_name"], "h264");
}
