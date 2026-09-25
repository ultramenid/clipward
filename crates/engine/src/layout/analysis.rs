//! Pure layout heuristics: shot segmentation, face tracking, classification,
//! webcam following, focus paths, framing styles and vision merge.
//! Port of the helpers in layout_analyzer.py; constants are unchanged.
use super::imgproc::compare_hist_bhattacharyya;
use super::types::{Box, ShotLayout, SCREEN, SCREEN_CAM, TALKING_HEAD, TWO_SHOT};
use serde_json::Value;

// ------------------------------------------------------------------
// Tunables
// ------------------------------------------------------------------

pub const ANALYSIS_FPS: f64 = 4.0;
pub const ANALYSIS_WIDTH: u32 = 640;
pub const KEYFRAME_WIDTH: u32 = 768;
pub const KEYFRAME_FPS: f64 = 1.0;
pub const MAX_ANALYSIS_HEIGHT: u32 = 1280;
pub const MAX_ANALYSIS_DURATION_MS: i64 = 60 * 60 * 1000;
pub const MAX_ANALYSIS_FRAMES: usize = (MAX_ANALYSIS_DURATION_MS as f64 / 1000.0 * ANALYSIS_FPS) as usize + 1;
pub const MAX_RETAINED_KEYFRAMES: usize = 256;
pub const MAX_KEYFRAME_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_FACES_PER_FRAME: usize = 32;

pub const FACE_SCORE_THRESHOLD: f32 = 0.72;
pub const FACE_NMS_THRESHOLD: f32 = 0.3;
pub const FACE_TOP_K: usize = 50;
/// Bhattacharyya distance between consecutive HSV histograms that counts as a cut.
pub const SHOT_CUT_THRESHOLD: f64 = 0.42;
pub const MIN_SHOT_MS: i64 = 1200;

/// A face track must be visible in this share of a shot's frames to count.
pub const MIN_TRACK_PRESENCE: f64 = 0.35;
/// Faces smaller than this (fraction of frame height) sitting in a corner are
/// treated as a webcam overlay rather than an on-camera person.
pub const OVERLAY_MAX_FACE_HEIGHT: f64 = 0.17;
/// Faces smaller than this are ignored entirely (crowds, posters, thumbnails).
pub const MIN_FACE_HEIGHT: f64 = 0.035;
/// Share of a webcam overlay's height its face spans (head-and-shoulders framing).
pub const CAM_FACE_SHARE: f64 = 0.28;

/// Face-tracked crop camera: ignore motion inside this share of the crop width,
/// and cap how fast the virtual camera pans (crop widths per second).
pub const CAMERA_DEADZONE: f64 = 0.12;
pub const CAMERA_MAX_SPEED: f64 = 0.9;
pub const MAX_PATH_KEYFRAMES: usize = 40;
pub const FOCUS_STEP_MS: i64 = (1000.0 / ANALYSIS_FPS) as i64;

/// Framing styles (config.LayoutStyle).
pub const STYLE_AUTO: &str = "auto";
pub const STYLE_FILL: &str = "fill";
pub const STYLE_FIT: &str = "fit";

pub const ALL_LAYOUTS: [&str; 4] = [TALKING_HEAD, TWO_SHOT, SCREEN_CAM, SCREEN];

/// Python's round(): nearest, ties to even.
fn py_round(v: f64) -> i64 {
    v.round_ties_even() as i64
}

/// Analysis frame size: at most 640 wide and 1280 tall, even dimensions.
/// Callers validate the source dimensions first (`media::validate_video_dimensions`).
pub fn analysis_dimensions(width: u32, height: u32) -> (u32, u32) {
    let (w, h) = (width as f64, height as f64);
    let scale = (ANALYSIS_WIDTH as f64 / w).min(MAX_ANALYSIS_HEIGHT as f64 / h);
    let aw = (py_round(w * scale / 2.0) * 2).max(2);
    let ah = (py_round(h * scale / 2.0) * 2).max(2);
    (aw as u32, ah as u32)
}

// ------------------------------------------------------------------
// Data
// ------------------------------------------------------------------

/// One analyzed frame: window-relative time, normalized face boxes and a
/// normalized 24x16 H/S histogram.
#[derive(Debug, Clone, Default)]
pub struct FrameInfo {
    pub t_ms: i64,
    pub faces: Vec<Box>,
    pub hist: Vec<f32>,
}

/// One person's face across the frames of a shot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FaceTrack {
    pub samples: Vec<(i64, Box)>,
}

/// statistics.median: mean of the two middle values for even counts.
pub fn median(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut v: Vec<f64> = values.into_iter().collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

pub fn median_box<'a>(boxes: impl Iterator<Item = &'a Box> + Clone) -> Box {
    Box {
        x: median(boxes.clone().map(|b| b.x)),
        y: median(boxes.clone().map(|b| b.y)),
        w: median(boxes.clone().map(|b| b.w)),
        h: median(boxes.map(|b| b.h)),
    }
}

impl FaceTrack {
    pub fn median_box(&self) -> Box {
        median_box(self.samples.iter().map(|(_, b)| b))
    }
}

/// A ShotLayout with the Python defaults (`source="heuristic"`).
pub fn new_shot(start_ms: i64, end_ms: i64, layout: &str) -> ShotLayout {
    ShotLayout { start_ms, end_ms, layout: layout.to_string(), source: "heuristic".into(), ..Default::default() }
}

// ------------------------------------------------------------------
// Pure helpers
// ------------------------------------------------------------------

/// Split the timeline into shots at histogram cuts; merge shots < MIN_SHOT_MS.
pub fn segment_shots(frames: &[FrameInfo], duration_ms: i64) -> Vec<(i64, i64)> {
    if frames.is_empty() {
        return vec![(0, duration_ms)];
    }
    let mut cuts = vec![0i64];
    for pair in frames.windows(2) {
        let (prev, cur) = (&pair[0], &pair[1]);
        let dist = compare_hist_bhattacharyya(&prev.hist, &cur.hist);
        if dist > SHOT_CUT_THRESHOLD {
            // The cut happened somewhere between the two samples.
            cuts.push((prev.t_ms + cur.t_ms).div_euclid(2));
        }
    }
    cuts.push(duration_ms);

    let mut merged: Vec<(i64, i64)> = Vec::new();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if end <= start {
            continue;
        }
        match merged.last_mut() {
            Some(last) if end - start < MIN_SHOT_MS || last.1 - last.0 < MIN_SHOT_MS => last.1 = end,
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Greedy nearest-center association of face boxes across frames.
pub fn track_faces(frames: &[FrameInfo]) -> Vec<FaceTrack> {
    let mut tracks: Vec<FaceTrack> = Vec::new();
    for frame in frames {
        let mut used: Vec<usize> = Vec::new();
        let mut faces = frame.faces.clone();
        faces.sort_by(|a, b| b.area().partial_cmp(&a.area()).unwrap_or(std::cmp::Ordering::Equal));
        for face in faces {
            let mut best: Option<usize> = None;
            let mut best_dist = 0.12;
            for (i, track) in tracks.iter().enumerate() {
                if used.contains(&i) {
                    continue;
                }
                let last = track.samples.last().expect("tracks are never empty").1;
                let dist = ((last.cx() - face.cx()).powi(2) + (last.cy() - face.cy()).powi(2)).sqrt();
                let size_ratio = face.h / last.h.max(1e-6);
                if dist < best_dist && 0.5 < size_ratio && size_ratio < 2.0 {
                    best = Some(i);
                    best_dist = dist;
                }
            }
            match best {
                None => {
                    tracks.push(FaceTrack { samples: vec![(frame.t_ms, face)] });
                    used.push(tracks.len() - 1);
                }
                Some(i) => {
                    tracks[i].samples.push((frame.t_ms, face));
                    used.push(i);
                }
            }
        }
    }
    tracks
}

/// Small face in a frame corner: almost always a webcam overlay.
pub fn is_corner_overlay(face: &Box) -> bool {
    if face.h > OVERLAY_MAX_FACE_HEIGHT {
        return false;
    }
    (face.cx() < 0.3 || face.cx() > 0.7) && (face.cy() < 0.4 || face.cy() > 0.6)
}

/// Estimate the webcam overlay rectangle around a face.
///
/// Used when the vision model is unavailable. A streaming webcam frames head
/// and shoulders with the face spanning ~28% of its height. The box stays in
/// the face's corner so it never reaches across the middle of the frame.
pub fn estimate_cam_box(face: &Box, src_w: u32, src_h: u32) -> Box {
    let cam_h = (face.h / CAM_FACE_SHARE).min(0.45);
    let cam_w = (cam_h * (16.0 / 9.0) * (src_h as f64 / src_w as f64)).min(0.4);
    let b = Box { x: face.cx() - cam_w / 2.0, y: face.cy() - cam_h * 0.40, w: cam_w, h: cam_h }.clamp();
    let (mut x0, mut x1) = if face.cx() >= 0.5 { (0.5, 1.0) } else { (0.0, 0.5) };
    let (mut y0, mut y1) = if face.cy() >= 0.5 { (0.5, 1.0) } else { (0.0, 0.5) };
    // Never cut into the face itself, whatever the corner bounds say.
    x0 = f64::min(x0, face.x);
    x1 = f64::max(x1, face.x + face.w);
    y0 = f64::min(y0, face.y);
    y1 = f64::max(y1, face.y + face.h);
    let (left, top) = (b.x.max(x0), b.y.max(y0));
    let (right, bottom) = ((b.x + b.w).min(x1), (b.y + b.h).min(y1));
    Box { x: left, y: top, w: right - left, h: bottom - top }
}

/// Move/scale a known webcam box to follow its face.
pub fn transfer_cam_box(ref_cam: &Box, ref_face: &Box, face: &Box) -> Box {
    let scale = face.h / ref_face.h.max(1e-6);
    Box {
        x: face.cx() + (ref_cam.x - ref_face.cx()) * scale,
        y: face.cy() + (ref_cam.y - ref_face.cy()) * scale,
        w: ref_cam.w * scale,
        h: ref_cam.h * scale,
    }
    .clamp()
}

fn overlay_moved(face: &Box, anchor: &Box) -> bool {
    let ratio = face.h / anchor.h.max(1e-6);
    (face.cx() - anchor.cx()).abs() > 0.6 * anchor.w
        || (face.cy() - anchor.cy()).abs() > 0.6 * anchor.h
        || !(0.8..=1.25).contains(&ratio)
}

/// Split a screen+webcam shot wherever the webcam overlay moves or resizes.
///
/// 1. Observe the webcam face per sample: the small face nearest the last one.
///    With a known webcam `region` (vision box), any face inside it counts too.
/// 2. Dropouts lasting MIN_SHOT_MS or more are "no webcam" stretches.
/// 3. A jump in position/size held for 2+ samples starts a new segment.
///
/// Segments shorter than MIN_SHOT_MS are absorbed by their neighbour.
/// Returns contiguous (start_ms, end_ms, median face or None) covering the shot.
pub fn split_overlay_segments(
    frames: &[FrameInfo],
    shot_start: i64,
    shot_end: i64,
    seed: Box,
    region: Option<Box>,
) -> Vec<(i64, i64, Option<Box>)> {
    let max_face_h = OVERLAY_MAX_FACE_HEIGHT * 1.3;
    let mut observations: Vec<(i64, Option<Box>)> = Vec::with_capacity(frames.len());
    let mut anchor = seed;
    for frame in frames {
        let dist = |f: &Box| (f.cx() - anchor.cx()).powi(2) + (f.cy() - anchor.cy()).powi(2);
        let mut face: Option<Box> = None;
        for f in &frame.faces {
            let in_region = region.is_some_and(|r| r.contains(f.cx(), f.cy()) && f.h <= r.h);
            if !(MIN_FACE_HEIGHT <= f.h && (f.h <= max_face_h || in_region)) {
                continue;
            }
            // min(): the first of equally near candidates wins.
            if face.is_none_or(|best| dist(f) < dist(&best)) {
                face = Some(*f);
            }
        }
        observations.push((frame.t_ms, face));
        if let Some(f) = face {
            anchor = f;
        }
    }

    // Long dropouts -> no-webcam stretches.
    let mut in_gap = vec![false; observations.len()];
    let mut i = 0;
    while i < observations.len() {
        if observations[i].1.is_some() {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < observations.len() && observations[j].1.is_none() {
            j += 1;
        }
        let gap_end = if j < observations.len() { observations[j].0 } else { shot_end };
        if gap_end - observations[i].0 >= MIN_SHOT_MS {
            in_gap[i..j].iter_mut().for_each(|g| *g = true);
        }
        i = j;
    }

    let mut segments: Vec<(i64, Option<Vec<Box>>)> = Vec::new();
    let mut current: Option<usize> = None;
    let mut reference: Option<Box> = None;
    let mut pending: Vec<(i64, Box)> = Vec::new();
    for (idx, &(t_ms, face)) in observations.iter().enumerate() {
        if in_gap[idx] {
            if current.is_none_or(|c| segments[c].1.is_some()) {
                segments.push((t_ms, None));
                current = Some(segments.len() - 1);
                pending.clear();
            }
            continue;
        }
        let Some(face) = face else {
            continue; // brief dropout (hand over face, looking away)
        };
        let cur = match current {
            Some(c) if segments[c].1.is_some() => c,
            _ => {
                segments.push((t_ms, Some(vec![face])));
                current = Some(segments.len() - 1);
                reference = Some(face);
                pending.clear();
                continue;
            }
        };
        let r = reference.unwrap_or(face);
        if overlay_moved(&face, &r) {
            pending.push((t_ms, face));
            if pending.len() >= 2 {
                segments.push((pending[0].0, Some(pending.iter().map(|p| p.1).collect())));
                current = Some(segments.len() - 1);
                reference = pending.last().map(|p| p.1);
                pending.clear();
            }
            continue;
        }
        pending.clear();
        if let Some(faces) = segments[cur].1.as_mut() {
            faces.push(face);
        }
        reference = Some(Box {
            x: r.x * 0.7 + face.x * 0.3,
            y: r.y * 0.7 + face.y * 0.3,
            w: r.w * 0.7 + face.w * 0.3,
            h: r.h * 0.7 + face.h * 0.3,
        });
    }

    if segments.is_empty() {
        return vec![(shot_start, shot_end, None)];
    }

    let mut result: Vec<(i64, i64, Option<Box>)> = Vec::new();
    for (i, (seg_start, faces)) in segments.iter().enumerate() {
        let seg_start = if i == 0 { shot_start } else { *seg_start };
        let seg_end = segments.get(i + 1).map_or(shot_end, |s| s.0);
        let face = faces.as_ref().filter(|f| !f.is_empty()).map(|f| median_box(f.iter()));
        match result.last_mut() {
            Some(prev) if seg_end - seg_start < MIN_SHOT_MS => prev.1 = seg_end,
            _ => result.push((seg_start, seg_end, face)),
        }
    }
    if result.len() > 1 && result[0].1 - result[0].0 < MIN_SHOT_MS {
        let (first, second) = (result[0], result[1]);
        result.splice(0..2, [(first.0, second.1, second.2.or(first.2))]);
    }
    result
}

/// Screen region: the full frame, trimmed away from a side-mounted webcam.
pub fn screen_box_excluding_cam(cam: &Box) -> Box {
    // Only trim when the cam spans most of a side; corner cams overlap the
    // screen, which is normal for streams.
    if cam.h > 0.8 && cam.x < 0.05 {
        return Box { x: cam.x + cam.w, y: 0.0, w: 1.0 - cam.w, h: 1.0 };
    }
    if cam.h > 0.8 && cam.x + cam.w > 0.95 {
        return Box { x: 0.0, y: 0.0, w: cam.x, h: 1.0 };
    }
    Box { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }
}

/// Heuristic layout for one shot from its face tracks.
///
/// Returns the layout (times filled in by the caller) and, for talking-head
/// shots, the track the camera should follow.
pub fn classify_shot(tracks: &[FaceTrack], shot_frames: usize, src_w: u32, src_h: u32) -> (ShotLayout, Option<FaceTrack>) {
    let min_samples = f64::max(1.0, MIN_TRACK_PRESENCE * shot_frames as f64);
    let mut present: Vec<(usize, Box)> = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (i, t.median_box()))
        .filter(|(i, m)| tracks[*i].samples.len() as f64 >= min_samples && m.h >= MIN_FACE_HEIGHT)
        .collect();
    present.sort_by(|a, b| b.1.area().partial_cmp(&a.1.area()).unwrap_or(std::cmp::Ordering::Equal));

    let mut overlays: Vec<(usize, Box)> = present.iter().filter(|(_, m)| is_corner_overlay(m)).copied().collect();
    let on_camera: Vec<(usize, Box)> =
        present.iter().filter(|(i, _)| !overlays.iter().any(|(o, _)| o == i)).copied().collect();

    // A webcam that moves mid-shot splits into several short tracks; judge
    // its presence by all corner-overlay faces together.
    if overlays.is_empty() {
        let mut corner: Vec<(usize, Box)> = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (i, t.median_box()))
            .filter(|(_, m)| m.h >= MIN_FACE_HEIGHT && is_corner_overlay(m))
            .collect();
        let total: usize = corner.iter().map(|(i, _)| tracks[*i].samples.len()).sum();
        if total as f64 >= MIN_TRACK_PRESENCE * shot_frames as f64 {
            corner.sort_by_key(|(i, _)| std::cmp::Reverse(tracks[*i].samples.len()));
            overlays = corner;
        }
    }

    if on_camera.len() >= 2 {
        let (a, b) = (on_camera[0].1, on_camera[1].1);
        let similar = a.h.min(b.h) / a.h.max(b.h) >= 0.5;
        let separated = (a.cx() - b.cx()).abs() >= 0.22;
        if on_camera.len() == 2 && similar && separated {
            let mut people = vec![a, b];
            people.sort_by(|p, q| p.cx().partial_cmp(&q.cx()).unwrap_or(std::cmp::Ordering::Equal));
            return (ShotLayout { people, ..new_shot(0, 0, TWO_SHOT) }, None);
        }
        if on_camera.len() > 2 {
            return (new_shot(0, 0, SCREEN), None);
        }
    }
    if let Some(&(main, main_box)) = on_camera.first() {
        return (ShotLayout { people: vec![main_box], ..new_shot(0, 0, TALKING_HEAD) }, Some(tracks[main].clone()));
    }
    if let Some(&(_, face)) = overlays.first() {
        let cam = estimate_cam_box(&face, src_w, src_h);
        return (
            ShotLayout {
                cam_box: Some(cam),
                cam_face: Some(face),
                screen_box: Some(screen_box_excluding_cam(&cam)),
                ..new_shot(0, 0, SCREEN_CAM)
            },
            None,
        );
    }
    (new_shot(0, 0, SCREEN), None)
}

/// Turn noisy face centers into a calm virtual-camera path.
///
/// Holds still while the subject stays inside a dead zone, then pans with a
/// capped speed. Returns keyframes (t_ms, cx, cy) relative to the shot start.
pub fn smooth_focus_path(samples: &[(i64, Box)], duration_ms: i64, crop_w_frac: f64) -> Vec<(i64, f64, f64)> {
    smooth_focus_path_step(samples, duration_ms, crop_w_frac, FOCUS_STEP_MS)
}

pub fn smooth_focus_path_step(
    samples: &[(i64, Box)],
    duration_ms: i64,
    crop_w_frac: f64,
    step_ms: i64,
) -> Vec<(i64, f64, f64)> {
    if samples.is_empty() || step_ms <= 0 {
        return Vec::new();
    }
    let mut samples = samples.to_vec();
    samples.sort_by_key(|s| s.0);

    // Resample onto a regular grid, holding the nearest detection.
    let mut grid: Vec<(i64, f64, f64)> = Vec::new();
    let mut j = 0;
    let mut t = 0;
    while t < duration_ms.max(1) {
        while j + 1 < samples.len() && (samples[j + 1].0 - t).abs() <= (samples[j].0 - t).abs() {
            j += 1;
        }
        grid.push((t, samples[j].1.cx(), samples[j].1.cy()));
        t += step_ms;
    }

    // Median filter to reject single-frame detector jumps.
    let k = 2usize;
    let xs: Vec<f64> = grid.iter().map(|g| g.1).collect();
    let ys: Vec<f64> = grid.iter().map(|g| g.2).collect();
    let window = |v: &[f64], i: usize| median(v[i.saturating_sub(k)..(i + k + 1).min(v.len())].iter().copied());
    let fx: Vec<f64> = (0..xs.len()).map(|i| window(&xs, i)).collect();
    let fy: Vec<f64> = (0..ys.len()).map(|i| window(&ys, i)).collect();

    let deadzone = CAMERA_DEADZONE * crop_w_frac;
    let max_step = CAMERA_MAX_SPEED * crop_w_frac * step_ms as f64 / 1000.0;
    let head = ((1000 / step_ms).max(1) as usize).min(fx.len());
    let mut cam_x = median(fx[..head].iter().copied());
    let mut cam_y = median(fy[..head].iter().copied());
    let mut path = vec![(0i64, cam_x, cam_y)];
    for i in 1..grid.len() {
        let (x, y) = (fx[i], fy[i]);
        let dx = x - cam_x;
        if dx.abs() > deadzone {
            // Move toward the subject (not all the way: re-center with ease).
            let sign = if dx > 0.0 { 1.0 } else { -1.0 };
            let mv = ((dx - deadzone * sign) * 0.5).min(max_step).max(-max_step);
            cam_x += mv;
        }
        cam_y += (y - cam_y) * 0.2;
        path.push((grid[i].0, cam_x, cam_y));
    }

    // Keep only keyframes where the camera actually moves.
    let mut keyframes = vec![path[0]];
    for i in 1..path.len().saturating_sub(1) {
        let (prev, cur, next) = (path[i - 1], path[i], path[i + 1]);
        let moving_before = (cur.1 - prev.1).abs() > 1e-4;
        let moving_after = (next.1 - cur.1).abs() > 1e-4;
        if moving_before != moving_after {
            keyframes.push(cur);
        }
    }
    if path.len() > 1 {
        keyframes.push(*path.last().expect("non-empty"));
    }

    // Bound expression size for FFmpeg: sample evenly, always keep the last.
    if keyframes.len() > MAX_PATH_KEYFRAMES {
        let step = keyframes.len().div_ceil(MAX_PATH_KEYFRAMES);
        let mut thinned: Vec<(i64, f64, f64)> = keyframes.iter().step_by(step).copied().collect();
        let last = *keyframes.last().expect("non-empty");
        if *thinned.last().expect("non-empty") != last {
            thinned.push(last);
        }
        keyframes = thinned;
    }
    keyframes
}

/// Adjust a detected layout to the user's chosen framing style.
pub fn apply_style(mut shot: ShotLayout, style: &str) -> ShotLayout {
    if shot.detected_layout.as_deref().is_none_or(str::is_empty) {
        shot.detected_layout = Some(shot.layout.clone());
    }
    if style == STYLE_FIT {
        shot.layout = SCREEN.into();
        shot.source = "style".into();
    } else if style == STYLE_FILL && [TWO_SHOT, SCREEN_CAM, SCREEN].contains(&shot.layout.as_str()) {
        let mut focus = shot.people.first().copied();
        if shot.layout == SCREEN_CAM && shot.cam_face.is_some() {
            focus = None; // a tiny webcam blown up to full frame looks bad
        }
        shot.layout = TALKING_HEAD.into();
        shot.source = "style".into();
        match focus {
            Some(f) if shot.focus_path.is_empty() => shot.focus_path = vec![(0, f.cx(), f.cy())],
            _ if shot.focus_path.is_empty() => shot.focus_path = vec![(0, 0.5, 0.5)],
            _ => {}
        }
    }
    shot
}

// ------------------------------------------------------------------
// Vision merge
// ------------------------------------------------------------------

/// Python `int(v)` for JSON numbers and numeric strings.
fn json_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        Value::Bool(b) => Some(*b as i64),
        _ => None,
    }
}

/// `[ymin, xmin, ymax, xmax]` in 0-1000 -> normalized Box; None when malformed or tiny.
pub fn box_from_1000(values: Option<&Value>) -> Option<Box> {
    let arr = values?.as_array()?;
    if arr.len() != 4 {
        return None;
    }
    let mut v = [0f64; 4];
    for (slot, raw) in v.iter_mut().zip(arr) {
        *slot = json_int(raw)?.clamp(0, 1000) as f64 / 1000.0;
    }
    let [ymin, xmin, ymax, xmax] = v;
    if xmax - xmin < 0.02 || ymax - ymin < 0.02 {
        return None;
    }
    Some(Box { x: xmin, y: ymin, w: xmax - xmin, h: ymax - ymin })
}

/// Approximate the face inside a head-and-shoulders box (the renderer sizes crops from faces).
pub fn face_from_person_box(person: &Box) -> Box {
    let face_h = person.h * 0.45;
    let face_w = person.w * 0.5;
    Box { x: person.cx() - face_w / 2.0, y: person.y + person.h * 0.08, w: face_w, h: face_h }.clamp()
}

/// Combine the vision model's layout with locally detected faces.
pub fn merge_vision_result(heuristic: &ShotLayout, result: &Value) -> ShotLayout {
    let Some(layout) = result.get("layout").and_then(Value::as_str).filter(|l| ALL_LAYOUTS.contains(l)) else {
        return heuristic.clone();
    };

    let mut cam = box_from_1000(result.get("cam_box"));
    let screen = box_from_1000(result.get("screen_box"));
    let mut people: Vec<Box> = result
        .get("people")
        .and_then(Value::as_array)
        .map(|ps| ps.iter().filter_map(|p| box_from_1000(Some(p))).map(|b| face_from_person_box(&b)).collect())
        .unwrap_or_default();
    people.sort_by(|a, b| a.cx().partial_cmp(&b.cx()).unwrap_or(std::cmp::Ordering::Equal));

    let mut merged = ShotLayout { source: "vision".into(), ..new_shot(0, 0, layout) };
    match layout {
        SCREEN_CAM => {
            if cam.is_none_or(|c| !(0.01..=0.5).contains(&c.area())) {
                cam = heuristic.cam_box;
            }
            let Some(cam) = cam else {
                merged.layout = SCREEN.into();
                return merged;
            };
            merged.cam_box = Some(cam);
            merged.screen_box = Some(screen.unwrap_or_else(|| screen_box_excluding_cam(&cam)));
            merged.screen_focus = box_from_1000(result.get("screen_focus"));
            // Prefer the locally detected face inside the cam for centering.
            let mut face = heuristic.cam_face;
            if face.is_none_or(|f| !cam.contains(f.cx(), f.cy())) {
                face = heuristic.people.iter().find(|p| cam.contains(p.cx(), p.cy())).copied();
            }
            merged.cam_face = face;
        }
        TWO_SHOT => {
            let local: &[Box] = if heuristic.layout == TWO_SHOT { &heuristic.people } else { &[] };
            merged.people = if local.len() == 2 { local.to_vec() } else { people.into_iter().take(2).collect() };
            if merged.people.len() < 2 {
                merged.layout = if merged.people.is_empty() { SCREEN } else { TALKING_HEAD }.into();
            }
        }
        TALKING_HEAD => {
            merged.people = if heuristic.people.is_empty() {
                people.into_iter().take(1).collect()
            } else {
                heuristic.people[..1].to_vec()
            };
            merged.focus_path = heuristic.focus_path.clone();
        }
        _ => {
            merged.screen_box = screen;
            merged.screen_focus = box_from_1000(result.get("screen_focus"));
        }
    }
    merged
}

// ------------------------------------------------------------------
// Shot post-processing
// ------------------------------------------------------------------

/// Split a screen+webcam shot where the overlay moves, resizes or disappears.
pub fn follow_webcam(shot: &ShotLayout, frames: &[FrameInfo], src_w: u32, src_h: u32) -> Vec<ShotLayout> {
    let Some(cam) = shot.cam_box else {
        return vec![shot.clone()];
    };
    let vision = shot.source == "vision";
    let seed = shot.cam_face.unwrap_or(Box {
        x: cam.cx() - cam.w * 0.15,
        y: cam.cy() - cam.h * 0.25,
        w: cam.w * 0.3,
        h: cam.h * 0.4,
    });
    let segments = split_overlay_segments(frames, shot.start_ms, shot.end_ms, seed, vision.then_some(cam));

    // The vision model saw the frame at the shot's midpoint; that segment's
    // face anchors its webcam box for the others.
    let mid = (shot.start_ms + shot.end_ms).div_euclid(2);
    let ref_face = segments.iter().find(|(a, b, f)| *a <= mid && mid < *b && f.is_some()).and_then(|s| s.2).or(shot.cam_face);

    let base = |start: i64, end: i64, layout: &str| ShotLayout {
        source: shot.source.clone(),
        screen_box: shot.screen_box,
        screen_focus: shot.screen_focus,
        ..new_shot(start, end, layout)
    };
    segments
        .into_iter()
        .map(|(start, end, face)| match face {
            // The vision model saw the webcam; YuNet missing its face doesn't remove it.
            None if vision => ShotLayout { cam_box: Some(cam), cam_face: shot.cam_face, ..base(start, end, SCREEN_CAM) },
            None => base(start, end, SCREEN),
            Some(face) => {
                let cam_box = match ref_face {
                    Some(r) if vision => transfer_cam_box(&cam, &r, &face),
                    _ if vision && cam.contains(face.cx(), face.cy()) => cam,
                    _ => estimate_cam_box(&face, src_w, src_h),
                };
                ShotLayout { cam_box: Some(cam_box), cam_face: Some(face), ..base(start, end, SCREEN_CAM) }
            }
        })
        .collect()
}

/// Join neighbouring shots that ended up with the same static framing.
pub fn merge_adjacent(shots: Vec<ShotLayout>) -> Vec<ShotLayout> {
    let mut merged: Vec<ShotLayout> = Vec::new();
    for shot in shots {
        let same_static = merged.last().is_some_and(|prev| {
            prev.layout == shot.layout
                && (shot.layout == SCREEN
                    || (shot.layout == SCREEN_CAM
                        && match (prev.cam_box, shot.cam_box) {
                            (Some(p), Some(s)) => {
                                (p.cx() - s.cx()).abs() < 0.05
                                    && (p.cy() - s.cy()).abs() < 0.05
                                    && (0.85..=1.18).contains(&(s.h / p.h.max(1e-6)))
                            }
                            _ => false,
                        }))
        });
        if same_static {
            if let Some(prev) = merged.last_mut() {
                prev.end_ms = shot.end_ms;
            }
        } else {
            merged.push(shot);
        }
    }
    merged
}
