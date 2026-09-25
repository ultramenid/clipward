//! Layout Renderer - turns a ClipLayoutPlan into an FFmpeg filter graph.
//! Port of layout_renderer.py; every string it builds matches the Python byte for byte.
//!
//! Each shot is trimmed from the source and framed with its own layout, then the
//! shots are concatenated back into one 9:16 stream:
//!
//! ```text
//! talking_head  full-frame crop that pans with the speaker
//! two_shot      each person in their own panel, stacked
//! screen_cam    screen (zoomed on the active area) over the webcam
//! screen        whole frame over a blurred copy of itself
//! ```
//!
//! Also provides per-shot positions for captions, the title card and the
//! channel banner so they never sit on top of a face or cut across a seam.
use super::clip_editor::TimeMap;
use super::pyfmt::{self, Rational};
use super::video_speed::{speed_audio_filter, validate_video_speed};
use crate::error::{EngineError, Result};
use crate::layout::types::{Box as LBox, ClipLayoutPlan, ShotLayout, SCREEN, SCREEN_CAM, TALKING_HEAD, TWO_SHOT};

pub const TWO_SHOT_TOP_RATIO: f64 = 0.5;
pub const SCREEN_CAM_TOP_RATIO: f64 = 0.6;
pub const MAX_SCREEN_CAM_TOP_RATIO: f64 = 0.7;
pub const MAX_UPSCALE: f64 = 2.6;
pub const PERSON_CROP_FACE_MULT: f64 = 3.2;
pub const PERSON_FACE_Y: f64 = 0.40;
pub const AVOID_MARGIN: f64 = 0.15;

/// Round down to an even integer (yuv420p needs even dimensions).
pub fn even(value: f64) -> i64 {
    (pyfmt::int(value).div_euclid(2) * 2).max(2)
}

fn clamp(value: f64, lo: f64, hi: f64) -> f64 {
    let m = if value < hi { value } else { hi };
    if m > lo {
        m
    } else {
        lo
    }
}

fn sort_points(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut p = points.to_vec();
    p.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    p
}

/// Piecewise-linear FFmpeg expression of `t` through (t_sec, value) points.
pub fn piecewise_expr(points: &[(f64, f64)]) -> String {
    if points.is_empty() {
        return "0".into();
    }
    let points = sort_points(points);
    let mut expr = format!("{:.1}", points[points.len() - 1].1);
    for pair in points.windows(2).rev() {
        let ((t0, v0), (t1, v1)) = (pair[0], pair[1]);
        let span = (t1 - t0).max(1e-3);
        expr = format!("if(lt(t,{t1:.3}),{v0:.1}+({:.1})*(t-{t0:.3})/{span:.3},{expr})", v1 - v0);
    }
    expr
}

/// Step-function expression: values[i] until boundaries[i], last value after.
pub fn step_expr(boundaries: &[f64], values: &[i64]) -> String {
    let mut expr = format!("{}", values[values.len() - 1]);
    for (boundary, value) in boundaries.iter().zip(&values[..values.len() - 1]).rev() {
        expr = format!("if(lt(t,{boundary:.3}),{value},{expr})");
    }
    expr
}

// ------------------------------------------------------------------
// Crop geometry (all in source pixels)
// ------------------------------------------------------------------

type Points = Vec<(f64, f64)>;

fn fill_crop_path(shot: &ShotLayout, src_w: i64, src_h: i64, out_w: i64, out_h: i64) -> (i64, i64, Points, Points) {
    let target = out_w as f64 / out_h as f64;
    let (crop_w, crop_h) = if src_w as f64 / src_h as f64 > target {
        (even(src_h as f64 * target), even(src_h as f64))
    } else {
        (even(src_w as f64), even(src_w as f64 / target))
    };
    let default = [(0i64, 0.5, 0.5)];
    let path: &[(i64, f64, f64)] = if shot.focus_path.is_empty() { &default } else { &shot.focus_path };
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    for &(t_ms, cx, cy) in path {
        let t = (shot.start_ms + t_ms) as f64 / 1000.0;
        xs.push((t, clamp(cx * src_w as f64 - crop_w as f64 / 2.0, 0.0, (src_w - crop_w) as f64)));
        ys.push((t, clamp(cy * src_h as f64 - crop_h as f64 * PERSON_FACE_Y, 0.0, (src_h - crop_h) as f64)));
    }
    (crop_w, crop_h, xs, ys)
}

/// 9:16 crop that follows the shot's focus path. Returns (w, h, x_expr, y_expr).
pub fn fill_crop(shot: &ShotLayout, src_w: i64, src_h: i64, out_w: i64, out_h: i64) -> (i64, i64, String, String) {
    let (crop_w, crop_h, xs, ys) = fill_crop_path(shot, src_w, src_h, out_w, out_h);
    let x_expr = if crop_w < src_w { piecewise_expr(&xs) } else { "0".into() };
    let y_expr = if crop_h < src_h { piecewise_expr(&ys) } else { "0".into() };
    (crop_w, crop_h, x_expr, y_expr)
}

/// Value of piecewise_expr(points) at time t (held flat past either end).
pub fn path_value_at(points: &[(f64, f64)], t: f64) -> f64 {
    let points = sort_points(points);
    if t <= points[0].0 {
        return points[0].1;
    }
    for pair in points.windows(2) {
        let ((t0, v0), (t1, v1)) = (pair[0], pair[1]);
        if t < t1 {
            return v0 + (v1 - v0) * (t - t0) / (t1 - t0).max(1e-3);
        }
    }
    points[points.len() - 1].1
}

/// (w, h, x, y) in source pixels.
pub type CropRect = (i64, i64, i64, i64);

fn fit_rect(cx: f64, cy: f64, w: f64, h: f64, bounds: (f64, f64, f64, f64)) -> CropRect {
    let (bx, by, bw, bh) = bounds;
    let (w, h) = (w.min(bw), h.min(bh));
    let x = clamp(cx - w / 2.0, bx, bx + bw - w);
    let y = clamp(cy - h / 2.0, by, by + bh - h);
    (even(w), even(h), even(x), even(y))
}

/// Head-and-shoulders crop for one person, at the panel's aspect ratio.
pub fn person_crop(face: &LBox, src_w: i64, src_h: i64, panel_w: i64, panel_h: i64, x_bounds: Option<(f64, f64)>) -> CropRect {
    let (src_wf, src_hf) = (src_w as f64, src_h as f64);
    let aspect = panel_w as f64 / panel_h as f64;
    let min_h = panel_h as f64 / MAX_UPSCALE;
    let (x0, x1) = x_bounds.unwrap_or((0.0, src_wf));
    let (fx, fy) = (face.cx() * src_wf, face.cy() * src_hf);

    let mut crop_h = clamp(face.h * src_hf * PERSON_CROP_FACE_MULT, min_h, src_hf);
    crop_h = crop_h.min(((x1 - x0) / aspect).max(min_h));
    // Keep the face between 25% and 58% of the panel height.
    crop_h = crop_h.min(((src_hf - fy) / (1.0 - 0.58)).max(min_h)).min((fy / 0.25).max(min_h));
    let crop_w = crop_h * aspect;

    let bounds = if crop_w <= x1 - x0 { (x0, 0.0, x1 - x0, src_hf) } else { (0.0, 0.0, src_wf, src_hf) };
    let cy = fy + crop_h * (0.5 - PERSON_FACE_Y);
    fit_rect(fx, cy, crop_w, crop_h, bounds)
}

/// Crop inside the webcam overlay, centered on the face.
pub fn cam_crop(cam: &LBox, face: Option<&LBox>, src_w: i64, src_h: i64, panel_w: i64, panel_h: i64) -> CropRect {
    let (src_wf, src_hf) = (src_w as f64, src_h as f64);
    let aspect = panel_w as f64 / panel_h as f64;
    let bounds = (cam.x * src_wf, cam.y * src_hf, cam.w * src_wf, cam.h * src_hf);
    let (bw, bh) = (bounds.2, bounds.3);
    let (mut crop_w, mut crop_h) = if bw / bh > aspect { (bh * aspect, bh) } else { (bw, bw / aspect) };
    if panel_w as f64 / crop_w > MAX_UPSCALE {
        crop_w = bw.min(panel_w as f64 / MAX_UPSCALE);
        crop_h = bh.min(panel_h as f64 / MAX_UPSCALE);
    }
    let fx = face.map(|f| f.cx()).unwrap_or(cam.cx()) * src_wf;
    let fy = face.map(|f| f.cy()).unwrap_or(cam.cy()) * src_hf;
    fit_rect(fx, fy + crop_h * (0.5 - PERSON_FACE_Y), crop_w, crop_h, bounds)
}

/// Scaled size of a crop that can't fill its panel within MAX_UPSCALE, else None.
pub fn panel_fit(rect: CropRect, panel_w: i64, panel_h: i64) -> Option<(i64, i64)> {
    let (w, h) = (rect.0 as f64, rect.1 as f64);
    let (pw, ph) = (panel_w as f64, panel_h as f64);
    let fill = (pw / w).max(ph / h);
    if fill <= MAX_UPSCALE * 1.02 && (w / h - pw / ph).abs() <= 0.02 * pw / ph {
        return None;
    }
    let scale = (pw / w).min(ph / h).min(MAX_UPSCALE);
    Some((even(w * scale), even(h * scale)))
}

/// Even top/bottom heights for one stacked shot.
pub fn stacked_panel_heights(shot: &ShotLayout, src_h: i64, out_h: i64) -> (i64, i64) {
    let top_h = match (&shot.cam_box, shot.layout == SCREEN_CAM) {
        (Some(cam), true) => {
            let preferred_bottom = out_h - even(out_h as f64 * SCREEN_CAM_TOP_RATIO);
            let minimum_bottom = out_h - even(out_h as f64 * MAX_SCREEN_CAM_TOP_RATIO);
            let webcam_budget = even(cam.h * src_h as f64 * MAX_UPSCALE);
            let bottom_h = minimum_bottom.max(preferred_bottom.min(webcam_budget));
            out_h - bottom_h
        }
        _ => even(out_h as f64 * TWO_SHOT_TOP_RATIO),
    };
    (top_h, out_h - top_h)
}

/// Crop of the screen at the panel's aspect, covering the focus area if known.
pub fn screen_crop(
    screen: Option<&LBox>,
    focus: Option<&LBox>,
    src_w: i64,
    src_h: i64,
    panel_w: i64,
    panel_h: i64,
    avoid: Option<&LBox>,
) -> CropRect {
    let (src_wf, src_hf) = (src_w as f64, src_h as f64);
    let (pw, ph) = (panel_w as f64, panel_h as f64);
    let aspect = pw / ph;
    let full = LBox { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };
    let screen = screen.unwrap_or(&full);
    let bounds = (screen.x * src_wf, screen.y * src_hf, screen.w * src_wf, screen.h * src_hf);
    let (bw, bh) = (bounds.2, bounds.3);

    let (max_w, max_h) = if bw / bh > aspect { (bh * aspect, bh) } else { (bw, bw / aspect) };
    let (mut crop_w, mut crop_h) = (max_w, max_h);
    let (mut cx, mut cy) = (bounds.0 + bw / 2.0, bounds.1 + bh / 2.0);

    if let Some(focus) = focus {
        let (fw, fh) = (focus.w * src_wf * 1.1, focus.h * src_hf * 1.1);
        let need_w = fw.max(fh * aspect);
        let min_w = (pw / MAX_UPSCALE).max(0.6 * max_h * aspect);
        crop_w = clamp(need_w, min_w, max_w);
        crop_h = crop_w / aspect;
        cx = focus.cx() * src_wf;
        cy = focus.cy() * src_hf;
    }
    let (mut w, mut h, mut x, mut y) = fit_rect(cx, cy, crop_w, crop_h, bounds);

    if let Some(avoid) = avoid {
        let (mx, my) = (avoid.w * AVOID_MARGIN * src_wf, avoid.h * AVOID_MARGIN * src_hf);
        let (ax0, ax1) = (avoid.x * src_wf - mx, (avoid.x + avoid.w) * src_wf + mx);
        let (ay0, ay1) = (avoid.y * src_hf - my, (avoid.y + avoid.h) * src_hf + my);
        let overlaps = |x: i64, y: i64, w: i64, h: i64| {
            (x as f64) < ax1 && ((x + w) as f64) > ax0 && (y as f64) < ay1 && ((y + h) as f64) > ay0
        };
        if overlaps(x, y, w, h) {
            let on_right = avoid.cx() * src_wf > x as f64 + w as f64 / 2.0;
            if on_right && ax0 - w as f64 >= bounds.0 {
                x = even(ax0 - w as f64);
            } else if !on_right && ax1 + w as f64 <= bounds.0 + bw {
                x = even(ax1);
            } else {
                let room = if on_right { ax0 - bounds.0 } else { bounds.0 + bw - ax1 };
                if room >= pw / MAX_UPSCALE {
                    let new_w = even(room);
                    let new_h = even(new_w as f64 / aspect);
                    x = if on_right { even(bounds.0) } else { even(ax1) };
                    y = even(clamp(y as f64 + (h - new_h) as f64 / 2.0, bounds.1, bounds.1 + bh - new_h as f64));
                    w = new_w;
                    h = new_h;
                }
            }
        }
        if overlaps(x, y, w, h) {
            let min_h = ph / MAX_UPSCALE;
            let mut best: Option<(f64, CropRect)> = None;
            for (top, bottom) in [(bounds.1, ay0), (ay1, bounds.1 + bh)] {
                let available_h = bottom - top;
                let candidate_h = (h as f64).min(available_h).min(bw / aspect);
                if candidate_h < min_h {
                    continue;
                }
                let candidate_w = candidate_h * aspect;
                let candidate_x = clamp(cx - candidate_w / 2.0, bounds.0, bounds.0 + bw - candidate_w);
                let candidate_y = clamp(cy - candidate_h / 2.0, top, bottom - candidate_h);
                let rect = (even(candidate_w), even(candidate_h), even(candidate_x), even(candidate_y));
                if best.map(|b| candidate_h > b.0).unwrap_or(true) {
                    best = Some((candidate_h, rect));
                }
            }
            if let Some((_, rect)) = best {
                (w, h, x, y) = rect;
            }
        }
    }
    (w, h, x, y)
}

// ------------------------------------------------------------------
// Filter graph
// ------------------------------------------------------------------

fn crop(rect: CropRect) -> String {
    format!("crop={}:{}:{}:{}", rect.0, rect.1, rect.2, rect.3)
}

/// (scaled video height, y offset) of the fit layout.
pub fn letterbox_geometry(src_w: i64, src_h: i64, out_w: i64, out_h: i64) -> (i64, i64) {
    let scaled_h = out_h.min(even((out_w * src_h) as f64 / src_w as f64 + 1.0));
    (scaled_h, (out_h - scaled_h).div_euclid(2))
}

/// Filters from [t{i}] (a trimmed piece) to [v{i}] (framed, out_w x out_h).
pub fn shot_chain(i: usize, shot: &ShotLayout, src_w: i64, src_h: i64, out_w: i64, out_h: i64, landscape: bool) -> String {
    let scale = "flags=lanczos";
    if landscape {
        let target = out_w as f64 / out_h as f64;
        if (src_w as f64 / src_h as f64 - target).abs() < 0.01 * out_w as f64 / out_h as f64 {
            return format!("[t{i}]scale={out_w}:{out_h}:{scale},setsar=1[v{i}]");
        }
        let (bg_w, bg_h) = (even(out_w.div_euclid(4) as f64), even(out_h.div_euclid(4) as f64));
        return format!(
            "[t{i}]split=2[lb{i}][lf{i}];\
             [lb{i}]scale={bg_w}:{bg_h}:force_original_aspect_ratio=increase,crop={bg_w}:{bg_h},\
             gblur=sigma=10,lutyuv=y=val-20,scale={out_w}:{out_h}[lbg{i}];\
             [lf{i}]scale={out_w}:{out_h}:force_original_aspect_ratio=decrease:{scale}[lfg{i}];\
             [lbg{i}][lfg{i}]overlay=(W-w)/2:(H-h)/2,setsar=1[v{i}]"
        );
    }
    if shot.layout == TALKING_HEAD {
        let (w, h, x_expr, y_expr) = fill_crop(shot, src_w, src_h, out_w, out_h);
        return format!("[t{i}]crop=w={w}:h={h}:x='{x_expr}':y='{y_expr}',scale={out_w}:{out_h}:{scale},setsar=1[v{i}]");
    }

    let (top_h, bottom_h) = stacked_panel_heights(shot, src_h, out_h);
    if shot.layout == TWO_SHOT && shot.people.len() >= 2 {
        let (left, right) = (&shot.people[0], &shot.people[1]);
        let mid = (left.cx() + right.cx()) / 2.0 * src_w as f64;
        return format!(
            "[t{i}]split=2[pa{i}][pb{i}];\
             [pa{i}]{},scale={out_w}:{top_h}:{scale}[top{i}];\
             [pb{i}]{},scale={out_w}:{bottom_h}:{scale}[bot{i}];\
             [top{i}][bot{i}]vstack=inputs=2,setsar=1[v{i}]",
            crop(person_crop(left, src_w, src_h, out_w, top_h, Some((0.0, mid)))),
            crop(person_crop(right, src_w, src_h, out_w, bottom_h, Some((mid, src_w as f64)))),
        );
    }
    if shot.layout == SCREEN_CAM {
        if let Some(cam_box) = &shot.cam_box {
            let cam_rect = cam_crop(cam_box, shot.cam_face.as_ref(), src_w, src_h, out_w, bottom_h);
            let cam_panel = match panel_fit(cam_rect, out_w, bottom_h) {
                None => format!("[sb{i}]{},scale={out_w}:{bottom_h}:{scale}[bot{i}]", crop(cam_rect)),
                Some((fit_w, fit_h)) => {
                    let (bg_w, bg_h) = (even(out_w.div_euclid(4) as f64), even(bottom_h.div_euclid(4) as f64));
                    format!(
                        "[sb{i}]{},split=2[cb{i}][cf{i}];\
                         [cb{i}]scale={bg_w}:{bg_h}:force_original_aspect_ratio=increase,crop={bg_w}:{bg_h},\
                         gblur=sigma=10,lutyuv=y=val-20,scale={out_w}:{bottom_h}[cbg{i}];\
                         [cf{i}]scale={fit_w}:{fit_h}:{scale}[cfg{i}];\
                         [cbg{i}][cfg{i}]overlay={}:{}[bot{i}]",
                        crop(cam_rect),
                        (out_w - fit_w).div_euclid(2),
                        (bottom_h - fit_h).div_euclid(2),
                    )
                }
            };
            let screen = screen_crop(
                shot.screen_box.as_ref(), shot.screen_focus.as_ref(), src_w, src_h, out_w, top_h, Some(cam_box),
            );
            return format!(
                "[t{i}]split=2[sa{i}][sb{i}];\
                 [sa{i}]{},scale={out_w}:{top_h}:{scale}[top{i}];\
                 {cam_panel};\
                 [top{i}][bot{i}]vstack=inputs=2,setsar=1[v{i}]",
                crop(screen),
            );
        }
    }

    // Fit: whole frame over a blurred, darkened fill of itself.
    let (scaled_h, overlay_y) = letterbox_geometry(src_w, src_h, out_w, out_h);
    let bg_w = src_w.min(even((src_h * out_w) as f64 / out_h as f64));
    let bg_h = if bg_w == src_w { src_h.min(even((src_w * out_h) as f64 / out_w as f64)) } else { src_h };
    format!(
        "[t{i}]split=2[bi{i}][fi{i}];\
         [bi{i}]crop={bg_w}:{bg_h}:{}:{},\
         scale={}:{},gblur=sigma=10,lutyuv=y=val-20,\
         scale={out_w}:{out_h}[bg{i}];\
         [fi{i}]scale={out_w}:{scaled_h}:{scale}[fg{i}];\
         [bg{i}][fg{i}]overlay=0:{overlay_y},setsar=1[v{i}]",
        (src_w - bg_w).div_euclid(2),
        (src_h - bg_h).div_euclid(2),
        out_w.div_euclid(4),
        out_h.div_euclid(4),
    )
}

/// (shot index, start_ms, end_ms) for every kept piece of every shot, in order.
pub fn timeline_pieces(plan: &ClipLayoutPlan, keeps: Option<&[(i64, i64)]>) -> Vec<(usize, i64, i64)> {
    let window_end = plan.shots.last().map(|s| s.end_ms).unwrap_or(0);
    let default = [(0, window_end)];
    let keeps = keeps.unwrap_or(&default);
    let mut pieces = Vec::new();
    for (i, shot) in plan.shots.iter().enumerate() {
        for &(k_start, k_end) in keeps {
            let (s, e) = (shot.start_ms.max(k_start), shot.end_ms.min(k_end));
            if e > s {
                pieces.push((i, s, e));
            }
        }
    }
    pieces
}

/// Audio polish: platform loudness target and click-free edges.
pub const AUDIO_FORMAT: &str = "aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo";
/// Preserve the window's clock, including delayed starts and missing packets.
pub const AUDIO_SYNC: &str = "aresample=48000:async=1:first_pts=0:min_hard_comp=0.001";
pub const LOUDNESS_TARGET: &str = "I=-14:TP=-1.5:LRA=11";
/// Some FFmpeg versions emit NaNs when normalizing short digital silence.
pub const FINITE_AUDIO: &str = "aeval='if(isnan(val(ch))+isinf(val(ch)),0,val(ch))':c=same";

pub fn loudness_filter_default() -> String {
    format!("loudnorm={LOUDNESS_TARGET},aresample=48000,{FINITE_AUDIO},{AUDIO_FORMAT}")
}

/// Second pass of two-pass loudnorm from a first-pass measurement (FFmpeg's
/// print_format=json output). None if the measurement is unusable.
pub fn measured_loudness_filter(measured: &serde_json::Value) -> Option<String> {
    let keys = ["input_i", "input_tp", "input_lra", "input_thresh", "target_offset"];
    let mut values = [0.0f64; 5];
    for (slot, key) in values.iter_mut().zip(keys) {
        *slot = pyfmt::json_float(measured.get(key)?)?;
    }
    if values.iter().any(|v| !v.is_finite()) || values[0] < -70.0 {
        return None;
    }
    Some(format!(
        "loudnorm={LOUDNESS_TARGET}:measured_I={:.2}:measured_TP={:.2}:measured_LRA={:.2}:measured_thresh={:.2}\
         :offset={:.2}:linear=true,aresample=48000,{FINITE_AUDIO},{AUDIO_FORMAT}",
        values[0], values[1], values[2], values[3], values[4],
    ))
}

pub const EDGE_FADE_S: f64 = 0.012;
pub const START_FADE_S: f64 = 0.03;
pub const END_FADE_S: f64 = 0.08;

fn audio_fades(duration_s: f64, first: bool, last: bool) -> String {
    let fade_in = if first { START_FADE_S } else { EDGE_FADE_S };
    let fade_out = if last { END_FADE_S } else { EDGE_FADE_S };
    if duration_s <= fade_in + fade_out {
        return String::new();
    }
    format!(
        ",afade=t=in:st=0:d={}\
         ,afade=t=out:st={:.3}:d={}",
        pyfmt::repr(fade_in),
        duration_s - fade_out,
        pyfmt::repr(fade_out),
    )
}

/// Options for [`build_layout_graph`] (the Python keyword arguments).
#[derive(Debug, Clone)]
pub struct GraphOptions<'a> {
    pub keeps: Option<&'a [(i64, i64)]>,
    pub with_audio: bool,
    pub landscape: bool,
    pub fps: &'a str,
    pub loudness_filter: Option<&'a str>,
    pub video_speed: f64,
}

impl Default for GraphOptions<'_> {
    fn default() -> Self {
        Self { keeps: None, with_audio: false, landscape: false, fps: "30", loudness_filter: None, video_speed: 1.0 }
    }
}

fn graph_error(detail: &str) -> EngineError {
    EngineError::msg(format!("video render failed: {detail}"))
}

/// Filter graph from [0:v] (and [0:a]) to [base] (and [aout]).
pub fn build_layout_graph(plan: &ClipLayoutPlan, out_w: i64, out_h: i64, opts: &GraphOptions) -> Result<String> {
    validate_video_speed(opts.video_speed)?;
    let (src_w, src_h) = (plan.source_width as i64, plan.source_height as i64);
    let pieces = timeline_pieces(plan, opts.keeps);
    if pieces.is_empty() {
        return Err(graph_error("The edit contains no video"));
    }
    let window_end = plan.shots[plan.shots.len() - 1].end_ms;
    let fps = opts.fps;
    let rate = Rational::parse(fps).ok_or_else(|| graph_error("invalid frame rate"))?;
    if rate.num <= 0 {
        return Err(graph_error("Frame rate must be positive"));
    }
    let frame_at = |ms: i64| -> i64 {
        // int(Fraction(ms, 1000) * rate + 1/2), truncating toward zero.
        let num = ms as i128 * rate.num as i128 * 2 + 1000 * rate.den as i128;
        let den = 2000 * rate.den as i128;
        (num / den) as i64
    };

    let mut video_pieces: Vec<(usize, i64, i64)> = Vec::new();
    let (mut elapsed_ms, mut frame_end) = (0i64, 0i64);
    for &(i, start, end) in &pieces {
        let start_frame = frame_at(start - elapsed_ms) + frame_end;
        elapsed_ms += end - start;
        let next_frame = frame_at(elapsed_ms);
        let count = next_frame - frame_end;
        if count != 0 {
            video_pieces.push((i, start_frame, count));
        }
        frame_end = next_frame;
    }
    if video_pieces.is_empty() {
        return Err(graph_error("The edit is shorter than one video frame"));
    }
    let n = video_pieces.len();
    let mut parts: Vec<String> = vec![format!(
        "[0:v]fps={fps}:start_time=0:round=near,tpad=stop_mode=clone:stop_duration={:.3}[clocked]",
        window_end as f64 / 1000.0
    )];
    if n == 1 {
        parts.push("[clocked]null[s0]".into());
    } else {
        parts.push(format!("[clocked]split={n}{}", (0..n).map(|k| format!("[s{k}]")).collect::<String>()));
    }
    for (k, &(i, start_frame, count)) in video_pieces.iter().enumerate() {
        parts.push(format!("[s{k}]trim=start_frame={start_frame}:end_frame={}[t{k}]", start_frame + count));
        let chain = shot_chain(k, &plan.shots[i], src_w, src_h, out_w, out_h, opts.landscape);
        let tail = format!("[v{k}]");
        let cut = chain.rfind(&tail).unwrap_or(chain.len());
        parts.push(format!("{}[c{k}]", &chain[..cut]));
        parts.push(format!("[c{k}]setpts=PTS-STARTPTS[v{k}]"));
    }
    let inputs: String = (0..n).map(|k| format!("[v{k}]")).collect();
    parts.push(format!("{inputs}concat=n={n}:v=1:a=0,settb=expr=1/({fps}),setpts=N,fps={fps}[base]"));

    if opts.with_audio {
        let default = [(0, window_end)];
        let audio_keeps = opts.keeps.unwrap_or(&default);
        let audio_n = audio_keeps.len();
        parts.push(format!("anullsrc=r=48000:cl=stereo:d={:.3}[silence]", window_end as f64 / 1000.0));
        parts.push(format!("[0:a:0]{AUDIO_FORMAT},{AUDIO_SYNC}[clocked_audio]"));
        let source_audio = format!(
            "[silence][clocked_audio]amix=inputs=2:duration=first:dropout_transition=0:normalize=0,atrim=end={:.3}",
            window_end as f64 / 1000.0
        );
        if audio_n == 1 {
            parts.push(format!("{source_audio}[as0]"));
        } else {
            parts.push(format!(
                "{source_audio},asplit={audio_n}{}",
                (0..audio_n).map(|k| format!("[as{k}]")).collect::<String>()
            ));
        }
        for (k, &(start, end)) in audio_keeps.iter().enumerate() {
            let fades = audio_fades((end - start) as f64 / 1000.0, k == 0, k == audio_n - 1);
            parts.push(format!(
                "[as{k}]atrim=start={:.3}:end={:.3},asetpts=PTS-{:.3}/TB{fades}[a{k}]",
                start as f64 / 1000.0,
                end as f64 / 1000.0,
                start as f64 / 1000.0
            ));
        }
        let inputs: String = (0..audio_n).map(|k| format!("[a{k}]")).collect();
        let kept: i64 = audio_keeps.iter().map(|(s, e)| e - s).sum();
        let loudness = opts.loudness_filter.map(str::to_string).unwrap_or_else(loudness_filter_default);
        parts.push(format!(
            "{inputs}concat=n={audio_n}:v=0:a=1,{loudness},{}asettb=1/48000,asetpts=N[aout]",
            speed_audio_filter(opts.video_speed, kept)?
        ));
    }
    Ok(parts.join(";"))
}

// ------------------------------------------------------------------
// Overlay placement per layout (output pixels)
// ------------------------------------------------------------------

pub const FILL_CAPTION_Y: i64 = 1340;
pub const FILL_BANNER_Y: i64 = 1590;
pub const TOP_TITLE_Y: i64 = 110;

/// (ASS alignment, y) for captions during this shot.
pub fn caption_anchor(shot: &ShotLayout, src_w: i64, src_h: i64, out_w: i64, out_h: i64) -> (i64, i64) {
    if shot.layout == TWO_SHOT || (shot.layout == SCREEN_CAM && shot.cam_box.is_some()) {
        return (5, stacked_panel_heights(shot, src_h, out_h).0);
    }
    if shot.layout == SCREEN || shot.layout == SCREEN_CAM {
        let (scaled_h, overlay_y) = letterbox_geometry(src_w, src_h, out_w, out_h);
        let bar = out_h - (overlay_y + scaled_h);
        if bar >= 200 {
            return (2, out_h - pyfmt::int(bar as f64 * 0.55));
        }
    }
    (5, pyfmt::int((out_h * FILL_CAPTION_Y) as f64 / 1920.0))
}

pub fn title_y(shot: &ShotLayout, src_w: i64, src_h: i64, out_w: i64, out_h: i64, title_h: i64) -> i64 {
    if shot.layout == SCREEN {
        let (_, overlay_y) = letterbox_geometry(src_w, src_h, out_w, out_h);
        if overlay_y > title_h + 20 {
            return 10.max(overlay_y.div_euclid(2) - title_h.div_euclid(2));
        }
    }
    pyfmt::int((out_h * TOP_TITLE_Y) as f64 / 1920.0)
}

pub fn banner_y(shot: &ShotLayout, src_w: i64, src_h: i64, out_w: i64, out_h: i64) -> i64 {
    if shot.layout == SCREEN {
        let (scaled_h, overlay_y) = letterbox_geometry(src_w, src_h, out_w, out_h);
        if out_h - (overlay_y + scaled_h) >= 120 {
            return overlay_y + scaled_h + 25;
        }
    }
    pyfmt::int((out_h * FILL_BANNER_Y) as f64 / 1920.0)
}

/// Expression of `t` that takes values[i] during shot i.
pub fn per_shot_expr(plan: &ClipLayoutPlan, values: &[i64]) -> String {
    if values.iter().all(|v| *v == values[0]) {
        return format!("{}", values[0]);
    }
    let boundaries: Vec<f64> = plan.shots[..plan.shots.len() - 1].iter().map(|s| s.end_ms as f64 / 1000.0).collect();
    step_expr(&boundaries, values)
}

// ------------------------------------------------------------------
// Faces on the output frame, for caption placement
// ------------------------------------------------------------------

/// (x0, y0, x1, y1) in output pixels.
pub type Rect = (i64, i64, i64, i64);

pub const FACE_HOLD_MS: i64 = 375;
pub const MIN_ZONE_FACE_PX: f64 = 48.0;

/// Faces visible on the output frame during [start_ms, end_ms) of output time.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceZone {
    pub start_ms: i64,
    pub end_ms: i64,
    pub rects: Vec<Rect>,
}

type View = ((f64, f64, f64, f64), (i64, i64, i64, i64));

/// Where the source lands on the output at window time t_ms: one
/// (source crop, output rect) pair per panel, both as (x, y, w, h).
pub fn shot_views(shot: &ShotLayout, t_ms: i64, src_w: i64, src_h: i64, out_w: i64, out_h: i64) -> Vec<View> {
    if shot.layout == TALKING_HEAD {
        let (crop_w, crop_h, xs, ys) = fill_crop_path(shot, src_w, src_h, out_w, out_h);
        let t = t_ms as f64 / 1000.0;
        let x = if crop_w < src_w { path_value_at(&xs, t) } else { 0.0 };
        let y = if crop_h < src_h { path_value_at(&ys, t) } else { 0.0 };
        return vec![((x, y, crop_w as f64, crop_h as f64), (0, 0, out_w, out_h))];
    }
    let (top_h, bottom_h) = stacked_panel_heights(shot, src_h, out_h);
    let as_view = |r: CropRect| (r.2 as f64, r.3 as f64, r.0 as f64, r.1 as f64);
    if shot.layout == TWO_SHOT && shot.people.len() >= 2 {
        let (left, right) = (&shot.people[0], &shot.people[1]);
        let mid = (left.cx() + right.cx()) / 2.0 * src_w as f64;
        let top = person_crop(left, src_w, src_h, out_w, top_h, Some((0.0, mid)));
        let bottom = person_crop(right, src_w, src_h, out_w, bottom_h, Some((mid, src_w as f64)));
        return vec![(as_view(top), (0, 0, out_w, top_h)), (as_view(bottom), (0, top_h, out_w, bottom_h))];
    }
    if shot.layout == SCREEN_CAM {
        if let Some(cam_box) = &shot.cam_box {
            let screen = screen_crop(
                shot.screen_box.as_ref(), shot.screen_focus.as_ref(), src_w, src_h, out_w, top_h, Some(cam_box),
            );
            let cam = cam_crop(cam_box, shot.cam_face.as_ref(), src_w, src_h, out_w, bottom_h);
            let cam_dest = match panel_fit(cam, out_w, bottom_h) {
                None => (0, top_h, out_w, bottom_h),
                Some((fw, fh)) => ((out_w - fw).div_euclid(2), top_h + (bottom_h - fh).div_euclid(2), fw, fh),
            };
            return vec![(as_view(screen), (0, 0, out_w, top_h)), (as_view(cam), cam_dest)];
        }
    }
    let (scaled_h, overlay_y) = letterbox_geometry(src_w, src_h, out_w, out_h);
    vec![((0.0, 0.0, src_w as f64, src_h as f64), (0, overlay_y, out_w, scaled_h))]
}

/// Output-pixel rects of the faces (normalized source boxes) visible at t_ms.
pub fn face_rects(shot: &ShotLayout, faces: &[LBox], t_ms: i64, src_w: i64, src_h: i64, out_w: i64, out_h: i64) -> Vec<Rect> {
    let (src_wf, src_hf) = (src_w as f64, src_h as f64);
    let mut rects = Vec::new();
    for ((cx, cy, cw, ch), (dx, dy, dw, dh)) in shot_views(shot, t_ms, src_w, src_h, out_w, out_h) {
        let (dx, dy, dw, dh) = (dx as f64, dy as f64, dw as f64, dh as f64);
        let (sx, sy) = (dw / cw, dh / ch);
        for face in faces {
            let x0 = (dx + (face.x * src_wf - cx) * sx).max(dx);
            let y0 = (dy + (face.y * src_hf - cy) * sy).max(dy);
            let x1 = (dx + ((face.x + face.w) * src_wf - cx) * sx).min(dx + dw);
            let y1 = (dy + ((face.y + face.h) * src_hf - cy) * sy).min(dy + dh);
            if x1 - x0 >= MIN_ZONE_FACE_PX / 2.0 && face.h * src_hf * sy >= MIN_ZONE_FACE_PX && y1 > y0 {
                rects.push((pyfmt::round(x0), pyfmt::round(y0), pyfmt::round(x1), pyfmt::round(y1)));
            }
        }
    }
    rects
}

fn shot_faces(shot: &ShotLayout) -> Vec<LBox> {
    if shot.layout == TALKING_HEAD || shot.layout == TWO_SHOT {
        return shot.people.clone();
    }
    if shot.layout == SCREEN_CAM {
        if let Some(face) = shot.cam_face {
            return vec![face];
        }
    }
    Vec::new()
}

/// Faces on the output frame over the edited timeline (9:16 only).
pub fn face_zones(plan: &ClipLayoutPlan, time_map: &TimeMap, out_w: i64, out_h: i64) -> Vec<FaceZone> {
    let (src_w, src_h) = (plan.source_width as i64, plan.source_height as i64);
    let mut samples: Vec<&(i64, Vec<LBox>)> = plan.face_samples.iter().collect();
    samples.sort_by_key(|s| s.0);
    let mut zones = Vec::new();
    for shot in &plan.shots {
        let mut in_shot: Vec<(i64, Vec<LBox>)> = samples
            .iter()
            .filter(|(t, _)| shot.start_ms <= *t && *t < shot.end_ms)
            .map(|(t, f)| (*t, f.clone()))
            .collect();
        let mut hold = FACE_HOLD_MS;
        if !in_shot.iter().any(|(_, faces)| !faces.is_empty()) {
            in_shot = vec![((shot.start_ms + shot.end_ms).div_euclid(2), shot_faces(shot))];
            hold = shot.end_ms - shot.start_ms;
        }
        for (t, faces) in in_shot {
            if faces.is_empty() {
                continue;
            }
            let rects = face_rects(shot, &faces, t, src_w, src_h, out_w, out_h);
            if rects.is_empty() {
                continue;
            }
            for (keep_start, keep_end) in time_map.pieces_within(shot.start_ms, shot.end_ms) {
                let (lo, hi) = (keep_start.max(t - hold), keep_end.min(t + hold));
                if hi > lo {
                    if let (Some(a), Some(b)) = (time_map.to_output(lo), time_map.to_output(hi)) {
                        zones.push(FaceZone { start_ms: a, end_ms: b, rects: rects.clone() });
                    }
                }
            }
        }
    }
    zones.sort_by_key(|z| z.start_ms);
    zones
}

pub const CAPTION_TOP_LIMIT: f64 = 0.15;
pub const CAPTION_BOTTOM_LIMIT: f64 = 0.80;
pub const FACE_PAD_X: f64 = 0.15;
pub const FACE_PAD_TOP: f64 = 0.25;
pub const FACE_PAD_BOTTOM: f64 = 0.12;
pub const MAX_FACE_PAD_BOTTOM_PX: f64 = 40.0;
pub const CAPTION_FACE_GAP: f64 = 12.0;

/// Picks each caption group's position so it doesn't cover a face.
///
/// A group starts at its shot's anchor (caption_anchor). If that would cover
/// a face visible at any time while the group is up, it moves just below or
/// above the faces, whichever is nearer the anchor, inside the band clear of
/// the title and banner. Later groups in the same shot keep a moved position
/// while it stays clear, so captions don't hop between phrases.
#[derive(Debug, Clone)]
pub struct CaptionPlacer {
    /// (until_ms, alignment, y)
    pub anchors: Vec<(i64, i64, i64)>,
    zones: Vec<FaceZone>,
    starts: Vec<i64>,
    longest: i64,
    out_w: i64,
    out_h: i64,
    last: Option<(usize, f64)>,
}

impl CaptionPlacer {
    pub fn new(anchors: Vec<(i64, i64, i64)>, mut zones: Vec<FaceZone>, out_w: i64, out_h: i64) -> Self {
        zones.sort_by_key(|z| z.start_ms);
        let starts = zones.iter().map(|z| z.start_ms).collect();
        let longest = zones.iter().map(|z| z.end_ms - z.start_ms).max().unwrap_or(0);
        Self { anchors, zones, starts, longest, out_w, out_h, last: None }
    }

    pub fn faces_during(&self, start_ms: i64, end_ms: i64) -> Vec<Rect> {
        let first = self.starts.partition_point(|s| *s < start_ms - self.longest);
        let last = self.starts.partition_point(|s| *s < end_ms);
        let mut out = Vec::new();
        if first < last {
            for z in &self.zones[first..last] {
                if z.end_ms > start_ms {
                    out.extend(z.rects.iter().copied());
                }
            }
        }
        out
    }

    /// (ASS alignment, y) for a caption block on screen during [start_ms, end_ms).
    pub fn place(&mut self, start_ms: i64, end_ms: i64, block_w: i64, block_h: i64) -> (i64, i64) {
        let index = self
            .anchors
            .iter()
            .position(|(until, _, _)| start_ms < *until)
            .unwrap_or(self.anchors.len().saturating_sub(1));
        let (_, alignment, anchor_y) = self.anchors[index];
        let half = block_h as f64 / 2.0;
        let anchor = anchor_y as f64;
        let default = match alignment {
            1..=3 => anchor - half,
            7..=9 => anchor + half,
            _ => anchor,
        };

        let (left, right) = ((self.out_w - block_w) as f64 / 2.0, (self.out_w + block_w) as f64 / 2.0);
        let (mut faces, mut keep_clear) = (Vec::new(), Vec::new());
        for (x0, y0, x1, y1) in self.faces_during(start_ms, end_ms) {
            let (x0, y0, x1, y1) = (x0 as f64, y0 as f64, x1 as f64, y1 as f64);
            let (w, h) = (x1 - x0, y1 - y0);
            if x1 + w * FACE_PAD_X > left && x0 - w * FACE_PAD_X < right {
                faces.push((y0, y1));
                keep_clear.push((
                    y0 - h * FACE_PAD_TOP - CAPTION_FACE_GAP,
                    y1 + (h * FACE_PAD_BOTTOM).min(MAX_FACE_PAD_BOTTOM_PX) + CAPTION_FACE_GAP,
                ));
            }
        }

        let out_h = self.out_h as f64;
        let (mut top, mut bottom) = (out_h * CAPTION_TOP_LIMIT + half, out_h * CAPTION_BOTTOM_LIMIT - half);
        if top > bottom {
            top = out_h / 2.0;
            bottom = top;
        }
        let in_band = |c: f64| {
            let m = if c > top { c } else { top };
            if bottom < m {
                bottom
            } else {
                m
            }
        };
        let overlap = |c: f64, spans: &[(f64, f64)]| -> f64 {
            spans.iter().map(|&(a, b)| 0f64.max((c + half).min(b) - (c - half).max(a))).sum()
        };

        let mut candidates: Vec<(f64, i32)> = vec![(default, 1)];
        if let Some((i, c)) = self.last {
            if i == index {
                candidates.push((c, 0));
            }
        }
        for &(a, b) in &keep_clear {
            candidates.push((in_band(b + half), 2));
            candidates.push((in_band(a - half), 2));
        }
        candidates.push((top, 2));
        candidates.push((bottom, 2));

        let key = |c: &(f64, i32)| (4.0 * overlap(c.0, &faces) + overlap(c.0, &keep_clear), c.1, (c.0 - default).abs());
        let mut best = candidates[0];
        let mut best_key = key(&best);
        for c in &candidates[1..] {
            let k = key(c);
            let less = k.0 < best_key.0 || (k.0 == best_key.0 && (k.1 < best_key.1 || (k.1 == best_key.1 && k.2 < best_key.2)));
            if less {
                best = *c;
                best_key = k;
            }
        }
        let center = best.0;
        self.last = Some((index, center));
        if center == default {
            return (alignment, anchor_y);
        }
        (5, pyfmt::round(center))
    }
}
