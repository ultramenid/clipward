//! Bounded frame sampling for visual-only clip planning (silent videos).
//! Port of `services/visual_clip_sampling.py`.
use crate::media;
use futures::StreamExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const MAX_PLANNING_FRAMES: usize = 24;
pub const MAX_SUPPORT_GAP_SECONDS: f64 = 45.0;
pub const FRAME_WIDTH: u32 = 512;
const FRAME_TIMEOUT: Duration = Duration::from_secs(20);
/// Grayscale thumbnail compared between frames (PIL: convert("L").resize((64, 36))).
const THUMB_W: usize = 64;
const THUMB_H: usize = 36;
const MIN_MEAN_DIFFERENCE: f64 = 3.0;

/// A sampled video frame on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct VisionFrame {
    pub timestamp_ms: i64,
    pub path: PathBuf,
}

/// Up to 24 sample times (seconds) inside [start, end] of the source.
pub fn sample_timestamps(
    duration_seconds: f64,
    start_time: Option<f64>,
    end_time: Option<f64>,
) -> Vec<f64> {
    if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
        return vec![];
    }
    let start = start_time.unwrap_or(0.0).max(0.0);
    let end = duration_seconds.min(end_time.unwrap_or(duration_seconds));
    if !start.is_finite() || !end.is_finite() || end <= start {
        return vec![];
    }
    // At least three samples are needed for the model to see a visual sequence.
    let count = MAX_PLANNING_FRAMES.min(3usize.max(((end - start) / 15.0).ceil() as usize));
    let span = end - start;
    if span / count as f64 <= MAX_SUPPORT_GAP_SECONDS {
        (0..count)
            .map(|i| start + (i as f64 + 0.5) * span / count as f64)
            .collect()
    } else {
        // Uniform samples on long sources are farther apart than one useful short
        // clip. Bounded three-frame windows give the planner local evidence at
        // representative points without increasing API media cost.
        let windows = count / 3;
        (0..windows)
            .flat_map(|w| {
                [-15.0, 0.0, 15.0]
                    .map(move |offset| start + (w as f64 + 0.5) * span / windows as f64 + offset)
            })
            .collect()
    }
}

fn frame_args(video: &Path, output: &Path, timestamp: f64) -> Vec<String> {
    let mut args: Vec<String> = [
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-ss",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(format!("{timestamp:.3}"));
    args.extend(media::MEDIA_INPUT_OPTIONS.iter().map(|s| s.to_string()));
    args.extend([
        "-i".into(),
        video.to_string_lossy().into_owned(),
        "-map".into(),
        "0:v:0".into(),
        "-frames:v".into(),
        "1".into(),
        "-vf".into(),
        format!("scale={FRAME_WIDTH}:-2"),
        "-q:v".into(),
        "7".into(),
        output.to_string_lossy().into_owned(),
    ]);
    args
}

async fn sample_one(
    ffmpeg: &Path,
    video: &Path,
    output: &Path,
    timestamp: f64,
    cancel: &CancellationToken,
) -> bool {
    let args = frame_args(video, output, timestamp);
    match media::run_bounded(
        ffmpeg,
        &args,
        FRAME_TIMEOUT,
        media::MAX_TOOL_OUTPUT_BYTES,
        cancel,
    )
    .await
    {
        Ok(out) if out.status.success() => std::fs::metadata(output)
            .map(|m| m.is_file() && m.len() > 1024)
            .unwrap_or(false),
        _ => false,
    }
}

/// Sample up to 24 source frames within the chosen range into `work_dir`.
pub async fn sample_visual_planning_frames(
    ffmpeg: &Path,
    video: &Path,
    duration_seconds: f64,
    work_dir: &Path,
    start_time: Option<f64>,
    end_time: Option<f64>,
    cancel: &CancellationToken,
) -> Vec<VisionFrame> {
    let timestamps = sample_timestamps(duration_seconds, start_time, end_time);
    if timestamps.is_empty() {
        return vec![];
    }
    let frame_dir = work_dir.join("visual_planning_frames");
    if std::fs::create_dir_all(&frame_dir).is_err() {
        return vec![];
    }
    let frames: Vec<Option<VisionFrame>> =
        futures::stream::iter(timestamps.into_iter().enumerate())
            .map(|(index, timestamp)| {
                let output = frame_dir.join(format!("frame_{index:02}.jpg"));
                async move {
                    if sample_one(ffmpeg, video, &output, timestamp, cancel).await {
                        Some(VisionFrame {
                            timestamp_ms: (timestamp * 1000.0).round_ties_even() as i64,
                            path: output,
                        })
                    } else {
                        tracing::warn!("Visual planning frame unavailable at {timestamp:.1}s");
                        None
                    }
                }
            })
            .buffered(4)
            .collect()
            .await;
    frames.into_iter().flatten().collect()
}

/// 64x36 grayscale pixels of an image file, decoded by ffmpeg.
async fn thumbnail(ffmpeg: &Path, image: &Path, cancel: &CancellationToken) -> Option<Vec<u8>> {
    let args: Vec<String> = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-i".into(),
        image.to_string_lossy().into_owned(),
        "-frames:v".into(),
        "1".into(),
        "-vf".into(),
        format!("scale={THUMB_W}:{THUMB_H}:flags=bicubic,format=gray"),
        "-f".into(),
        "rawvideo".into(),
        "pipe:1".into(),
    ];
    let out = media::run_bounded(
        ffmpeg,
        &args,
        FRAME_TIMEOUT,
        media::MAX_TOOL_OUTPUT_BYTES,
        cancel,
    )
    .await
    .ok()?;
    (out.status.success() && out.stdout.len() == THUMB_W * THUMB_H).then_some(out.stdout)
}

/// Mean absolute pixel difference of two equal-size grayscale images.
pub fn mean_difference(a: &[u8], b: &[u8]) -> f64 {
    let total: u64 = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y) as u64).sum();
    total as f64 / a.len().max(1) as f64
}

/// True when two frames within 45 s of each other differ visibly.
/// `thumbs` are grayscale thumbnails in frame order; None means unreadable.
pub fn visual_change_in(times_ms: &[i64], thumbs: &[Option<Vec<u8>>]) -> bool {
    if thumbs.len() < 3 {
        return false;
    }
    let mut previous: Option<(&[u8], i64)> = None;
    for (time, thumb) in times_ms.iter().zip(thumbs) {
        // PIL raising OSError on any frame rejects the whole set.
        let Some(current) = thumb.as_deref() else {
            return false;
        };
        if let Some((prev, prev_time)) = previous {
            if time - prev_time <= (MAX_SUPPORT_GAP_SECONDS * 1000.0) as i64
                && mean_difference(prev, current) >= MIN_MEAN_DIFFERENCE
            {
                return true;
            }
        }
        previous = Some((current, *time));
    }
    false
}

/// Reject a still image or static slide before paying for visual planning.
pub async fn has_visual_change(
    ffmpeg: &Path,
    frames: &[VisionFrame],
    cancel: &CancellationToken,
) -> bool {
    if frames.len() < 3 {
        return false;
    }
    let mut thumbs = Vec::with_capacity(frames.len());
    for frame in frames {
        let thumb = thumbnail(ffmpeg, &frame.path, cancel).await;
        let missing = thumb.is_none();
        thumbs.push(thumb);
        if missing {
            break;
        }
    }
    let times: Vec<i64> = frames.iter().map(|f| f.timestamp_ms).collect();
    visual_change_in(&times[..thumbs.len()], &thumbs)
}
