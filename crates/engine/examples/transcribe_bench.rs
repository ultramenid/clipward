//! Times whisper.cpp on a slice of a video with the engine's settings, and writes the
//! words so two runs can be compared:
//! `cargo run --release -p clip_engine --example transcribe_bench -- <video> <model> <start s> <length s> <words.json>`
//! BENCH_FLASH=1 turns flash attention on; BENCH_DTW=1 writes each word's end from
//! whisper.cpp's DTW alignment in place of its start (a reference to judge the others by;
//! large-v3-turbo only, not with flash attention).
//! BENCH_JOBS=<n> instead runs the engine's own transcription n times in one process (model
//! cache, one transcription at a time, VAD when transcribe::VAD_MODEL_FILE sits next to
//! <model>), all at once with BENCH_PARALLEL=1, and writes every job's words.
use std::path::PathBuf;
use std::time::Instant;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ffmpeg = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../engine-bin/ffmpeg");
    if let Ok(jobs) = std::env::var("BENCH_JOBS") {
        return engine_jobs(&args, ffmpeg, jobs.parse().unwrap());
    }
    // The engine's own extraction (transcribe::extract_args).
    let audio = std::process::Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-ss", &args[3], "-t", &args[4], "-i", &args[1], "-map", "0:a:0", "-vn"])
        .args(["-af", "aresample=16000:async=1:first_pts=0:min_hard_comp=0.001", "-acodec", "pcm_f32le", "-ar", "16000", "-ac", "1", "-f", "f32le", "pipe:1"])
        .output()
        .unwrap();
    let samples: Vec<f32> = audio.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();

    let mut context = WhisperContextParameters::default();
    context.flash_attn(std::env::var("BENCH_FLASH").is_ok());
    let dtw = std::env::var("BENCH_DTW").is_ok();
    if dtw {
        context.dtw_parameters.mode = whisper_rs::DtwMode::ModelPreset { model_preset: whisper_rs::DtwModelPreset::LargeV3Turbo };
    }
    let load = Instant::now();
    let ctx = WhisperContext::new_with_params(&args[2], context).unwrap();
    let mut state = ctx.create_state().unwrap();
    let load = load.elapsed().as_secs_f64();

    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 5 });
    params.set_n_threads(threads.saturating_sub(1).clamp(1, 8) as i32);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_token_timestamps(true);
    params.set_max_len(1);
    params.set_split_on_word(true);
    params.set_language(Some("auto"));

    let run = Instant::now();
    state.full(params, &samples).unwrap();
    let run = run.elapsed().as_secs_f64();
    let words: Vec<(i64, i64, String)> =
        state
            .as_iter()
            .map(|s| {
                // DTW times a token at its end: the last aligned token's is the word's end.
                let aligned = (0..s.n_tokens()).filter_map(|i| s.get_token(i)).map(|t| t.token_data().t_dtw).filter(|t| *t >= 0).last();
                let start = if dtw { aligned.unwrap_or(-1) } else { s.start_timestamp() };
                (start, s.end_timestamp(), s.to_str_lossy().unwrap().into_owned())
            })
            .collect();
    std::fs::write(&args[5], serde_json::to_string(&words).unwrap()).unwrap();
    let seconds = samples.len() as f64 / 16_000.0;
    println!("audio {seconds:.0}s  load {load:.1}s  transcribe {run:.1}s  ({:.1}x real time)  {} words", seconds / run, words.len());
}

/// BENCH_JOBS: `transcribe::transcribe` as a job calls it, after the job-start preload.
fn engine_jobs(args: &[String], ffmpeg: PathBuf, jobs: usize) {
    use clip_engine::transcribe;
    use clip_engine::types::{EngineConfig, ProgressFn};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    let cfg = Arc::new(EngineConfig {
        ffprobe: ffmpeg.with_file_name("ffprobe"),
        ffmpeg,
        ytdlp: None,
        assets_dir: PathBuf::new(),
        whisper_model_path: args[2].clone().into(),
        ai: Default::default(),
        custom_vocabulary: vec![],
        work_dir: std::env::temp_dir(),
        output_dir: std::env::temp_dir(),
    });
    let video = PathBuf::from(&args[1]);
    let start: f64 = args[3].parse().unwrap();
    let range = Some((start, start + args[4].parse::<f64>().unwrap()));
    let parallel = std::env::var("BENCH_PARALLEL").is_ok();
    let began = Instant::now();
    let job = move |i: usize| {
        let (cfg, video) = (cfg.clone(), video.clone());
        async move {
            let t = Instant::now();
            transcribe::preload_model(&cfg.whisper_model_path);
            let progress: ProgressFn = Arc::new(move |p| {
                if p.step.starts_with("Waiting") {
                    println!("job {i}: {}", p.step);
                }
            });
            let transcript = transcribe::transcribe(&video, &cfg, range, &progress, &CancellationToken::new()).await.unwrap();
            println!("job {i}: {:.2}s", t.elapsed().as_secs_f64());
            transcript.segments.into_iter().flat_map(|s| s.words).map(|w| (w.start_time_ms, w.end_time_ms, w.word)).collect::<Vec<_>>()
        }
    };
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let words: Vec<Vec<(i64, i64, String)>> = runtime.block_on(async {
        if parallel {
            let handles: Vec<_> = (0..jobs).map(|i| tokio::spawn(job(i))).collect();
            let mut words = Vec::new();
            for handle in handles {
                words.push(handle.await.unwrap());
            }
            words
        } else {
            let mut words = Vec::new();
            for i in 0..jobs {
                words.push(job(i).await);
            }
            words
        }
    });
    println!("all jobs: {:.2}s", began.elapsed().as_secs_f64());
    std::fs::write(&args[5], serde_json::to_string(&words).unwrap()).unwrap();
}
