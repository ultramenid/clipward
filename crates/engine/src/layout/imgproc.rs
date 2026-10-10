//! The few OpenCV image operations the analyzer used, ported for 8-bit BGR frames:
//! `resize` (INTER_LINEAR), `cvtColor(BGR2HSV)`, `calcHist` over H/S,
//! `normalize` (L2) and `compareHist(HISTCMP_BHATTACHARYYA)`.

/// Histogram bins over (hue, saturation), as `calcHist(..., [24, 16], [0, 180, 0, 256])`.
pub const HIST_H_BINS: usize = 24;
pub const HIST_S_BINS: usize = 16;
pub const HIST_LEN: usize = HIST_H_BINS * HIST_S_BINS;
/// Frames are shrunk to this size before the histogram (`cv2.resize(image, (160, 90))`).
pub const HIST_W: usize = 160;
pub const HIST_H: usize = 90;

/// Bilinear resize of a packed 3-channel image with OpenCV's INTER_LINEAR sample
/// positions (pixel centers, clamped at the border). Floating point rather than
/// OpenCV's fixed-point taps, so values can differ by one level.
pub fn resize_linear_3c(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    fn taps(dst: usize, src_len: usize, dst_len: usize) -> (usize, f32) {
        let scale = src_len as f64 / dst_len as f64;
        let f = (dst as f64 + 0.5) * scale - 0.5;
        let mut s = f.floor();
        let mut frac = f - s;
        if s < 0.0 {
            s = 0.0;
            frac = 0.0;
        }
        if s as usize >= src_len.saturating_sub(1) {
            s = src_len.saturating_sub(1) as f64;
            frac = 0.0;
        }
        (s as usize, frac as f32)
    }
    let xs: Vec<(usize, f32)> = (0..dw).map(|x| taps(x, sw, dw)).collect();
    let mut out = vec![0u8; dw * dh * 3];
    for dy in 0..dh {
        let (sy, fy) = taps(dy, sh, dh);
        let sy1 = (sy + 1).min(sh - 1);
        let row0 = &src[sy * sw * 3..(sy + 1) * sw * 3];
        let row1 = &src[sy1 * sw * 3..(sy1 + 1) * sw * 3];
        for (dx, &(sx, fx)) in xs.iter().enumerate() {
            let sx1 = (sx + 1).min(sw - 1);
            for c in 0..3 {
                let top = row0[sx * 3 + c] as f32 * (1.0 - fx) + row0[sx1 * 3 + c] as f32 * fx;
                let bottom = row1[sx * 3 + c] as f32 * (1.0 - fx) + row1[sx1 * 3 + c] as f32 * fx;
                let v = top * (1.0 - fy) + bottom * fy;
                out[(dy * dw + dx) * 3 + c] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

const HSV_SHIFT: i32 = 12;

fn sdiv(v: i32) -> i32 {
    if v == 0 {
        0
    } else {
        ((255i64 << HSV_SHIFT) as f64 / v as f64).round_ties_even() as i32
    }
}

fn hdiv180(d: i32) -> i32 {
    if d == 0 {
        0
    } else {
        ((180i64 << HSV_SHIFT) as f64 / (6.0 * d as f64)).round_ties_even() as i32
    }
}

/// OpenCV's exact 8-bit BGR -> HSV (H in 0..180), fixed-point tables included.
pub fn bgr_to_hsv(b: u8, g: u8, r: u8) -> (u8, u8, u8) {
    let (b, g, r) = (b as i32, g as i32, r as i32);
    let v = b.max(g).max(r);
    let vmin = b.min(g).min(r);
    let diff = v - vmin;
    let vr: i32 = if v == r { -1 } else { 0 };
    let vg: i32 = if v == g { -1 } else { 0 };
    let s = (diff * sdiv(v) + (1 << (HSV_SHIFT - 1))) >> HSV_SHIFT;
    let mut h = (vr & (g - b)) + (!vr & ((vg & (b - r + 2 * diff)) + (!vg & (r - g + 4 * diff))));
    h = (h * hdiv180(diff) + (1 << (HSV_SHIFT - 1))) >> HSV_SHIFT;
    if h < 0 {
        h += 180;
    }
    (h.clamp(0, 255) as u8, s.clamp(0, 255) as u8, v as u8)
}

/// L2-normalized 24x16 hue/saturation histogram of a BGR frame, computed on a
/// 160x90 downscale (row-major: hue bin, then saturation bin).
pub fn hs_histogram(bgr: &[u8], width: usize, height: usize) -> Vec<f32> {
    let small = resize_linear_3c(bgr, width, height, HIST_W, HIST_H);
    let mut hist = vec![0f32; HIST_LEN];
    for px in small.chunks_exact(3) {
        let (h, s, _) = bgr_to_hsv(px[0], px[1], px[2]);
        // calcHist's uniform 8-bit lookup: bin = floor(value * bins / range).
        let hb = (h as usize * HIST_H_BINS) / 180;
        let sb = (s as usize * HIST_S_BINS) / 256;
        if hb < HIST_H_BINS && sb < HIST_S_BINS {
            hist[hb * HIST_S_BINS + sb] += 1.0;
        }
    }
    normalize_l2(&mut hist);
    hist
}

/// `cv2.normalize(hist, hist)`: NORM_L2 to 1; an all-zero histogram stays zero.
pub fn normalize_l2(hist: &mut [f32]) {
    let norm = hist.iter().map(|&v| v as f64 * v as f64).sum::<f64>().sqrt();
    let scale = if norm > f64::EPSILON { 1.0 / norm } else { 0.0 };
    for v in hist.iter_mut() {
        *v = (*v as f64 * scale) as f32;
    }
}

/// `cv2.compareHist(a, b, cv2.HISTCMP_BHATTACHARYYA)`.
pub fn compare_hist_bhattacharyya(a: &[f32], b: &[f32]) -> f64 {
    let (mut s1, mut s2, mut result) = (0f64, 0f64, 0f64);
    for (&x, &y) in a.iter().zip(b) {
        let (x, y) = (x as f64, y as f64);
        result += (x * y).sqrt();
        s1 += x;
        s2 += y;
    }
    s1 *= s2;
    let s1 = if s1.abs() > f32::EPSILON as f64 { 1.0 / s1.sqrt() } else { 1.0 };
    (1.0 - result * s1).max(0.0).sqrt()
}

pub const THUMB_W: usize = 32;
pub const THUMB_H: usize = 18;

/// 32x18 luma thumbnail: catches cuts between angles of the same set, which
/// share colors (and so a histogram) but not a layout.
pub fn luma_thumb(bgr: &[u8], width: usize, height: usize) -> Vec<u8> {
    resize_linear_3c(bgr, width, height, THUMB_W, THUMB_H)
        .chunks_exact(3)
        .map(|p| (0.114 * p[0] as f32 + 0.587 * p[1] as f32 + 0.299 * p[2] as f32).round() as u8)
        .collect()
}

/// Mean absolute difference of two luma thumbnails (0-255).
pub fn thumb_diff(a: &[u8], b: &[u8]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len().max(1) as f64
}

/// 2x box downscale of a packed 3-channel image with even dimensions: the
/// detector's frame from the sharper one face patches are read from.
pub fn halve_3c(src: &[u8], sw: usize, sh: usize) -> Vec<u8> {
    let (dw, dh) = (sw / 2, sh / 2);
    let mut out = vec![0u8; dw * dh * 3];
    if dw == 0 {
        return out;
    }
    // Fixed-size chunks instead of indexing: no bounds checks in the loop.
    for (o, rows) in out.chunks_exact_mut(dw * 3).zip(src.chunks_exact(2 * sw * 3)) {
        let (top, bottom) = rows.split_at(sw * 3);
        for ((o, t), b) in o.as_chunks_mut::<3>().0.iter_mut().zip(top.as_chunks::<6>().0).zip(bottom.as_chunks::<6>().0) {
            for c in 0..3 {
                let sum = t[c] as u32 + t[3 + c] as u32 + b[c] as u32 + b[3 + c] as u32;
                o[c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    out
}

pub const FACE_PATCH_W: usize = 16;
pub const FACE_PATCH_H: usize = 18;
/// Patch rows above this hold the eyes and nose bridge, which move only with
/// the head; the rows from it down hold the mouth and chin.
pub const FACE_PATCH_SPLIT: usize = 10;
/// Cells a patch may slide while aligning (detector jitter, head movement).
const FACE_PATCH_SLIDE: i32 = 2;

/// Luma patch of a face from above the eyes to below the chin, laid out along
/// the eyes-to-mouth axis so roll and zoom cancel out, normalized to the upper
/// face's mean and spread so lighting does too. `row` is a YuNet detection on a frame
/// `scale` times smaller than `bgr`. Faces whose eyes-to-mouth distance is
/// under `min_unit_px` get an empty patch (no talk signal): their mouth is a
/// few noisy pixels.
pub fn face_patch(bgr: &[u8], width: usize, height: usize, row: &[f32; 15], scale: f32, min_unit_px: f32) -> Vec<f32> {
    let eye = ((row[4] + row[6]) / 2.0 * scale, (row[5] + row[7]) / 2.0 * scale);
    let mouth = ((row[10] + row[12]) / 2.0 * scale, (row[11] + row[13]) / 2.0 * scale);
    let (dx, dy) = (mouth.0 - eye.0, mouth.1 - eye.1);
    let unit = dx.hypot(dy);
    if unit.is_nan() || unit < min_unit_px || width == 0 || height == 0 {
        return Vec::new();
    }
    // Unit vectors down the face and across it; eight cells per eyes-to-mouth
    // distance, the eyes on the fifth row and the mouth on the thirteenth.
    let (down, across) = ((dx / unit, dy / unit), (dy / unit, -dx / unit));
    let cell = unit / 8.0;
    let mut patch = Vec::with_capacity(FACE_PATCH_W * FACE_PATCH_H);
    for j in 0..FACE_PATCH_H {
        for i in 0..FACE_PATCH_W {
            // Four samples per cell: a single pixel would alias on stubble and teeth.
            let mut sum = 0.0;
            for (ox, oy) in [(-0.25, -0.25), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)] {
                let lx = (i as f32 + 0.5 + ox - FACE_PATCH_W as f32 / 2.0) * cell;
                let ly = (j as f32 + 0.5 + oy) * cell - 0.55 * unit;
                let x = ((eye.0 + across.0 * lx + down.0 * ly).max(0.0) as usize).min(width - 1);
                let y = ((eye.1 + across.1 * lx + down.1 * ly).max(0.0) as usize).min(height - 1);
                let p = &bgr[(y * width + x) * 3..];
                sum += 0.114 * p[0] as f32 + 0.587 * p[1] as f32 + 0.299 * p[2] as f32;
            }
            patch.push(sum / 4.0);
        }
    }
    // Normalized by the upper face alone, so an opening mouth can't shift it.
    let upper = &patch[..FACE_PATCH_SPLIT * FACE_PATCH_W];
    let mean = upper.iter().sum::<f32>() / upper.len() as f32;
    let spread = (upper.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / upper.len() as f32).sqrt().max(4.0);
    patch.iter_mut().for_each(|v| *v = (*v - mean) / spread);
    patch
}

/// How much two face patches differ once `b` is slid onto `a` by its eyes and
/// nose: (what is left in the upper face, what is left around the mouth), as
/// mean absolute differences. The upper face only changes with the head and
/// the detector, so it measures everything that is not the mouth moving.
/// None unless both are `face_patch` results.
pub fn face_motion(a: &[f32], b: &[f32]) -> Option<(f64, f64)> {
    if a.len() != FACE_PATCH_W * FACE_PATCH_H || b.len() != a.len() {
        return None;
    }
    let margin = FACE_PATCH_SLIDE as usize;
    let rest = |sx: i32, sy: i32, rows: std::ops::Range<usize>| -> f64 {
        let cells = rows.len() * (FACE_PATCH_W - 2 * margin);
        let mut sum = 0.0;
        for j in rows {
            for i in margin..FACE_PATCH_W - margin {
                let moved = (j as i32 + sy) as usize * FACE_PATCH_W + (i as i32 + sx) as usize;
                sum += (a[j * FACE_PATCH_W + i] - b[moved]).abs() as f64;
            }
        }
        sum / cells as f64
    };
    // Unmoved unless sliding fits the upper face strictly better.
    let mut best = (rest(0, 0, margin..FACE_PATCH_SPLIT), rest(0, 0, FACE_PATCH_SPLIT..FACE_PATCH_H - margin));
    for sy in -FACE_PATCH_SLIDE..=FACE_PATCH_SLIDE {
        for sx in -FACE_PATCH_SLIDE..=FACE_PATCH_SLIDE {
            let upper = rest(sx, sy, margin..FACE_PATCH_SPLIT);
            if upper < best.0 {
                best = (upper, rest(sx, sy, FACE_PATCH_SPLIT..FACE_PATCH_H - margin));
            }
        }
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A YuNet row for an upright face: eyes level at `eye_y`, mouth `unit` below.
    fn face_row(cx: f32, eye_y: f32, unit: f32) -> [f32; 15] {
        let mut row = [0f32; 15];
        (row[4], row[5], row[6], row[7]) = (cx - unit / 2.0, eye_y, cx + unit / 2.0, eye_y);
        (row[10], row[11], row[12], row[13]) = (cx - unit / 3.0, eye_y + unit, cx + unit / 3.0, eye_y + unit);
        row
    }

    /// A textured face on a 96x96 frame; `open` darkens a slot where the mouth is.
    fn face_frame(gain: u8, shift: usize, open: bool) -> Vec<u8> {
        (0..96 * 96)
            .flat_map(|i| {
                let (x, y) = ((i % 96 + 96 - shift) % 96, i / 96);
                let mouth = open && (40..56).contains(&x) && (58..64).contains(&y);
                let v = if mouth { 10 } else { ((x * 5 + y * 3) % 97) as u8 + 40 } + gain;
                [v, v, v]
            })
            .collect()
    }

    #[test]
    fn face_patch_ignores_lighting_and_tiny_faces() {
        let row = face_row(48.0, 36.0, 24.0);
        let a = face_patch(&face_frame(0, 0, false), 96, 96, &row, 1.0, 12.0);
        let b = face_patch(&face_frame(60, 0, false), 96, 96, &row, 1.0, 12.0);
        assert_eq!(a.len(), FACE_PATCH_W * FACE_PATCH_H);
        assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 0.05));
        assert!(face_patch(&face_frame(0, 0, false), 96, 96, &row, 1.0, 30.0).is_empty());
        // A detection on the half-size frame reads the same pixels.
        let half = face_row(24.0, 18.0, 12.0);
        assert_eq!(face_patch(&face_frame(0, 0, false), 96, 96, &half, 2.0, 12.0), a);
        assert!(face_patch(&[], 0, 0, &row, 1.0, 12.0).is_empty());
    }

    #[test]
    fn face_motion_sees_the_mouth_not_the_head() {
        let row = face_row(48.0, 36.0, 24.0);
        let patch = |shift: usize, open: bool| face_patch(&face_frame(0, shift, open), 96, 96, &row, 1.0, 12.0);
        let (closed, open) = (patch(0, false), patch(0, true));
        // The same face: nothing left. The detector landing 3 px off: slid back.
        assert_eq!(face_motion(&closed, &closed), Some((0.0, 0.0)));
        let (upper, lower) = face_motion(&closed, &patch(3, false)).unwrap();
        assert!(upper < 0.05 && lower < 0.05, "{upper} {lower}");
        // An opening mouth stays in the lower face.
        let (upper, lower) = face_motion(&closed, &open).unwrap();
        assert!(upper < 0.05 && lower > 0.15, "{upper} {lower}");
        assert_eq!(face_motion(&closed, &[]), None);
    }

    #[test]
    fn halve_averages_blocks() {
        let src: Vec<u8> = [[0u8, 10, 20], [4, 10, 20], [0, 30, 20], [4, 30, 22]].concat();
        assert_eq!(halve_3c(&src, 2, 2), vec![2, 20, 21]);
        let flat: Vec<u8> = [10u8, 20, 200].repeat(8 * 4);
        assert!(halve_3c(&flat, 8, 4).chunks_exact(3).all(|p| p == [10, 20, 200]));
    }

    #[test]
    fn hsv_matches_opencv_reference_values() {
        // cv2.cvtColor(np.uint8([[[b, g, r]]]), cv2.COLOR_BGR2HSV)
        assert_eq!(bgr_to_hsv(0, 0, 255), (0, 255, 255));
        assert_eq!(bgr_to_hsv(0, 255, 0), (60, 255, 255));
        assert_eq!(bgr_to_hsv(255, 0, 0), (120, 255, 255));
        assert_eq!(bgr_to_hsv(128, 128, 128), (0, 0, 128));
        assert_eq!(bgr_to_hsv(0, 0, 0), (0, 0, 0));
        assert_eq!(bgr_to_hsv(255, 0, 255), (150, 255, 255));
        assert_eq!(bgr_to_hsv(50, 100, 200), (10, 191, 200));
    }

    #[test]
    fn bhattacharyya_identical_is_zero_and_disjoint_is_one() {
        let mut a = vec![0f32; HIST_LEN];
        let mut b = vec![0f32; HIST_LEN];
        a[3] = 1.0;
        b[40] = 1.0;
        assert!(compare_hist_bhattacharyya(&a, &a) < 1e-6);
        assert!((compare_hist_bhattacharyya(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn resize_keeps_flat_color_and_histogram_is_unit() {
        let src: Vec<u8> = [10u8, 20, 200].repeat(640 * 360);
        let small = resize_linear_3c(&src, 640, 360, 160, 90);
        assert!(small.chunks_exact(3).all(|p| p == [10, 20, 200]));
        let hist = hs_histogram(&src, 640, 360);
        let norm: f32 = hist.iter().map(|v| v * v).sum();
        assert!((norm - 1.0).abs() < 1e-5);
    }
}
