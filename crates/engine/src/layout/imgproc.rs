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

#[cfg(test)]
mod tests {
    use super::*;

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
