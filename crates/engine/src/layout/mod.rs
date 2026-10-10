//! Layout analyzer: decides how each shot of a clip should be framed for 9:16.
//! Port of layout_analyzer.py.
//!
//! For a clip it:
//! 1. Decodes low-res frames (ANALYSIS_FPS) with FFmpeg into memory.
//! 2. Detects faces per frame (YuNet on tract-onnx) and shot cuts (HSV histogram jumps),
//!    and reads each face's patch for the active-speaker signal.
//! 3. Tracks faces within each shot and classifies the shot's layout
//!    (talking_head / two_shot / screen_cam / screen).
//! 4. Optionally asks a vision LLM, once per distinct setup, to confirm the
//!    layout and return the real webcam / screen overlay rectangles.
//!
//! Every step degrades gracefully: without the face model `analyze_layout`
//! returns `Ok(None)` (letterbox render); without the vision model the
//! heuristic classification is used.
pub mod analysis;
pub mod imgproc;
pub mod types;
pub mod vision;
pub mod yunet;

pub use analysis::{
    analysis_dimensions, apply_style, box_from_1000, classify_shot, estimate_cam_box, face_from_person_box,
    follow_speaker, follow_webcam, is_corner_overlay, is_still_media, merge_adjacent, merge_vision_result, screen_box_excluding_cam, segment_shots,
    smooth_focus_path, speaker_turns, split_overlay_segments, track_faces, transfer_cam_box, FaceTrack, FrameInfo, STYLE_AUTO,
    MEDIA_SOURCE, STYLE_FILL, STYLE_FIT,
};
pub use types::*;
pub use vision::{Keyframe, VisionCache};

use crate::error::{EngineError, Result};
use crate::media;
use crate::types::{AiSettings, EngineConfig};
use analysis::*;
use futures::{StreamExt, TryStreamExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use yunet::YuNet;

pub const YUNET_MODEL_FILE: &str = "face_detection_yunet_2023mar.onnx";
const DECODE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// What to analyze: the render window [start_ms, start_ms + duration_ms) of a source.
#[derive(Debug, Clone)]
pub struct LayoutRequest {
    pub video_path: PathBuf,
    pub start_ms: i64,
    pub duration_ms: i64,
    pub src_w: u32,
    pub src_h: u32,
    /// "auto" | "fill" | "fit"
    pub style: String,
    /// Ask the vision model (the job's `layout_vision` toggle). False = heuristics
    /// only, used when the plan only informs pacing.
    pub vision: bool,
    /// Model for vision calls; None uses `cfg.ai.model`.
    pub vision_model: Option<String>,
}

/// Where the YuNet model lives at runtime.
pub fn yunet_model_path(cfg: &EngineConfig) -> PathBuf {
    cfg.assets_dir.join("models").join(YUNET_MODEL_FILE)
}

/// Smart framing is possible (the face model is installed).
pub fn layout_available(cfg: &EngineConfig) -> bool {
    yunet_model_path(cfg).is_file()
}

/// Plan the framing for the request's render window.
///
/// `Ok(None)` means "use the letterbox" (face model missing, or nothing decoded).
/// Errors are decode failures or limits; callers fall back to the letterbox too.
/// Pass one `VisionCache` per job to reuse vision answers across clips.
pub async fn analyze_layout(
    cfg: &EngineConfig,
    req: &LayoutRequest,
    vision_cache: Option<&VisionCache>,
    cancel: &CancellationToken,
) -> Result<Option<ClipLayoutPlan>> {
    let (src_w, src_h) = (req.src_w, req.src_h);
    if req.style == STYLE_FIT {
        return Ok(Some(ClipLayoutPlan {
            shots: vec![ShotLayout { source: "style".into(), ..analysis::new_shot(0, req.duration_ms, SCREEN) }],
            source_width: src_w,
            source_height: src_h,
            ..Default::default()
        }));
    }
    let model = yunet_model_path(cfg);
    if !model.is_file() {
        tracing::warn!("Layout analysis unavailable (face model missing); using letterbox");
        return Ok(None);
    }

    let ai: AiSettings = match &req.vision_model {
        Some(m) if !m.trim().is_empty() => AiSettings { model: m.clone(), ..cfg.ai.clone() },
        _ => cfg.ai.clone(),
    };
    let use_vision = req.vision && vision::vision_available(&ai);

    let (frames, keyframes) = decode_and_detect(&cfg.ffmpeg, &model, req, use_vision, cancel).await?;
    if frames.is_empty() {
        tracing::warn!("Layout analysis decoded no frames; using letterbox");
        return Ok(None);
    }

    let local_cache;
    let cache = match vision_cache {
        Some(c) => c,
        None => {
            local_cache = VisionCache::new();
            &local_cache
        }
    };

    let crop_w_frac = f64::min(1.0, (src_h as f64 * 9.0 / 16.0) / src_w as f64);
    let mut shots: Vec<ShotLayout> = Vec::new();
    let mut vision_cost = 0.0;

    let cuts = snap_cuts(&cfg.ffmpeg, req, segment_shots(&frames, req.duration_ms), cancel).await?;
    let mut planned = Vec::with_capacity(cuts.len());
    for (shot_start, shot_end) in cuts {
        if cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        let shot_frames: Vec<FrameInfo> =
            frames.iter().filter(|f| shot_start <= f.t_ms && f.t_ms < shot_end).cloned().collect();
        let tracks = track_faces(&shot_frames);
        let (mut shot, main_track) = classify_shot(&tracks, shot_frames.len(), src_w, src_h);

        if let Some(track) = main_track {
            let rel: Vec<(i64, Box)> = track.samples.iter().map(|(t, b)| (t - shot_start, *b)).collect();
            shot.focus_path = smooth_focus_path(&rel, shot_end - shot_start, crop_w_frac);
        }
        planned.push((shot_start, shot_end, shot_frames, tracks, shot));
    }

    let mut answers = vec![None; planned.len()];
    if use_vision {
        let asks: Vec<vision::VisionAsk> = planned
            .iter()
            .map(|(s, e, f, _, shot)| (vision::pick_keyframe(&keyframes, (s + e).div_euclid(2)), f.as_slice(), shot))
            .collect();
        for (answer, (result, cost)) in answers.iter_mut().zip(vision::vision_classify(&ai, &asks, cache, cancel).await?) {
            vision_cost += cost;
            *answer = result;
        }
    }

    for ((shot_start, shot_end, shot_frames, tracks, mut shot), result) in planned.into_iter().zip(answers) {
        if let Some(result) = result {
            let mut refined = merge_vision_result(&shot, &result);
            if refined.layout == TALKING_HEAD && refined.focus_path.is_empty() {
                refined.focus_path = match refined.people.first() {
                    Some(f) => vec![(0, f.cx(), f.cy())],
                    None => vec![(0, 0.5, 0.5)],
                };
            }
            shot = refined;
        }

        // Without vision the webcam box is a guess from the face alone (it assumes a
        // 16:9 cam), and a zoomed guess drags in whatever surrounds the overlay.
        // Show the whole frame instead: the webcam is already in the picture.
        if shot.layout == SCREEN_CAM && shot.source != "vision" {
            shot.detected_layout = Some(SCREEN_CAM.into());
            shot.layout = SCREEN.into();
        }

        // Inserted media (a photo, a screenshot) is shown whole, even with a face in it.
        if is_still_media(&shot_frames) {
            shot = ShotLayout {
                detected_layout: Some(shot.layout.clone()),
                source: MEDIA_SOURCE.into(),
                ..analysis::new_shot(0, 0, SCREEN)
            };
        }

        shot.start_ms = shot_start;
        shot.end_ms = shot_end;
        let sub_shots = if shot.layout == SCREEN_CAM {
            follow_webcam(&shot, &shot_frames, src_w, src_h)
        } else if shot.layout == TALKING_HEAD
            || (shot.layout == SCREEN && shot.source == "heuristic" && req.style != STYLE_FILL)
        {
            follow_speaker(&shot, &tracks, shot_frames.len(), crop_w_frac)
        } else {
            vec![shot]
        };
        shots.extend(sub_shots.into_iter().map(|s| apply_style(s, &req.style)));
    }

    let shots = merge_adjacent(shots);
    tracing::info!(
        "Layout plan: {}{}",
        shots
            .iter()
            .map(|s| format!(
                "{}[{:.1}-{:.1}s,{}]",
                s.layout,
                s.start_ms as f64 / 1000.0,
                s.end_ms as f64 / 1000.0,
                s.source
            ))
            .collect::<Vec<_>>()
            .join(", "),
        if vision_cost > 0.0 { format!(" (vision ${vision_cost:.4})") } else { String::new() }
    );
    Ok(Some(ClipLayoutPlan {
        shots,
        source_width: src_w,
        source_height: src_h,
        vision_cost_usd: vision_cost,
        face_samples: frames.into_iter().map(|f| (f.t_ms, f.faces)).collect(),
    }))
}

// ------------------------------------------------------------------
// Decoding & detection
// ------------------------------------------------------------------

/// How many times the detector's size frames are decoded at (`PATCH_SCALE`
/// when the source has that many pixels, else 1).
pub fn decode_scale(src_w: u32, src_h: u32) -> u32 {
    let (w, h) = analysis_dimensions(src_w, src_h);
    if src_w >= PATCH_SCALE * w && src_h >= PATCH_SCALE * h {
        PATCH_SCALE
    } else {
        1
    }
}

/// FFmpeg arguments that decode the window as raw BGR frames at ANALYSIS_FPS.
pub fn decode_args(video_path: &Path, start_ms: i64, duration_ms: i64, width: u32, height: u32) -> Vec<String> {
    let mut args: Vec<String> =
        ["-nostdin", "-v", "error", "-ss"].iter().map(|s| s.to_string()).collect();
    args.push(format!("{:.3}", start_ms as f64 / 1000.0));
    args.extend(media::MEDIA_INPUT_OPTIONS.iter().map(|s| s.to_string()));
    args.push("-i".into());
    args.push(video_path.to_string_lossy().into_owned());
    args.push("-t".into());
    args.push(format!("{:.3}", duration_ms as f64 / 1000.0));
    args.push("-vf".into());
    // round=up: the frame labelled t is the one showing at t, not up to half a step later.
    args.push(format!("fps={ANALYSIS_FPS:?}:round=up,scale={width}:{height}"));
    args.extend(["-f", "rawvideo", "-pix_fmt", "bgr24", "-"].iter().map(|s| s.to_string()));
    args
}

/// How far around a sampled cut to look for the frame where it really happens
/// (the samples only bracket it within one step).
const CUT_SEARCH_MS: i64 = 200;
/// scdet score the cut frame needs; hard cuts measured 10-20, motion under 2.
const CUT_MIN_SCORE: f64 = 5.0;

/// Move each shot boundary onto the exact frame of its cut. At 4 fps a cut is
/// only known to within 125 ms, and a crop that moves a few frames before the
/// camera cuts reads as a jolt. A cut whose frame isn't found stays put.
async fn snap_cuts(
    ffmpeg: &Path,
    req: &LayoutRequest,
    mut shots: Vec<(i64, i64)>,
    cancel: &CancellationToken,
) -> Result<Vec<(i64, i64)>> {
    // A few cuts at once; each search depends only on its own sampled boundary.
    let found: Vec<Option<i64>> = futures::stream::iter(1..shots.len())
        .map(|i| {
            let from = shots[i].0 - CUT_SEARCH_MS;
            let mut args: Vec<String> = vec!["-nostdin".into(), "-v".into(), "error".into(), "-ss".into()];
            args.push(format!("{:.3}", (req.start_ms + from) as f64 / 1000.0));
            args.extend(media::MEDIA_INPUT_OPTIONS.iter().map(|s| s.to_string()));
            args.extend(["-i".into(), req.video_path.to_string_lossy().into_owned(), "-t".into()]);
            args.push(format!("{:.3}", 2.0 * CUT_SEARCH_MS as f64 / 1000.0));
            args.extend(["-an", "-vf", "scale=64:-2,scdet,metadata=print:file=-", "-f", "null", "-"].map(String::from));
            async move {
                match media::run_bounded(ffmpeg, &args, Duration::from_secs(30), media::MAX_TOOL_OUTPUT_BYTES, cancel).await {
                    Err(EngineError::Cancelled) => Err(EngineError::Cancelled),
                    Ok(out) if out.status.success() => Ok(cut_frame_ms(&String::from_utf8_lossy(&out.stdout)).map(|at| from + at)),
                    _ => Ok(None),
                }
            }
        })
        .buffered(4)
        .try_collect()
        .await?;
    for (i, cut) in (1..).zip(found) {
        if let Some(cut) = cut {
            shots[i - 1].1 = cut;
            shots[i].0 = cut;
        }
    }
    Ok(shots)
}

/// Time (ms into the window) of the frame scdet scores highest, if it is a cut.
pub fn cut_frame_ms(metadata: &str) -> Option<i64> {
    let (mut t, mut best) = (None, None::<(f64, i64)>);
    for line in metadata.lines() {
        if let Some(pts) = line.split("pts_time:").nth(1) {
            t = pts.trim().parse::<f64>().ok();
        } else if let (Some(score), Some(t)) = (line.strip_prefix("lavfi.scd.score="), t) {
            let score: f64 = score.trim().parse().unwrap_or(0.0);
            if score >= CUT_MIN_SCORE && best.is_none_or(|b| score > b.0) {
                best = Some((score, (t * 1000.0).round() as i64));
            }
        }
    }
    best.map(|b| b.1)
}

/// A frame's analysis and, for keyframes, its JPEG.
type Analyzed = (FrameInfo, Option<Vec<u8>>);

/// One decoded frame's analysis, for benches: see `process_frame`.
pub fn frame_info(detector: &YuNet, image: &[u8], width: u32, height: u32, scale: u32, t_ms: i64) -> Result<FrameInfo> {
    process_frame(detector, image, width, height, scale, t_ms, false).map(|r| r.0)
}

/// Face boxes (normalized, score-filtered, at most MAX_FACES_PER_FRAME), face
/// patches, H/S histogram, thumbnail and optional JPEG for one BGR frame
/// `scale` times the detector's `width` x `height`. Blocking.
fn process_frame(
    detector: &YuNet,
    image: &[u8],
    width: u32,
    height: u32,
    scale: u32,
    t_ms: i64,
    keyframe: bool,
) -> Result<Analyzed> {
    let (big_w, big_h) = ((width * scale) as usize, (height * scale) as usize);
    if image.len() != big_w * big_h * 3 {
        return Err(EngineError::msg("face detection model failed: frame size mismatch"));
    }
    // The detector and the shot statistics see the small frame, as before;
    // only face patches are read from the sharper one.
    let small;
    let (patch_src, image) = if scale == 2 {
        small = imgproc::halve_3c(image, big_w, big_h);
        (image, small.as_slice())
    } else {
        (image, image)
    };
    let mut rows = detector.detect(image)?;
    rows.sort_by(|a, b| b[14].partial_cmp(&a[14]).unwrap_or(std::cmp::Ordering::Equal));
    rows.truncate(MAX_FACES_PER_FRAME);
    let (w, h) = (width as f64, height as f64);
    rows.retain(|r| r[14] >= FACE_SCORE_THRESHOLD);
    let faces = rows
        .iter()
        .map(|r| Box { x: r[0] as f64 / w, y: r[1] as f64 / h, w: r[2] as f64 / w, h: r[3] as f64 / h }.clamp())
        .collect();
    let patches = rows
        .iter()
        .map(|r| imgproc::face_patch(patch_src, big_w, big_h, r, scale as f32, MIN_TALK_UNIT_PX))
        .collect();
    let hist = imgproc::hs_histogram(image, width as usize, height as usize);
    let thumb = imgproc::luma_thumb(image, width as usize, height as usize);
    let jpeg = if keyframe {
        let mut out = Vec::new();
        let encoder = jpeg_encoder::Encoder::new(&mut out, 80);
        match encoder.encode(image, width as u16, height as u16, jpeg_encoder::ColorType::Bgr) {
            Ok(()) => Some(out),
            Err(e) => {
                tracing::warn!("Layout keyframe encoding failed: {e}");
                None
            }
        }
    } else {
        None
    };
    Ok((FrameInfo { t_ms, faces, patches, hist, thumb }, jpeg))
}

/// Fill `buf` from `reader`; returns bytes read (short only at EOF).
async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = reader.read(&mut buf[filled..]).await?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

/// Detectors (one core each) running at once across every clip being analysed.
fn detect_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

static DETECT_SLOTS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(detect_threads()));

/// `want_keyframes`: keep JPEG keyframes for the vision model.
async fn decode_and_detect(
    ffmpeg: &Path,
    model: &Path,
    req: &LayoutRequest,
    want_keyframes: bool,
    cancel: &CancellationToken,
) -> Result<(Vec<FrameInfo>, Vec<Keyframe>)> {
    media::validate_video_dimensions(req.src_w, req.src_h)?;
    let (width, height) = analysis_dimensions(req.src_w, req.src_h);
    let scale = decode_scale(req.src_w, req.src_h);
    if !(0 < req.duration_ms && req.duration_ms <= MAX_ANALYSIS_DURATION_MS) || req.start_ms < 0 {
        return Err(EngineError::msg("layout analysis window exceeds supported limits"));
    }
    let frame_limit =
        MAX_ANALYSIS_FRAMES.min((req.duration_ms as f64 / 1000.0 * ANALYSIS_FPS).ceil() as usize + 1);
    let keyframe_every = [1, (ANALYSIS_FPS / KEYFRAME_FPS).round() as usize, frame_limit.div_ceil(MAX_RETAINED_KEYFRAMES)]
        .into_iter()
        .max()
        .unwrap_or(1);

    let model_path = model.to_path_buf();
    let detector = tokio::task::spawn_blocking(move || {
        YuNet::new(&model_path, width as usize, height as usize, FACE_SCORE_THRESHOLD, FACE_NMS_THRESHOLD, FACE_TOP_K)
    })
    .await
    .map_err(|_| EngineError::msg("face detection model failed: worker panicked"))??;

    let mut child = media::media_command(ffmpeg)
        .args(decode_args(&req.video_path, req.start_ms, req.duration_ms, width * scale, height * scale))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = child.stdout.take().ok_or_else(|| EngineError::msg("layout decoding failed"))?;
    let mut stderr = child.stderr.take().ok_or_else(|| EngineError::msg("layout decoding failed"))?;

    // Drain stderr (never let FFmpeg block on it); returns false when it
    // exceeded the diagnostics cap.
    let stderr_task = tokio::spawn(async move {
        let mut total = 0usize;
        let mut chunk = vec![0u8; 65536];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => return total <= media::MAX_TOOL_OUTPUT_BYTES,
                Ok(n) => total = total.saturating_add(n),
            }
        }
    });

    let frame_bytes = (width * scale) as usize * (height * scale) as usize * 3;
    let work = async {
        let mut frames: Vec<FrameInfo> = Vec::new();
        let mut keyframes: Vec<Keyframe> = Vec::new();
        let mut keyframe_bytes = 0usize;
        // Detect several frames at once (results collected in order) while FFmpeg keeps
        // decoding; DETECT_SLOTS caps the detectors running across all clips.
        let in_flight = 2 * detect_threads();
        let mut pending: std::collections::VecDeque<tokio::task::JoinHandle<Result<Analyzed>>> =
            std::collections::VecDeque::with_capacity(in_flight);
        let mut index = 0usize;

        let collect = |frames: &mut Vec<FrameInfo>,
                       keyframes: &mut Vec<Keyframe>,
                       keyframe_bytes: &mut usize,
                       (info, jpeg): Analyzed|
         -> Result<()> {
            if let Some(jpeg) = jpeg {
                if jpeg.len() > MAX_KEYFRAME_BYTES - *keyframe_bytes {
                    return Err(EngineError::msg("layout keyframes exceed the size limit"));
                }
                *keyframe_bytes += jpeg.len();
                keyframes.push(Keyframe { t_ms: info.t_ms, jpeg, hist: info.hist.clone() });
            }
            frames.push(info);
            Ok(())
        };

        loop {
            let mut buf = vec![0u8; frame_bytes];
            let n = read_frame(&mut stdout, &mut buf).await?;
            if n == 0 {
                break;
            }
            if n != frame_bytes {
                return Err(EngineError::msg("incomplete layout analysis frame"));
            }
            if index >= frame_limit {
                return Err(EngineError::msg("layout analysis exceeds the frame limit"));
            }
            let t_ms = (index as f64 * 1000.0 / ANALYSIS_FPS) as i64;
            let is_keyframe = want_keyframes && index.is_multiple_of(keyframe_every);
            let det = detector.clone();
            let slot = DETECT_SLOTS.acquire().await.expect("detector slots are never closed");
            pending.push_back(tokio::task::spawn_blocking(move || {
                let _slot = slot;
                process_frame(&det, &buf, width, height, scale, t_ms, is_keyframe)
            }));
            if pending.len() >= in_flight {
                let prev = pending.pop_front().expect("pending is non-empty");
                let res = prev.await.map_err(|_| EngineError::msg("layout analysis worker panicked"))??;
                collect(&mut frames, &mut keyframes, &mut keyframe_bytes, res)?;
            }
            index += 1;
        }
        while let Some(prev) = pending.pop_front() {
            let res = prev.await.map_err(|_| EngineError::msg("layout analysis worker panicked"))??;
            collect(&mut frames, &mut keyframes, &mut keyframe_bytes, res)?;
        }
        Ok::<_, EngineError>((frames, keyframes))
    };

    let outcome = tokio::select! {
        r = tokio::time::timeout(DECODE_TIMEOUT, work) => match r {
            Ok(r) => r,
            Err(_) => Err(EngineError::msg("media processing exceeded its time limit")),
        },
        _ = cancel.cancelled() => Err(EngineError::Cancelled),
    };
    let (frames, keyframes) = match outcome {
        Ok(v) => v,
        Err(e) => {
            let _ = child.kill().await;
            stderr_task.abort();
            return Err(e);
        }
    };
    let status = tokio::select! {
        s = child.wait() => s?,
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            return Err(EngineError::Cancelled);
        }
    };
    if !stderr_task.await.unwrap_or(true) {
        return Err(EngineError::msg("media diagnostics exceeded the size limit"));
    }
    if !status.success() {
        return Err(EngineError::msg("layout decoding failed"));
    }
    Ok((frames, keyframes))
}

#[cfg(test)]
mod tests;
