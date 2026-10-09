//! Re-runs a finished job's clips through the whole pipeline as a manual run (no planner)
//! and prints where the time went:
//! `cargo run --release -p clip_engine --example job_bench -- <run folder> <whisper model> <out dir>`
use clip_engine::types::{ClipJobRequest, EngineConfig};
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (run, model, out) = (PathBuf::from(&args[1]), PathBuf::from(&args[2]), PathBuf::from(&args[3]));
    let read = |name: &str| -> serde_json::Value { serde_json::from_str(&std::fs::read_to_string(run.join(name)).unwrap()).unwrap() };
    let (saved, plan) = (read("request.json"), read("plan.json"));
    // The run's own settings, with its planned clips typed in as manual ranges.
    let mut request = saved["request"].clone();
    request["videoUrl"] = saved["source"]["path"].clone();
    request["manualClips"] = plan["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| serde_json::json!({ "startSeconds": s["start_time_ms"].as_f64().unwrap() / 1000.0, "endSeconds": s["end_time_ms"].as_f64().unwrap() / 1000.0 }))
        .collect();
    let request: ClipJobRequest = serde_json::from_value(request).unwrap();

    let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cfg = EngineConfig {
        ffmpeg: engine.join("../../engine-bin/ffmpeg"),
        ffprobe: engine.join("../../engine-bin/ffprobe"),
        ytdlp: None,
        assets_dir: engine.join("assets"),
        whisper_model_path: model,
        ai: Default::default(),
        custom_vocabulary: vec![],
        work_dir: out.join("work"),
        output_dir: out.clone(),
    };
    let output = clip_engine::pipeline::run_job(request, cfg, Arc::new(|_| {}), CancellationToken::new()).await.unwrap();
    let metrics = &output["metrics"];
    println!("total {:.1}s  stages {}", output["processing_time_seconds"].as_f64().unwrap_or(0.0), metrics["stage_durations_seconds"]);
    for clip in metrics["clip_layouts"].as_array().into_iter().flatten() {
        println!(
            "clip {}: analysis {}s  loudness {}s  encode {}s  ({})",
            clip["clip_index"], clip["analysis_seconds"], clip["loudness_seconds"], clip["encode_seconds"], clip["layout_type"]
        );
    }
}
