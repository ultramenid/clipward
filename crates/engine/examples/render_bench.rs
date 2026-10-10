//! Times one real render_clip: `cargo run --release -p clip_engine --example render_bench -- <src> <transcript.json> <out.mp4> [9:16|16:9] [tight|natural]`
//! Env: BENCH_START, BENCH_STYLE, BENCH_LINK, BENCH_LOGOS, BENCH_LONGFORM, BENCH_X264, BENCH_ARGS; BENCH_N + BENCH_C render N clips.
use clip_engine::render::process::{LineSink, MediaOutput, MediaRunner, SystemRunner};
use clip_engine::render::service::{RenderRequest, Renderer};
use clip_engine::types::{EngineConfig, TranscriptSegment};
use futures::future::BoxFuture;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

struct Timed(SystemRunner);

fn label(program: &Path, args: &[String]) -> String {
    let name = program.file_name().unwrap().to_string_lossy().into_owned();
    let hint = args.iter().find(|a| a.starts_with("stream=") || a.starts_with("packet=") || (a.contains("loudnorm") && !a.contains("filter")));
    format!("{name} {}", hint.map(|h| &h[..h.len().min(40)]).unwrap_or(if args.iter().any(|a| a == "-filter_complex" || a == "-/filter_complex") { "RENDER" } else { "" }))
}

impl MediaRunner for Timed {
    fn run<'a>(&'a self, p: &'a Path, a: &'a [String], t: Duration, m: usize, c: &'a CancellationToken) -> BoxFuture<'a, clip_engine::error::Result<MediaOutput>> {
        Box::pin(async move {
            for x in a.iter().filter(|x| x.ends_with(".png")) { let _ = std::fs::copy(x, format!("{x}.keep")); }
            let start = Instant::now();
            let out = self.0.run(p, a, t, m, c).await;
            eprintln!("[bench] {:>7.2}s {}", start.elapsed().as_secs_f64(), label(p, a));
            if std::env::var("BENCH_ARGS").is_ok() { eprintln!("{a:?}"); }
            out
        })
    }
    fn run_lines<'a>(&'a self, p: &'a Path, a: &'a [String], t: Duration, m: usize, s: LineSink<'a>, c: &'a CancellationToken) -> BoxFuture<'a, clip_engine::error::Result<MediaOutput>> {
        Box::pin(async move {
            let start = Instant::now();
            let out = self.0.run_lines(p, a, t, m, s, c).await;
            eprintln!("[bench] {:>7.2}s {}", start.elapsed().as_secs_f64(), label(p, a));
            out
        })
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (src, transcript, out) = (PathBuf::from(&args[1]), &args[2], PathBuf::from(&args[3]));
    let aspect = args.get(4).cloned().unwrap_or("9:16".into());
    let pacing = args.get(5).cloned().unwrap_or("tight".into());
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(transcript).unwrap()).unwrap();
    let segments: Vec<TranscriptSegment> = serde_json::from_value(json["segments"].clone()).unwrap();
    // Shift a 60 s stretch of the real transcript onto the source's clock.
    // BENCH_START (ms): cut from there on the transcript's own clock (it matches the source).
    let base: i64 = std::env::var("BENCH_START").map_or(0, |v| v.parse().unwrap());
    let offset = if base > 0 { 0 } else { segments.iter().find(|s| s.start_time_ms > 300_000).unwrap().start_time_ms - 2_000 };
    let shifted: Vec<TranscriptSegment> = segments
        .into_iter()
        .map(|mut s| {
            s.start_time_ms -= offset;
            s.end_time_ms -= offset;
            for w in &mut s.words { w.start_time_ms -= offset; w.end_time_ms -= offset; }
            s
        })
        .filter(|s| base > 0 || (s.start_time_ms >= 0 && s.end_time_ms < 65_000))
        .collect();
    let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cfg = EngineConfig {
        ffmpeg: engine.join("../../engine-bin/ffmpeg"),
        ffprobe: engine.join("../../engine-bin/ffprobe"),
        ytdlp: None,
        assets_dir: engine.join("assets"),
        whisper_model_path: PathBuf::new(),
        ai: Default::default(),
        custom_vocabulary: vec![],
        work_dir: std::env::temp_dir(),
        output_dir: out.parent().unwrap().to_path_buf(),
    };
    let mut renderer = Renderer::new(&cfg);
    renderer.runner = Arc::new(Timed(SystemRunner));
    if std::env::var("BENCH_X264").is_ok() { renderer.videotoolbox = false; }
    let inner = clip_engine::render::service::default_analyzer(&cfg);
    renderer.analyzer = Some(Arc::new(move |req, cancel| {
        let inner = inner.clone();
        Box::pin(async move {
            let start = Instant::now();
            let r = inner(req, cancel).await;
            eprintln!("[bench] {:>7.2}s layout analysis", start.elapsed().as_secs_f64());
            r
        })
    }));
    let request = RenderRequest {
        video_path: src,
        output_path: out,
        start_time_ms: base + 3_000,
        end_time_ms: base + 48_000,
        transcript_segments: clip_engine::render::service::filter_transcript_for_clip(&shifted, base + 3_000, base + 48_000),
        layout_style: std::env::var("BENCH_STYLE").unwrap_or("auto".into()),
        title_text: Some("Getting the most out of the new model".into()),
        banner_platform: Some("youtube".into()),
        banner_channel_url: Some(std::env::var("BENCH_LINK").unwrap_or("youtube.com/@clipward".into())),
        logos: std::env::var("BENCH_LOGOS")
            .map(|v| {
                v.split(',')
                    .map(|entry| {
                        let mut parts = entry.split('|');
                        clip_engine::types::LogoOverlay {
                            path: parts.next().unwrap().into(),
                            position: parts.next().unwrap_or("top-right").into(),
                            size: parts.next().map_or(0.18, |s| s.parse().unwrap()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        aspect_ratio: aspect,
        layout_vision: false,
        pacing,
        longform: std::env::var("BENCH_LONGFORM").is_ok(),
        ..Default::default()
    };
    if let Ok(n) = std::env::var("BENCH_N") {
        let (n, c): (usize, usize) = (n.parse().unwrap(), std::env::var("BENCH_C").unwrap().parse().unwrap());
        let requests: Vec<RenderRequest> = (0..n)
            .map(|k| {
                let (s, e) = (base + 3_000 + k as i64 * 4_000, base + 43_000 + k as i64 * 4_000);
                RenderRequest {
                    output_path: request.output_path.with_file_name(format!("multi_{k}.mp4")),
                    start_time_ms: s,
                    end_time_ms: e,
                    transcript_segments: clip_engine::render::service::filter_transcript_for_clip(&shifted, s, e),
                    ..request.clone()
                }
            })
            .collect();
        let start = Instant::now();
        let results = clip_engine::render::service::render_clips(&renderer, &requests, c, None, &CancellationToken::new()).await;
        assert!(results.iter().all(|r| r.is_ok()));
        eprintln!("[bench] MULTI n={n} c={c}: {:.2}s total, {:.2}s per clip", start.elapsed().as_secs_f64(), start.elapsed().as_secs_f64() / n as f64);
        return;
    }
    let start = Instant::now();
    let result = renderer.render_clip(&request, &CancellationToken::new()).await.unwrap();
    eprintln!("[bench] TOTAL {:.2}s  layout={} cuts={} size={}", start.elapsed().as_secs_f64(), result.layout_type, result.cut_count, result.file_size_bytes);
}
