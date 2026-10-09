//! Times whisper.cpp on a slice of a video with the engine's settings, and writes the
//! words so two runs can be compared:
//! `cargo run --release -p clip_engine --example transcribe_bench -- <video> <model> <start s> <length s> <words.json>`
//! BENCH_FLASH=1 turns flash attention on; BENCH_VAD=<silero ggml model> skips non-speech;
//! BENCH_DTW=1 writes each word's end from whisper.cpp's DTW alignment in place of its
//! start (a reference to judge the others by; large-v3-turbo only, not with flash attention).
use std::path::PathBuf;
use std::time::Instant;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ffmpeg = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../engine-bin/ffmpeg");
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
    let vad = std::env::var("BENCH_VAD").ok();
    if let Some(path) = vad.as_deref() {
        params.set_vad_model_path(Some(path));
        params.enable_vad(true);
    }

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
