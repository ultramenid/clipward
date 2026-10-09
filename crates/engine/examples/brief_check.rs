//! Plans clips with a real provider and checks each clip against keyword rules:
//! `cargo run --release -p clip_engine --example brief_check -- <transcript.json> <brief.txt> <must-word> <banned-word> [provider] [cli-path] [model]`
use clip_engine::types::{AiSettings, ClipJobRequest, EngineConfig, Transcript};
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    let transcript = Transcript { segments: serde_json::from_value(json["segments"].clone()).unwrap(), ..Default::default() };
    let brief = std::fs::read_to_string(&args[2]).unwrap();
    let (must, banned) = (args[3].to_lowercase(), args[4].to_lowercase());
    let provider = args.get(5).cloned().unwrap_or("cli:claude".into());
    let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let work = tempfile::tempdir().unwrap();
    let cfg = EngineConfig {
        ffmpeg: engine.join("../../engine-bin/ffmpeg"),
        ffprobe: engine.join("../../engine-bin/ffprobe"),
        ytdlp: None,
        assets_dir: engine.join("assets"),
        whisper_model_path: PathBuf::new(),
        ai: AiSettings {
            provider,
            model: args.get(7).cloned().unwrap_or("opus".into()),
            cli_path: args.get(6).map(PathBuf::from),
            ..Default::default()
        },
        custom_vocabulary: vec![],
        work_dir: work.path().to_path_buf(),
        output_dir: work.path().to_path_buf(),
    };
    let req: ClipJobRequest = serde_json::from_value(serde_json::json!({
        "videoUrl": "x", "requirements": brief, "maxClips": null, "autoClipCount": true, "durationRanges": null,
        "aspectRatio": "9:16", "layoutStyle": "auto", "layoutVision": false, "pacing": "tight",
        "includeCaptions": true, "captionPreset": "subtle", "startTimeSeconds": null, "endTimeSeconds": null,
        "bannerPlatform": null, "bannerChannelUrl": null,
    }))
    .unwrap();
    let duration = transcript.segments.last().map(|s| s.end_time_ms as f64 / 1000.0).unwrap_or(0.0) + 1.0;
    let progress: clip_engine::types::ProgressFn = Arc::new(|_| {});
    let started = std::time::Instant::now();
    let plan = clip_engine::planner::plan_clips(
        &transcript,
        &req,
        &cfg,
        &work.path().join("x.mp4"),
        duration,
        &progress,
        &CancellationToken::new(),
    )
    .await;
    println!("planning took {:.0}s", started.elapsed().as_secs_f64());
    let plan = match plan {
        Ok(plan) => plan,
        Err(e) => {
            println!("RESULT: no clips — {e}");
            return;
        }
    };
    let mut compliant = 0;
    for (i, clip) in plan.clips.iter().enumerate() {
        // Words spoken inside the clip window.
        let text: String = transcript
            .segments
            .iter()
            .flat_map(|s| &s.words)
            .filter(|w| w.start_time_ms >= clip.start_time_ms && w.end_time_ms <= clip.end_time_ms)
            .map(|w| w.word.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let (has_must, has_banned) = (text.contains(&must), text.contains(&banned));
        let ok = has_must && !has_banned;
        compliant += ok as usize;
        println!(
            "clip {i}: {:>6.1}-{:>6.1}s  must='{must}':{has_must}  banned='{banned}':{has_banned}  {}\n   title: {}\n   tags: {:?}",
            clip.start_time_ms as f64 / 1000.0,
            clip.end_time_ms as f64 / 1000.0,
            if ok { "OK" } else { "VIOLATION" },
            clip.summary.as_deref().unwrap_or(""),
            clip.tags,
        );
    }
    println!("RESULT: {compliant}/{} clips comply", plan.clips.len());
}
