//! YuNet face detection (face_detection_yunet_2023mar.onnx) on tract-onnx.
//!
//! Port of OpenCV's `FaceDetectorYN` (modules/objdetect/src/face_detect.cpp):
//! the input is padded to a multiple of 32 with zeros, fed as a raw BGR float
//! blob (`blobFromImage` defaults: no scaling, no mean, no channel swap), the
//! 12 stride outputs are decoded against the stride-8/16/32 grids, filtered by
//! score and reduced with `dnn::NMSBoxes` on integer boxes.
use crate::error::{EngineError, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tract_onnx::prelude::*;

const DIVISOR: usize = 32;
const STRIDES: [usize; 3] = [8, 16, 32];
const OUTPUT_NAMES: [&str; 12] = [
    "cls_8", "cls_16", "cls_32", "obj_8", "obj_16", "obj_32", "bbox_8", "bbox_16", "bbox_32", "kps_8", "kps_16",
    "kps_32",
];

type Plan = Arc<TypedRunnableModel>;

/// One detected face in input pixels:
/// (x, y, w, h, re_x, re_y, le_x, le_y, nt_x, nt_y, rcm_x, rcm_y, lcm_x, lcm_y, score).
pub type FaceRow = [f32; 15];

/// A detector bound to one input size (OpenCV's `setInputSize`). Cheap to clone;
/// the optimized plan is shared and safe to run from several threads.
#[derive(Clone)]
pub struct YuNet {
    plan: Plan,
    input_w: usize,
    input_h: usize,
    pad_w: usize,
    pad_h: usize,
    score_threshold: f32,
    nms_threshold: f32,
    top_k: usize,
}

type PlanCache = Mutex<HashMap<(PathBuf, usize, usize), Plan>>;

fn plan_cache() -> &'static PlanCache {
    static CACHE: OnceLock<PlanCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn model_error(e: impl std::fmt::Display) -> EngineError {
    EngineError::msg(format!("face detection model failed: {e:#}"))
}

impl YuNet {
    /// `FaceDetectorYN.create(model, "", (width, height), score, nms, top_k)`.
    /// Blocking: loads and optimizes the model on first use for a given size.
    pub fn new(
        model: &Path,
        width: usize,
        height: usize,
        score_threshold: f32,
        nms_threshold: f32,
        top_k: usize,
    ) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(EngineError::msg("face detection model failed: empty input size"));
        }
        let pad_w = ((width - 1) / DIVISOR + 1) * DIVISOR;
        let pad_h = ((height - 1) / DIVISOR + 1) * DIVISOR;
        let key = (model.to_path_buf(), pad_w, pad_h);
        // Held while building, so clips starting together build each plan once.
        // ponytail: one lock for every size; per-size locks if sizes ever load together.
        let mut cache = plan_cache().lock().unwrap_or_else(|e| e.into_inner());
        let plan = match cache.get(&key) {
            Some(plan) => plan.clone(),
            None => {
                // The exported graph pins a 640x640 input in its shape annotations;
                // OpenCV reshapes freely, so drop them and infer from our input fact.
                let plan = tract_onnx::onnx()
                    .with_ignore_output_shapes(true)
                    .with_ignore_value_info(true)
                    .model_for_path(model)
                    .and_then(|m| m.with_input_fact(0, f32::fact([1, 3, pad_h, pad_w]).into()))
                    .and_then(|m| m.with_outputs_by_name(OUTPUT_NAMES))
                    .and_then(|m| m.into_optimized())
                    .and_then(|m| m.into_runnable())
                    .map_err(model_error)?;
                cache.insert(key, plan.clone());
                plan
            }
        };
        Ok(Self { plan, input_w: width, input_h: height, pad_w, pad_h, score_threshold, nms_threshold, top_k })
    }

    /// `detector.detect(image)` on a packed BGR24 frame of the configured size.
    pub fn detect(&self, bgr: &[u8]) -> Result<Vec<FaceRow>> {
        let (w, h) = (self.input_w, self.input_h);
        if bgr.len() != w * h * 3 {
            return Err(EngineError::msg("face detection model failed: frame size mismatch"));
        }
        // padWithDivisor (zero border right/bottom) + blobFromImage (NCHW, BGR, f32).
        let plane = self.pad_w * self.pad_h;
        let mut blob = vec![0f32; 3 * plane];
        for y in 0..h {
            let row = &bgr[y * w * 3..(y + 1) * w * 3];
            let base = y * self.pad_w;
            for x in 0..w {
                for c in 0..3 {
                    blob[c * plane + base + x] = row[x * 3 + c] as f32;
                }
            }
        }
        let input = Tensor::from_shape(&[1, 3, self.pad_h, self.pad_w], &blob).map_err(model_error)?;
        let outputs = self.plan.run(tvec!(input.into_tvalue())).map_err(model_error)?;
        let mut data: Vec<Vec<f32>> = Vec::with_capacity(OUTPUT_NAMES.len());
        for out in outputs.iter() {
            data.push(out.to_plain_array_view::<f32>().map_err(model_error)?.iter().copied().collect());
        }
        let views: Vec<&[f32]> = data.iter().map(Vec::as_slice).collect();
        Ok(self.post_process(&views))
    }

    fn post_process(&self, blobs: &[&[f32]]) -> Vec<FaceRow> {
        let mut faces: Vec<FaceRow> = Vec::new();
        let n = STRIDES.len();
        for (i, &stride) in STRIDES.iter().enumerate() {
            let cols = self.pad_w / stride;
            let rows = self.pad_h / stride;
            let (cls, obj, bbox, kps) = (blobs[i], blobs[i + n], blobs[i + 2 * n], blobs[i + 3 * n]);
            let s = stride as f32;
            for r in 0..rows {
                for c in 0..cols {
                    let idx = r * cols + c;
                    if idx >= cls.len() || idx >= obj.len() || idx * 4 + 3 >= bbox.len() || idx * 10 + 9 >= kps.len() {
                        continue;
                    }
                    let cls_score = cls[idx].clamp(0.0, 1.0);
                    let obj_score = obj[idx].clamp(0.0, 1.0);
                    let score = (cls_score * obj_score).sqrt();
                    if score < self.score_threshold {
                        continue;
                    }
                    let cx = (c as f32 + bbox[idx * 4]) * s;
                    let cy = (r as f32 + bbox[idx * 4 + 1]) * s;
                    let bw = bbox[idx * 4 + 2].exp() * s;
                    let bh = bbox[idx * 4 + 3].exp() * s;
                    let mut face = [0f32; 15];
                    face[0] = cx - bw / 2.0;
                    face[1] = cy - bh / 2.0;
                    face[2] = bw;
                    face[3] = bh;
                    for k in 0..5 {
                        face[4 + 2 * k] = (kps[idx * 10 + 2 * k] + c as f32) * s;
                        face[4 + 2 * k + 1] = (kps[idx * 10 + 2 * k + 1] + r as f32) * s;
                    }
                    face[14] = score;
                    faces.push(face);
                }
            }
        }
        if faces.len() > 1 {
            let boxes: Vec<[i32; 4]> =
                faces.iter().map(|f| [f[0] as i32, f[1] as i32, f[2] as i32, f[3] as i32]).collect();
            let scores: Vec<f32> = faces.iter().map(|f| f[14]).collect();
            let keep = nms_boxes(&boxes, &scores, self.score_threshold, self.nms_threshold, self.top_k);
            keep.into_iter().map(|i| faces[i]).collect()
        } else {
            faces
        }
    }
}

/// Jaccard overlap of two integer rects (x, y, w, h), as OpenCV's `rectOverlap`.
fn rect_overlap(a: &[i32; 4], b: &[i32; 4]) -> f64 {
    let area_a = a[2] as i64 * a[3] as i64;
    let area_b = b[2] as i64 * b[3] as i64;
    if area_a + area_b <= 0 {
        return 1.0;
    }
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let iw = (a[0] + a[2]).min(b[0] + b[2]) - x1;
    let ih = (a[1] + a[3]).min(b[1] + b[3]) - y1;
    let inter = if iw <= 0 || ih <= 0 { 0 } else { iw as i64 * ih as i64 };
    inter as f64 / (area_a + area_b - inter) as f64
}

/// `cv::dnn::NMSBoxes(boxes, scores, score_threshold, nms_threshold, indices, eta=1, top_k)`.
pub fn nms_boxes(boxes: &[[i32; 4]], scores: &[f32], score_threshold: f32, nms_threshold: f32, top_k: usize) -> Vec<usize> {
    // GetMaxScoreIndex: strictly above the threshold, stable descending sort, top_k cut.
    let mut order: Vec<(f32, usize)> =
        scores.iter().enumerate().filter(|(_, &s)| s > score_threshold).map(|(i, &s)| (s, i)).collect();
    order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    if top_k > 0 && top_k < order.len() {
        order.truncate(top_k);
    }
    let mut keep: Vec<usize> = Vec::new();
    for &(_, idx) in &order {
        let ok = keep.iter().all(|&k| rect_overlap(&boxes[idx], &boxes[k]) <= nms_threshold as f64);
        if ok {
            keep.push(idx);
        }
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nms_suppresses_overlaps_and_keeps_order() {
        let boxes = [[10, 10, 100, 100], [12, 12, 100, 100], [300, 300, 50, 50], [0, 0, 5, 5]];
        let scores = [0.8, 0.9, 0.75, 0.5];
        // 0.5 is not above the 0.72 threshold; box 0 overlaps box 1 heavily.
        assert_eq!(nms_boxes(&boxes, &scores, 0.72, 0.3, 50), vec![1, 2]);
        assert_eq!(nms_boxes(&boxes, &scores, 0.72, 0.3, 1), vec![1]);
    }

    fn model_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/models/face_detection_yunet_2023mar.onnx")
    }

    /// Set YUNET_TEST_BGR=<file>:<width>:<height> (raw bgr24 of a portrait) to
    /// check the model finds a face; e.g. OpenCV's samples/data/lena.jpg via
    /// `ffmpeg -i lena.jpg -vf scale=640:640 -f rawvideo -pix_fmt bgr24 lena.bgr`.
    #[test]
    fn real_model_finds_face_in_sample_image() {
        let Ok(spec) = std::env::var("YUNET_TEST_BGR") else { return };
        let parts: Vec<&str> = spec.rsplitn(3, ':').collect();
        let (h, w, file): (usize, usize, &str) = (parts[0].parse().unwrap(), parts[1].parse().unwrap(), parts[2]);
        let det = YuNet::new(&model_path(), w, h, 0.72, 0.3, 50).unwrap();
        let faces = det.detect(&std::fs::read(file).unwrap()).unwrap();
        eprintln!("faces: {faces:?}");
        assert!(!faces.is_empty());
        assert!(faces[0][2] > 20.0 && faces[0][3] > 20.0 && faces[0][14] >= 0.72);
    }

    #[test]
    fn real_model_runs_on_blank_frame() {
        let path = model_path();
        if !path.is_file() {
            eprintln!("YuNet model missing; skipping");
            return;
        }
        let det = YuNet::new(&path, 640, 360, 0.72, 0.3, 50).expect("model loads");
        assert_eq!((det.pad_w, det.pad_h), (640, 384));
        let faces = det.detect(&vec![0u8; 640 * 360 * 3]).expect("inference runs");
        assert!(faces.is_empty(), "blank frame has no faces: {faces:?}");
        // Same size reuses the cached plan; another size builds its own.
        let again = YuNet::new(&path, 640, 360, 0.72, 0.3, 50).unwrap();
        assert!(Arc::ptr_eq(&det.plan, &again.plan));
        let other = YuNet::new(&path, 360, 640, 0.72, 0.3, 50).unwrap();
        assert!(other.detect(&vec![128u8; 360 * 640 * 3]).unwrap().is_empty());
    }
}
