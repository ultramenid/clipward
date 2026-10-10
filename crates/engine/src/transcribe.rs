//! Offline word-timed transcription with whisper.cpp (via whisper-rs).
//!
//! Port of `engine/clip_engine/services/transcription_service.py` (local path only):
//! ffmpeg extracts 16 kHz mono audio (optionally just a padded window of the source),
//! whisper.cpp runs with token timestamps and one word per segment
//! (`max_len = 1`, `split_on_word`), non-speech markers such as `[BLANK_AUDIO]` or
//! `(laughter)` are dropped (never captioned), word times are shifted back onto the
//! source timeline and grouped into sentence segments (a pause over two seconds also
//! ends a segment).
//!
//! Also hosts the sentence-boundary helpers the planner uses to snap clip edges
//! (`find_sentence_end_boundary`, `find_sentence_start_boundary`,
//! `last_sentence_end_between`, `next_start_at_or_after`), as the Python module did.
//!
//! Errors carry the lowercase marker phrases `pipeline::describe_failure` maps to UI
//! text; they never include paths or decoder output.
use crate::error::{EngineError, Result};
use crate::media;
use crate::types::{EngineConfig, Progress, ProgressFn, Stage, Transcript, TranscriptSegment, TranscriptWord};
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use whisper_rs::WhisperContext;

/// Context kept around a requested time range so sentence boundaries at its edges still resolve.
pub const TRANSCRIPTION_RANGE_PAD_SECONDS: f64 = 5.0;

/// The planner sees times rounded to 0.1 s, so a clip end it copies from a transcript
/// line can land up to 50 ms after the real sentence end. Treat a sentence end this
/// close to the requested time as the intended one.
pub const SENTENCE_END_TOLERANCE_MS: i64 = 300;

/// `Transcript::model` when the source has no audio track (visual-only planning).
/// Mirrors the Python pipeline's `TranscriptionResult(provider="no_audio")` fallback.
pub const NO_AUDIO_MODEL: &str = "no_audio";

/// Sentence-ending punctuation marks.
const SENTENCE_END_PUNCTUATION: [char; 6] = ['.', '!', '?', '。', '！', '？'];

// Conservative application limits for vocabulary hints.
const KEYTERM_MAX_CHARS: usize = 49;
const KEYTERM_MAX_WORDS: usize = 5;
const KEYTERM_MAX_COUNT: usize = 200;
const KEYTERM_FORBIDDEN_CHARS: [char; 7] = ['<', '>', '{', '}', '[', ']', '\\'];

/// Input whitelist from services/media_process.py (MEDIA_INPUT_OPTIONS).
const MEDIA_INPUT_OPTIONS: [&str; 4] = [
    "-protocol_whitelist",
    "file,pipe,fd",
    "-format_whitelist",
    "mov,matroska,webm,avi,flv,mpegts",
];

const SAMPLE_RATE: usize = 16_000;
/// Same bound the Python `_audio_duration` accepted: (0, 6 h + 1 s].
const MAX_AUDIO_SECONDS: f64 = 6.0 * 3600.0 + 1.0;
const MAX_SAMPLES: usize = (MAX_AUDIO_SECONDS as usize) * SAMPLE_RATE;
/// MEDIA_TIMEOUT_SECONDS in media_process.py.
const MEDIA_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Pipeline percent while transcribing (the Python pipeline kept 15 for the whole stage).
const TRANSCRIBING_PERCENT: f64 = 15.0;

/// Silero VAD model, read from next to the whisper model. When it is there long
/// silences and music are cut before decoding (they cost no encoder time and can't
/// turn into hallucinated words); without it every second is decoded.
/// ponytail: the app doesn't download it yet. On a 57 min talk show it was 3% slower
/// and changed 15% of words (some loops gone, new ones added); fetch it from core once
/// real-video accuracy checks favour it.
pub const VAD_MODEL_FILE: &str = "ggml-silero-v5.1.2.bin";
/// Audio kept either side of detected speech so word onsets and endings survive.
const VAD_SPEECH_PAD_MS: c_int = 200;
/// Pauses up to this long stay as they are; longer ones (silence, music) shrink to it.
const VAD_MAX_PAUSE_SAMPLES: usize = SAMPLE_RATE;

/// The last loaded model, shared by jobs: loading it costs 0.3-3.5 s per job otherwise.
// ponytail: stays resident until another model is used; drop it when idle if memory matters.
static MODEL: Mutex<Option<(PathBuf, Arc<WhisperContext>)>> = Mutex::new(None);
/// One transcription at a time: two jobs sharing the GPU finish no sooner than in turn.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Environment variables media tools keep (media_process._ALLOWED_ENV); provider keys never reach them.
const ALLOWED_ENV: [&str; 16] = [
    "PATH", "HOME", "TMPDIR", "TEMP", "TMP", "SYSTEMROOT", "WINDIR", "USERPROFILE",
    "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "FONTCONFIG_PATH", "FONTCONFIG_FILE",
    "LANG", "LC_ALL", "LC_CTYPE", "TZ",
];

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Transcribes `video` offline with the ggml model at `cfg.whisper_model_path`.
///
/// `range` is an optional (start, end) source window in seconds. Only that part of the
/// audio (plus [`TRANSCRIPTION_RANGE_PAD_SECONDS`] of context either side) is decoded;
/// timestamps still refer to the full source. A non-finite end means "to the end".
///
/// A source with no audio track is not an error: it returns an empty transcript whose
/// `model` is [`NO_AUDIO_MODEL`], so the caller can fall back to visual-only planning.
pub async fn transcribe(
    video: &Path,
    cfg: &EngineConfig,
    range: Option<(f64, f64)>,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> Result<Transcript> {
    if !video.is_file() {
        return Err(EngineError::msg("transcription failed: source video not found"));
    }
    let (start, end) = match range {
        Some((s, e)) => (Some(s), if e.is_finite() { Some(e) } else { None }),
        None => (None, None),
    };
    let (window_start, window_end) = transcription_window(start, end);

    report(progress, "Transcribing audio...".into());
    let samples = match extract_audio(cfg, video, window_start, window_end, cancel).await? {
        Some(samples) => samples,
        None => {
            tracing::info!("source has no audio track; returning an empty transcript");
            return Ok(Transcript { model: NO_AUDIO_MODEL.into(), ..Default::default() });
        }
    };
    let duration = audio_duration_seconds(samples.len())?;

    let model_path = cfg.whisper_model_path.clone();
    ensure_model(&model_path)?;
    let prompt = initial_prompt(&cfg.custom_vocabulary);

    let turn = match TURN.try_lock() {
        Ok(turn) => turn,
        Err(_) => {
            report(progress, "Waiting for another job to finish transcribing...".into());
            tokio::select! {
                turn = TURN.lock() => turn,
                _ = cancel.cancelled() => return Err(EngineError::Cancelled),
            }
        }
    };
    report(progress, "Transcribing audio on this computer...".into());
    let raw = {
        let progress = progress.clone();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            // Released when whisper.cpp is done, even if this job's future is dropped.
            let _turn = turn;
            run_whisper(&model_path, samples, prompt, progress, cancel)
        })
            .await
            .map_err(|_| EngineError::msg("local transcription failed"))??
    };
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }

    let words = words_from_whisper(&raw);
    Ok(build_transcript(words, window_start, duration, whisper_model_label(&cfg.whisper_model_path)))
}

/// Starts loading the model on a background thread so it is ready by the time the
/// audio is; a job calls this before downloading its source.
pub fn preload_model(path: &Path) {
    if path.is_file() {
        let path = path.to_path_buf();
        std::thread::spawn(move || load_model(&path));
    }
}

/// `ggml-large-v3-turbo.bin` -> `whisper.cpp large-v3-turbo`.
pub fn whisper_model_label(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = name.strip_prefix("ggml-").unwrap_or(&name);
    let name = name.strip_suffix(".bin").unwrap_or(name);
    format!("whisper.cpp {}", if name.is_empty() { "model" } else { name })
}

// ---------------------------------------------------------------------------
// Pure helpers (transcript building)
// ---------------------------------------------------------------------------

/// Source window to decode: pad the requested range by five seconds either side.
/// Returns (start, optional end) in seconds.
fn transcription_window(start: Option<f64>, end: Option<f64>) -> (f64, Option<f64>) {
    let mut window_start = 0.0;
    if let Some(s) = start {
        if s > 0.0 {
            window_start = (s - TRANSCRIPTION_RANGE_PAD_SECONDS).max(0.0);
        }
    }
    let window_end = end.map(|e| window_start.max(e + TRANSCRIPTION_RANGE_PAD_SECONDS));
    (window_start, window_end)
}

/// Clean vocabulary hints before passing them to whisper.
///
/// Trims and collapses whitespace, truncates to under 50 chars, drops empties, terms
/// with forbidden characters or more than five words, and dedupes case-insensitively.
pub fn normalize_keyterms(keyterms: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for term in keyterms {
        let collapsed = term.split_whitespace().collect::<Vec<_>>().join(" ");
        let truncated: String = collapsed.chars().take(KEYTERM_MAX_CHARS).collect();
        let trimmed = truncated.trim();
        if trimmed.is_empty() || trimmed.chars().any(|c| KEYTERM_FORBIDDEN_CHARS.contains(&c)) {
            continue;
        }
        if trimmed.split_whitespace().count() > KEYTERM_MAX_WORDS {
            continue;
        }
        if !seen.insert(trimmed.to_lowercase()) {
            continue;
        }
        normalized.push(trimmed.to_string());
        if normalized.len() >= KEYTERM_MAX_COUNT {
            break;
        }
    }
    normalized
}

/// whisper.cpp initial prompt for the custom vocabulary, if any survives normalizing.
fn initial_prompt(vocabulary: &[String]) -> Option<String> {
    let phrases = normalize_keyterms(vocabulary);
    (!phrases.is_empty()).then(|| format!("Vocabulary: {}", phrases.join(", ")))
}

/// Raw whisper.cpp segments (t0, t1 in 10 ms units, text) -> words.
///
/// With `max_len = 1` each segment is one word. Non-speech markers such as
/// `[BLANK_AUDIO]` or `(laughter)` are not words and never reach captions.
fn words_from_whisper(raw: &[(i64, i64, String)]) -> Vec<TranscriptWord> {
    raw.iter()
        .filter_map(|(t0, t1, text)| {
            let text = text.trim();
            let first = text.chars().next()?;
            let last = text.chars().last()?;
            if "[(".contains(first) && "])".contains(last) {
                return None;
            }
            Some(TranscriptWord { word: text.to_string(), start_time_ms: t0 * 10, end_time_ms: (*t0).max(*t1) * 10 })
        })
        .collect()
}

/// Shift words onto the source timeline and group them into a transcript.
fn build_transcript(words: Vec<TranscriptWord>, offset_seconds: f64, duration: f64, model: String) -> Transcript {
    let shift_ms = (offset_seconds * 1000.0).round_ties_even() as i64;
    let shifted = words
        .into_iter()
        .map(|w| TranscriptWord { start_time_ms: w.start_time_ms + shift_ms, end_time_ms: w.end_time_ms + shift_ms, ..w })
        .collect();
    let segments = group_words(shifted);
    let full_text = segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
    Transcript { segments, full_text, language: None, duration_seconds: Some(duration), model }
}

fn ends_sentence(word: &str) -> bool {
    word.trim_end().chars().last().is_some_and(|c| SENTENCE_END_PUNCTUATION.contains(&c))
}

/// Longest transcript line; unpunctuated speech is split here.
const MAX_SEGMENT_MS: i64 = 15_000;

/// Group timed words into sentence segments; a pause over 2 s or 15 s of speech also ends one.
pub fn group_words(words: Vec<TranscriptWord>) -> Vec<TranscriptSegment> {
    fn flush(segments: &mut Vec<TranscriptSegment>, current: &mut Vec<TranscriptWord>) {
        if current.is_empty() {
            return;
        }
        let words = std::mem::take(current);
        segments.push(TranscriptSegment {
            start_time_ms: words[0].start_time_ms,
            end_time_ms: words[words.len() - 1].end_time_ms,
            text: words.iter().map(|w| w.word.as_str()).collect::<Vec<_>>().join(" "),
            speaker_label: None,
            words,
            audio_events: Vec::new(),
        });
    }
    let mut segments = Vec::new();
    let mut current: Vec<TranscriptWord> = Vec::new();
    for word in words {
        if let Some(prev) = current.last() {
            if word.start_time_ms - prev.end_time_ms > 2000 {
                flush(&mut segments, &mut current);
            }
        }
        let ends = word.word.chars().last().is_some_and(|c| SENTENCE_END_PUNCTUATION.contains(&c));
        // Unpunctuated speech (common in casual non-English) would otherwise run for
        // minutes in one line, and the planner can only cut on line boundaries.
        // ponytail: hard cut mid-phrase; split at the widest pause if seams read badly.
        let too_long = current.first().is_some_and(|first| word.end_time_ms - first.start_time_ms >= MAX_SEGMENT_MS);
        current.push(word);
        if ends || too_long {
            flush(&mut segments, &mut current);
        }
    }
    flush(&mut segments, &mut current);
    segments
}

fn ensure_model(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || !path.is_file() {
        return Err(EngineError::msg("whisper model is not downloaded"));
    }
    Ok(())
}

fn audio_duration_seconds(samples: usize) -> Result<f64> {
    let duration = samples as f64 / SAMPLE_RATE as f64;
    if !(duration.is_finite() && duration > 0.0 && duration <= MAX_AUDIO_SECONDS) {
        return Err(EngineError::msg("audio duration could not be determined"));
    }
    Ok(duration)
}

// ---------------------------------------------------------------------------
// Sentence-boundary helpers (used by the planner)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

/// Nearest sentence end after (Forward) or before (Backward) `timestamp_ms`.
///
/// Forward first takes the nearest sentence end within `tolerance_ms` either side of
/// the timestamp (absorbs display rounding). Returns `timestamp_ms` unchanged when no
/// boundary lies within `max_extension_ms`. Defaults in Python: 5000 ms, Forward,
/// [`SENTENCE_END_TOLERANCE_MS`].
pub fn find_sentence_end_boundary(
    segments: &[TranscriptSegment],
    timestamp_ms: i64,
    max_extension_ms: i64,
    direction: SearchDirection,
    tolerance_ms: i64,
) -> i64 {
    if segments.is_empty() {
        return timestamp_ms;
    }
    let mut candidates: Vec<(&str, i64)> = Vec::new();
    for segment in segments {
        let (seg_start, seg_end) = (segment.start_time_ms, segment.end_time_ms);
        match direction {
            SearchDirection::Forward => {
                if seg_end < timestamp_ms - tolerance_ms {
                    continue;
                }
                if seg_start > timestamp_ms + max_extension_ms {
                    break;
                }
            }
            SearchDirection::Backward => {
                if seg_start > timestamp_ms || seg_end < timestamp_ms - max_extension_ms {
                    continue;
                }
            }
        }
        if !segment.words.is_empty() {
            candidates.extend(segment.words.iter().map(|w| (w.word.as_str(), w.end_time_ms)));
        } else {
            let text = segment.text.trim();
            if !text.is_empty() {
                candidates.push((text, seg_end));
            }
        }
    }
    if candidates.is_empty() {
        return timestamp_ms;
    }
    match direction {
        SearchDirection::Forward => {
            let near = candidates
                .iter()
                .filter(|(word, end)| (end - timestamp_ms).abs() <= tolerance_ms && ends_sentence(word))
                .map(|(_, end)| ((end - timestamp_ms).abs(), *end))
                .min();
            if let Some((_, end)) = near {
                return end;
            }
            for (word, end) in &candidates {
                if *end < timestamp_ms {
                    continue;
                }
                if *end > timestamp_ms + max_extension_ms {
                    return timestamp_ms;
                }
                if ends_sentence(word) {
                    return *end;
                }
            }
        }
        SearchDirection::Backward => {
            let last = candidates
                .iter()
                .filter(|(word, end)| {
                    *end <= timestamp_ms && *end >= timestamp_ms - max_extension_ms && ends_sentence(word)
                })
                .map(|(_, end)| *end)
                .next_back();
            if let Some(end) = last {
                return end;
            }
        }
    }
    timestamp_ms
}

/// End time of the last sentence finishing inside `[earliest_ms, latest_ms]`, if any.
///
/// Used to shorten an over-long clip to its last complete sentence.
pub fn last_sentence_end_between(segments: &[TranscriptSegment], earliest_ms: i64, latest_ms: i64) -> Option<i64> {
    let mut last = None;
    for segment in segments {
        if segment.end_time_ms < earliest_ms {
            continue;
        }
        if segment.start_time_ms > latest_ms {
            break;
        }
        let timed: Vec<(&str, i64)> = if segment.words.is_empty() {
            vec![(segment.text.as_str(), segment.end_time_ms)]
        } else {
            segment.words.iter().map(|w| (w.word.as_str(), w.end_time_ms)).collect()
        };
        for (word, end) in timed {
            if earliest_ms <= end && end <= latest_ms && ends_sentence(word) {
                last = Some(end);
            }
        }
    }
    last
}

/// Where a clip that can't start before `earliest_ms` should start instead.
///
/// The first sentence starting within `sentence_within_ms` after `earliest_ms`,
/// otherwise the first word starting at or after it (so the clip at least doesn't
/// open mid-word). None when nothing starts after `earliest_ms`.
pub fn next_start_at_or_after(segments: &[TranscriptSegment], earliest_ms: i64, sentence_within_ms: i64) -> Option<i64> {
    let mut first_word: Option<i64> = None;
    for segment in segments {
        if segment.end_time_ms < earliest_ms {
            continue;
        }
        if segment.start_time_ms >= earliest_ms {
            if segment.start_time_ms <= earliest_ms + sentence_within_ms || first_word.is_none() {
                return Some(segment.start_time_ms);
            }
            return first_word;
        }
        // This sentence straddles earliest_ms: remember its first word after it.
        if first_word.is_none() {
            first_word = segment.words.iter().map(|w| w.start_time_ms).find(|&s| s >= earliest_ms);
        }
    }
    first_word
}

/// Snap a clip start to the start of the word/sentence containing `timestamp_ms`, so
/// clips never open mid-word. Python default for `max_adjustment_ms`: 3000.
pub fn find_sentence_start_boundary(segments: &[TranscriptSegment], timestamp_ms: i64, max_adjustment_ms: i64) -> i64 {
    if segments.is_empty() {
        return timestamp_ms;
    }
    let mut all_words: Vec<(&str, i64, i64)> = Vec::new();
    for segment in segments {
        if segment.end_time_ms < timestamp_ms - max_adjustment_ms {
            continue;
        }
        if segment.start_time_ms > timestamp_ms + 1000 {
            break;
        }
        if !segment.words.is_empty() {
            all_words.extend(segment.words.iter().map(|w| (w.word.as_str(), w.start_time_ms, w.end_time_ms)));
        } else {
            all_words.push((segment.text.as_str(), segment.start_time_ms, segment.end_time_ms));
        }
    }
    if all_words.is_empty() {
        return timestamp_ms;
    }

    let mut best_start = None;
    let mut best_is_sentence_start = false;
    for (i, &(_, start, end)) in all_words.iter().enumerate() {
        if start <= timestamp_ms && timestamp_ms <= end {
            return start; // mid-word: snap to its start
        }
        if start < timestamp_ms && end <= timestamp_ms {
            if i > 0 && ends_sentence(all_words[i - 1].0) && timestamp_ms - start <= max_adjustment_ms {
                best_start = Some(start);
                best_is_sentence_start = true;
            }
            if !best_is_sentence_start && timestamp_ms - start <= max_adjustment_ms {
                best_start = Some(start);
            }
        }
    }
    // Timestamp between words: start at the next word when it is close.
    if let Some(&(_, start, _)) = all_words.iter().find(|w| w.1 > timestamp_ms) {
        if start - timestamp_ms <= 500 {
            return start;
        }
    }
    best_start.unwrap_or(timestamp_ms)
}

// ---------------------------------------------------------------------------
// Audio extraction
// ---------------------------------------------------------------------------

fn extract_args(video: &Path, start_seconds: f64, end_seconds: Option<f64>) -> Vec<String> {
    let mut args: Vec<String> = ["-nostdin", "-nostats", "-v", "error", "-y"].iter().map(|s| s.to_string()).collect();
    args.extend(MEDIA_INPUT_OPTIONS.iter().map(|s| s.to_string()));
    if start_seconds > 0.0 {
        args.extend(["-ss".into(), format!("{start_seconds:.3}")]);
    }
    if let Some(end) = end_seconds {
        args.extend(["-t".into(), format!("{:.3}", end - start_seconds)]);
    }
    args.extend(["-i".into(), video.to_string_lossy().into_owned()]);
    args.extend(
        [
            "-map", "0:a:0", // Match the track used by the render graph.
            "-vn",
            // Materialize silence at delayed starts/packet gaps so word times stay on the video's clock.
            "-af", "aresample=16000:async=1:first_pts=0:min_hard_comp=0.001",
            "-acodec", "pcm_f32le", "-ar", "16000", "-ac", "1",
            "-f", "f32le", "pipe:1",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    args
}

fn media_command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear();
    for (key, value) in std::env::vars_os() {
        if ALLOWED_ENV.iter().any(|k| key.to_string_lossy().eq_ignore_ascii_case(k)) {
            cmd.env(key, value);
        }
    }
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

/// Decodes the source's first audio track to 16 kHz mono f32 samples.
/// Returns `Ok(None)` when ffprobe confirms the source has no audio stream.
async fn extract_audio(
    cfg: &EngineConfig,
    video: &Path,
    start_seconds: f64,
    end_seconds: Option<f64>,
    cancel: &CancellationToken,
) -> Result<Option<Vec<f32>>> {
    let mut child = media_command(&cfg.ffmpeg)
        .args(extract_args(video, start_seconds, end_seconds))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| EngineError::msg("audio extraction failed"))?;
    let mut stdout = child.stdout.take().ok_or_else(|| EngineError::msg("audio extraction failed"))?;

    let read = async {
        let mut samples: Vec<f32> = Vec::new();
        let mut buf = vec![0u8; 1 << 16];
        let mut carry: Vec<u8> = Vec::with_capacity(4);
        loop {
            let n = stdout.read(&mut buf).await.map_err(|_| EngineError::msg("audio extraction failed"))?;
            if n == 0 {
                break;
            }
            let mut chunk = &buf[..n];
            if !carry.is_empty() {
                let need = 4 - carry.len();
                let take = need.min(chunk.len());
                carry.extend_from_slice(&chunk[..take]);
                chunk = &chunk[take..];
                if carry.len() == 4 {
                    samples.push(f32::from_le_bytes([carry[0], carry[1], carry[2], carry[3]]));
                    carry.clear();
                }
            }
            let whole = chunk.len() / 4 * 4;
            samples.extend(chunk[..whole].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
            carry.extend_from_slice(&chunk[whole..]);
            if samples.len() > MAX_SAMPLES {
                return Err(EngineError::msg("audio duration could not be determined"));
            }
        }
        let status = child.wait().await.map_err(|_| EngineError::msg("audio extraction failed"))?;
        Ok((samples, status.success()))
    };

    let (samples, ok) = tokio::select! {
        r = tokio::time::timeout(MEDIA_TIMEOUT, read) => match r {
            Ok(r) => r?,
            Err(_) => return Err(EngineError::msg("audio extraction failed")),
        },
        _ = cancel.cancelled() => return Err(EngineError::Cancelled),
    };
    if !ok {
        // An audio-less video is valid input for visual-only planning. Confirm that
        // case with ffprobe; other ffmpeg errors must fail.
        if has_no_audio_stream(cfg, video, cancel).await? {
            return Ok(None);
        }
        return Err(EngineError::msg("audio extraction failed"));
    }
    Ok(Some(samples))
}

async fn has_no_audio_stream(cfg: &EngineConfig, video: &Path, cancel: &CancellationToken) -> Result<bool> {
    let mut args: Vec<String> = ["-v", "error", "-select_streams", "a", "-show_entries", "stream=index", "-of", "csv=p=0"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.extend(MEDIA_INPUT_OPTIONS.iter().map(|s| s.to_string()));
    args.push(video.to_string_lossy().into_owned());
    match tokio::time::timeout(PROBE_TIMEOUT, media::run(&cfg.ffprobe, &args, cancel)).await {
        Ok(Ok(out)) => Ok(out.status.success() && String::from_utf8_lossy(&out.stdout).trim().is_empty()),
        Ok(Err(EngineError::Cancelled)) => Err(EngineError::Cancelled),
        _ => Ok(false),
    }
}

// ---------------------------------------------------------------------------
// whisper.cpp
// ---------------------------------------------------------------------------

fn report(progress: &ProgressFn, step: String) {
    let progress = progress.clone();
    // A failing progress sink must not fail transcription.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        progress(Progress { stage: Stage::Transcribing, percent: TRANSCRIBING_PERCENT, step, clips_done: 0, clips_total: 0 })
    }));
}

/// Runs whisper.cpp with one word per segment (max_len = 1 + split_on_word).
/// Returns raw segments as (t0, t1 in 10 ms units, text). Blocking.
fn run_whisper(
    model_path: &Path,
    samples: Vec<f32>,
    prompt: Option<String>,
    progress: ProgressFn,
    cancel: CancellationToken,
) -> Result<Vec<(i64, i64, String)>> {
    use whisper_rs::{FullParams, SamplingStrategy};

    let failed = || EngineError::msg("local transcription failed");
    let ctx = load_model(model_path)?;
    // Speech only when the VAD model is installed; `map` leads its times back to the source.
    let (samples, map) = match speech_spans(&model_path.with_file_name(VAD_MODEL_FILE), &samples) {
        Some(spans) => {
            let joined = join_speech(&samples, &spans);
            tracing::info!("VAD kept {:.0} s of {:.0} s of audio", joined.0.len() as f64 / SAMPLE_RATE as f64, samples.len() as f64 / SAMPLE_RATE as f64);
            drop(samples);
            joined
        }
        None => (samples, Vec::new()),
    };
    if samples.is_empty() {
        return Ok(Vec::new()); // no speech at all
    }
    let mut state = ctx.create_state().map_err(|_| failed())?;

    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 5 });
    params.set_n_threads(threads.saturating_sub(1).clamp(1, 8) as c_int);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_token_timestamps(true);
    params.set_max_len(1);
    params.set_split_on_word(true);
    params.set_language(Some("auto"));
    if let Some(prompt) = prompt.as_deref() {
        params.set_initial_prompt(prompt);
    }

    // whisper-rs 0.16's abort trampoline is only sound when the closure type is itself
    // `Box<dyn FnMut() -> bool>`, so box it explicitly.
    let abort_token = cancel.clone();
    let abort: Box<dyn FnMut() -> bool> = Box::new(move || abort_token.is_cancelled());
    params.set_abort_callback_safe(abort);
    let mut reported = 0;
    params.set_progress_callback_safe(move |percent: i32| {
        let percent = percent.min(99);
        if percent >= reported + 5 {
            reported = percent;
            report(&progress, format!("Transcribing audio on this computer... {percent}%"));
        }
    });

    let result = state.full(params, &samples);
    drop(samples);
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    if result.is_err() {
        tracing::error!("whisper.cpp transcription failed");
        return Err(failed());
    }

    let mut raw = Vec::new();
    for segment in state.as_iter() {
        let text = segment.to_str_lossy().map_err(|_| failed())?.into_owned();
        let (t0, t1) = source_times(segment.start_timestamp(), segment.end_timestamp(), &map);
        raw.push((t0, t1, text));
    }
    Ok(raw)
}

/// The cached model for `path`, loading it (and dropping another) when needed.
fn load_model(path: &Path) -> Result<Arc<WhisperContext>> {
    // Held while loading, so a job waits for a preload in flight instead of loading twice.
    let mut cached = MODEL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((loaded, ctx)) = cached.as_ref() {
        if loaded == path {
            return Ok(ctx.clone());
        }
    }
    *cached = None;
    whisper_rs::install_logging_hooks();
    // Flash attention: about a quarter faster on the GPU (transcribe_bench), word timing as good.
    let mut params = whisper_rs::WhisperContextParameters::default();
    params.flash_attn(true);
    let ctx = WhisperContext::new_with_params(path, params).map_err(|_| {
        tracing::error!("whisper.cpp could not load the model");
        EngineError::msg("local transcription failed")
    })?;
    let ctx = Arc::new(ctx);
    *cached = Some((path.to_path_buf(), ctx.clone()));
    // ggml frees its Metal device at exit and aborts if a model still holds GPU memory then;
    // registered after the first load, this runs before that.
    #[cfg(target_os = "macos")]
    {
        extern "C" fn unload() {
            if let Ok(mut cached) = MODEL.try_lock() {
                *cached = None;
            }
        }
        static UNLOAD_AT_EXIT: std::sync::Once = std::sync::Once::new();
        UNLOAD_AT_EXIT.call_once(|| unsafe {
            libc::atexit(unload);
        });
    }
    Ok(ctx)
}

/// Speech as (start, end) sample ranges, padded, with pauses up to
/// [`VAD_MAX_PAUSE_SAMPLES`] kept inside them. None when the VAD model is missing or
/// fails, which decodes everything.
fn speech_spans(vad_model: &Path, samples: &[f32]) -> Option<Vec<(usize, usize)>> {
    use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};
    // whisper.cpp's built-in VAD (FullParams::enable_vad) only runs in whisper_full,
    // not in the whisper_full_with_state whisper-rs calls, so it is applied here.
    if !vad_model.is_file() {
        return None;
    }
    // One thread: the model runs once per 32 ms of audio, and syncing more threads that
    // often costs far more than the work (minutes for five minutes of audio on a busy CPU).
    let mut context = WhisperVadContextParams::default();
    context.set_n_threads(1);
    let mut vad = WhisperVadContext::new(vad_model.to_str()?, context).ok()?;
    let mut params = WhisperVadParams::default();
    params.set_speech_pad(VAD_SPEECH_PAD_MS);
    let segments = vad.segments_from_samples(params, samples).ok()?;
    let at = |cs: f32| ((cs as f64 * (SAMPLE_RATE / 100) as f64).round() as usize).min(samples.len());
    // The audio's own edges count as speech, so audio without a long pause is decoded as is.
    let mut spans: Vec<(usize, usize)> = vec![(0, 0)];
    for segment in segments {
        let (start, end) = (at(segment.start), at(segment.end));
        match spans.last_mut() {
            Some(last) if start <= last.1 + VAD_MAX_PAUSE_SAMPLES => last.1 = last.1.max(end),
            _ => spans.push((start, end)),
        }
    }
    if let Some(last) = spans.last_mut().filter(|last| samples.len() <= last.1 + VAD_MAX_PAUSE_SAMPLES) {
        last.1 = samples.len();
    }
    spans.retain(|&(start, end)| start < end);
    Some(spans)
}

/// The speech spans back to back, [`VAD_MAX_PAUSE_SAMPLES`] of silence between them,
/// and per span its (start in that audio, start in the source audio, length) in samples.
fn join_speech(samples: &[f32], spans: &[(usize, usize)]) -> (Vec<f32>, Vec<(usize, usize, usize)>) {
    let mut speech = Vec::new();
    let mut map = Vec::new();
    for &(start, end) in spans {
        if !speech.is_empty() {
            speech.resize(speech.len() + VAD_MAX_PAUSE_SAMPLES, 0.0);
        }
        map.push((speech.len(), start, end - start));
        speech.extend_from_slice(&samples[start..end]);
    }
    (speech, map)
}

/// A word's whisper times (10 ms units) in the joined speech, on the source timeline.
///
/// whisper starts the first word after a pause where the word before it ended, and
/// often times all of it in the silence. Speech before a pause ends inside its padded
/// span, so a time in the silence between spans belongs to the next span, and a word
/// starts no earlier than the span it ends in.
fn source_times(t0: i64, t1: i64, map: &[(usize, usize, usize)]) -> (i64, i64) {
    const PER_TICK: usize = SAMPLE_RATE / 100;
    if map.is_empty() {
        return (t0, t1);
    }
    // (span, source sample) of a time.
    let locate = |t: i64| {
        let at = t.max(0) as usize * PER_TICK;
        let i = map.partition_point(|&(joined, ..)| joined <= at).saturating_sub(1);
        let (joined, source, len) = map[i];
        match map.get(i + 1) {
            _ if at < joined + len => (i, source + at - joined),
            Some(&(_, next, _)) => (i + 1, next),
            None => (i, source + len),
        }
    };
    let ((i0, start), (i1, end)) = (locate(t0), locate(t1));
    let start = if i1 > i0 { map[i1].1 } else { start };
    ((start / PER_TICK) as i64, (end / PER_TICK) as i64)
}

// ---------------------------------------------------------------------------
// Tests (ported from engine/tests/test_transcription.py + sentence helpers)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn w(word: &str, start: i64, end: i64) -> TranscriptWord {
        TranscriptWord { word: word.into(), start_time_ms: start, end_time_ms: end }
    }

    #[test]
    fn whisper_words_land_on_the_source_timeline_and_group_into_sentences() {
        let raw: Vec<(i64, i64, String)> = [
            (0, 40, " So"), (40, 90, " anyway."), (90, 100, " [BLANK_AUDIO]"),
            (500, 540, " Next"), (540, 600, " (music)"), (600, 650, " part"),
        ]
        .iter()
        .map(|(a, b, t)| (*a, *b, t.to_string()))
        .collect();
        let t = build_transcript(words_from_whisper(&raw), 12.5, 10.0, whisper_model_label(Path::new("/m/ggml-small.bin")));
        let words: Vec<_> = t.segments.iter().flat_map(|s| s.words.iter())
            .map(|w| (w.word.as_str(), w.start_time_ms, w.end_time_ms)).collect();
        assert_eq!(words, vec![("So", 12500, 12900), ("anyway.", 12900, 13400), ("Next", 17500, 17900), ("part", 18500, 19000)]);
        let texts: Vec<_> = t.segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["So anyway.", "Next part"]);
        assert_eq!(t.full_text, "So anyway. Next part");
        assert_eq!(t.model, "whisper.cpp small");
        assert_eq!(t.duration_seconds, Some(10.0));
        assert!(t.segments.iter().all(|s| s.audio_events.is_empty() && s.speaker_label.is_none()));
    }

    #[test]
    fn end_time_never_precedes_start() {
        let words = words_from_whisper(&[(50, 40, "x".into()), (0, 0, "  ".into())]);
        assert_eq!(words, vec![w("x", 500, 500)]);
    }

    #[test]
    fn vocabulary_becomes_the_initial_prompt() {
        assert_eq!(initial_prompt(&["Acme".into()]).as_deref(), Some("Vocabulary: Acme"));
        assert_eq!(initial_prompt(&["Acme".into(), " acme ".into(), "Zed Co".into()]).as_deref(), Some("Vocabulary: Acme, Zed Co"));
        assert_eq!(initial_prompt(&[]), None);
        assert_eq!(initial_prompt(&["<bad>".into()]), None);
    }

    fn cfg(model: PathBuf) -> EngineConfig {
        EngineConfig {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
            ytdlp: None,
            assets_dir: PathBuf::new(),
            whisper_model_path: model,
            ai: Default::default(),
            custom_vocabulary: vec![],
            work_dir: std::env::temp_dir(),
            output_dir: std::env::temp_dir(),
        }
    }

    #[tokio::test]
    async fn missing_source_fails_without_exposing_the_path() {
        let progress: ProgressFn = Arc::new(|_| {});
        let err = transcribe(Path::new("/nonexistent/secret-dir/video.mp4"), &cfg("/nope/ggml-small.bin".into()), None, &progress, &CancellationToken::new())
            .await
            .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("transcription failed"));
        assert!(!text.contains("secret-dir"));
    }

    #[test]
    fn missing_model_and_decoder_failures_use_their_markers() {
        let progress: ProgressFn = Arc::new(|_| {});
        let err = run_whisper(Path::new("/nonexistent/private/ggml-small.bin"), vec![0.0; 16000], None, progress, CancellationToken::new()).unwrap_err();
        assert_eq!(err.to_string(), "local transcription failed");
        assert!(!err.to_string().contains("private"));
        let missing = ensure_model(Path::new("/nonexistent/private/missing.bin")).unwrap_err().to_string();
        assert_eq!(missing, "whisper model is not downloaded");
        assert!(ensure_model(Path::new("")).is_err());
    }

    /// Extraction against a real ffmpeg; skipped when ffmpeg/ffprobe are not installed.
    #[tokio::test]
    async fn extracts_windowed_audio_and_detects_silent_sources() {
        let (Ok(ffmpeg), Ok(ffprobe)) = (which::which("ffmpeg"), which::which("ffprobe")) else { return };
        let dir = tempfile::tempdir().unwrap();
        let mut c = cfg(dir.path().join("ggml-missing.bin"));
        c.ffmpeg = ffmpeg.clone();
        c.ffprobe = ffprobe;
        let make = |name: &str, extra: &[&str]| {
            let out = dir.path().join(name);
            let mut args = vec!["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=black:s=64x64:d=12"];
            args.extend_from_slice(extra);
            args.extend(["-c:v", "libx264", "-t", "12"]);
            let ok = std::process::Command::new(&ffmpeg).args(&args).arg(&out).status().map(|s| s.success()).unwrap_or(false);
            ok.then_some(out)
        };
        let Some(with_audio) = make("a.mkv", &["-f", "lavfi", "-i", "sine=f=440:d=12", "-c:a", "aac"]) else { return };
        let Some(silent) = make("v.mkv", &[]) else { return };
        let cancel = CancellationToken::new();

        let samples = extract_audio(&c, &with_audio, 5.0, Some(8.0), &cancel).await.unwrap().unwrap();
        assert!((samples.len() as f64 / 16000.0 - 3.0).abs() < 0.1, "{} samples", samples.len());
        assert!(extract_audio(&c, &silent, 0.0, None, &cancel).await.unwrap().is_none());

        let progress: ProgressFn = Arc::new(|_| {});
        let t = transcribe(&silent, &c, None, &progress, &cancel).await.unwrap();
        assert_eq!(t.model, NO_AUDIO_MODEL);
        assert!(t.segments.is_empty());
        let err = transcribe(&with_audio, &c, Some((6.0, 7.0)), &progress, &cancel).await.unwrap_err();
        assert_eq!(err.to_string(), "whisper model is not downloaded");
    }

    #[test]
    fn speech_only_times_map_back_onto_the_source() {
        // Speech at 1-2 s and 5-6 s of a 7 s source; whisper times are in 10 ms units.
        let source: Vec<f32> = (0..7 * 16000).map(|i| i as f32).collect();
        let (speech, map) = join_speech(&source, &[(16000, 32000), (80000, 96000)]);
        assert_eq!(speech.len(), 32000 + VAD_MAX_PAUSE_SAMPLES);
        assert_eq!(speech[16000 + VAD_MAX_PAUSE_SAMPLES], 80000.0);
        let (pause, second) = ((16000 + VAD_MAX_PAUSE_SAMPLES / 2) as i64 / 160, (16000 + VAD_MAX_PAUSE_SAMPLES) as i64 / 160);
        assert_eq!(source_times(0, 50, &map), (100, 150));
        assert_eq!(source_times(second, second + 50, &map), (500, 550));
        assert_eq!(source_times(second + 500, second + 500, &map), (600, 600));
        // The silence between spans belongs to the next one, and a word ending in a
        // span starts in it.
        assert_eq!(source_times(pause, second + 50, &map), (500, 550));
        assert_eq!(source_times(80, pause, &map), (500, 500));
        assert_eq!(source_times(50, second + 50, &map), (500, 550));
        assert_eq!(source_times(42, 50, &[]), (42, 50));
    }

    #[test]
    fn long_pauses_split_segments_even_mid_sentence() {
        let segments = group_words(vec![w("wait", 0, 300), w("for", 2400, 2600), w("it.", 2600, 2900)]);
        let texts: Vec<_> = segments.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["wait", "for it."]);
        assert_eq!((segments[1].start_time_ms, segments[1].end_time_ms), (2400, 2900));
        assert!(group_words(vec![]).is_empty());
    }

    #[test]
    fn unpunctuated_speech_is_split_every_15_seconds() {
        let words = (0..40).map(|i| w("ya", i * 1000, i * 1000 + 800)).collect();
        let lens: Vec<_> = group_words(words).iter().map(|s| s.words.len()).collect();
        assert_eq!(lens, vec![16, 16, 8]);
    }

    #[test]
    fn model_label() {
        assert_eq!(whisper_model_label(Path::new("/x/ggml-large-v3-turbo-q5_0.bin")), "whisper.cpp large-v3-turbo-q5_0");
        assert_eq!(whisper_model_label(Path::new("")), "whisper.cpp model");
    }

    #[test]
    fn keyterms_enforce_limits() {
        let terms: Vec<String> = ["  Clipward  ", "clipward", "one two three four five six", "bad<chars>", &"x".repeat(80), ""]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let cleaned = normalize_keyterms(&terms);
        assert_eq!(cleaned[0], "Clipward");
        assert!(!cleaned.iter().any(|t| t == "clipward"));
        assert!(!cleaned.iter().any(|t| t.contains("six") || t.contains('<')));
        assert!(cleaned.iter().all(|t| t.chars().count() < 50));
        assert_eq!(cleaned.len(), 2);
    }

    #[test]
    fn keyterms_cap_count() {
        let terms: Vec<String> = (0..1500).map(|i| format!("term{i}")).collect();
        assert_eq!(normalize_keyterms(&terms).len(), 200);
    }

    #[test]
    fn window_pads_the_requested_range() {
        assert_eq!(transcription_window(None, None), (0.0, None));
        assert_eq!(transcription_window(Some(3.0), Some(20.0)), (0.0, Some(25.0)));
        assert_eq!(transcription_window(Some(60.0), Some(90.0)), (55.0, Some(95.0)));
        assert_eq!(transcription_window(Some(0.0), Some(10.0)), (0.0, Some(15.0)));
        let args = extract_args(Path::new("in.mkv"), 55.0, Some(95.0));
        let joined = args.join(" ");
        assert!(joined.contains("-protocol_whitelist file,pipe,fd -format_whitelist mov,matroska,webm,avi,flv,mpegts -ss 55.000 -t 40.000 -i in.mkv"));
        assert!(joined.ends_with("-ar 16000 -ac 1 -f f32le pipe:1"));
        assert!(!extract_args(Path::new("in.mkv"), 0.0, None).iter().any(|a| a == "-ss" || a == "-t"));
    }

    #[test]
    fn audio_duration_bounds() {
        assert!(audio_duration_seconds(0).unwrap_err().to_string().contains("audio duration could not be determined"));
        assert_eq!(audio_duration_seconds(16000).unwrap(), 1.0);
        assert!(audio_duration_seconds(MAX_SAMPLES + 16000).is_err());
    }

    /// Port of test_plan_boundaries.make_transcript.
    fn make_transcript(n_sentences: usize) -> Vec<TranscriptSegment> {
        let (words_per, word_ms, gap_ms, pause_ms) = (12, 300, 80, 400);
        let mut t = 1000;
        (0..n_sentences)
            .map(|s| {
                let words: Vec<_> = (0..words_per)
                    .map(|i| {
                        let text = format!("w{s}_{i}{}", if i == words_per - 1 { "." } else { "" });
                        let word = w(&text, t, t + word_ms);
                        t += word_ms + gap_ms;
                        word
                    })
                    .collect();
                t += pause_ms;
                TranscriptSegment {
                    start_time_ms: words[0].start_time_ms,
                    end_time_ms: words[words.len() - 1].end_time_ms,
                    text: words.iter().map(|w| w.word.as_str()).collect::<Vec<_>>().join(" "),
                    speaker_label: Some("S1".into()),
                    words,
                    audio_events: vec![],
                }
            })
            .collect()
    }

    #[test]
    fn nearest_end_within_tolerance_wins() {
        let tr = make_transcript(5);
        let end = tr[1].end_time_ms;
        let find = |ts| find_sentence_end_boundary(&tr, ts, 5000, SearchDirection::Forward, SENTENCE_END_TOLERANCE_MS);
        assert_eq!(find(end + 40), end);
        assert_eq!(find(end - 40), end);
        assert_eq!(find(end - 1000), end);
        assert_eq!(find_sentence_end_boundary(&tr, end + 1000, 5000, SearchDirection::Backward, SENTENCE_END_TOLERANCE_MS), end);
    }

    #[test]
    fn last_sentence_end_between_finds_the_last_fitting_end() {
        let tr = make_transcript(5);
        let ends: Vec<i64> = tr.iter().map(|s| s.words.last().unwrap().end_time_ms).collect();
        assert_eq!(last_sentence_end_between(&tr, ends[0], ends[2] + 100), Some(ends[2]));
        assert_eq!(last_sentence_end_between(&tr, ends[2] + 1, ends[3] - 1), None);
    }

    #[test]
    fn start_helpers_avoid_mid_word_starts() {
        let tr = make_transcript(5);
        let mid = tr[2].words[3].start_time_ms + 100;
        assert_eq!(find_sentence_start_boundary(&tr, mid, 3000), tr[2].words[3].start_time_ms);
        // Near a sentence start: jump to it; far: the next word.
        assert_eq!(next_start_at_or_after(&tr, tr[2].words[10].start_time_ms + 1, 4000), Some(tr[3].start_time_ms));
        assert_eq!(next_start_at_or_after(&tr, mid, 1000), Some(tr[2].words[4].start_time_ms));
        assert_eq!(next_start_at_or_after(&tr, tr[4].end_time_ms + 1, 1000), None);
    }
}
