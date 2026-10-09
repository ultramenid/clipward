//! Compares today's per-shot face tracking with tracking + SFace identity on a real video.
//! `cargo run --release -p clip_engine --example face_id_bench -- <video> <sface.onnx> <out_dir>`
//! Writes report.json plus contact sheets (one row per identity) to out_dir.
use clip_engine::layout::analysis::{classify_shot, is_corner_overlay, MIN_FACE_HEIGHT, MIN_TRACK_PRESENCE, segment_shots, track_faces, FaceTrack, FrameInfo, ANALYSIS_FPS, FACE_NMS_THRESHOLD, FACE_SCORE_THRESHOLD, FACE_TOP_K, MAX_FACES_PER_FRAME, MIN_TALK_UNIT_PX};
use clip_engine::layout::yunet::{FaceRow, YuNet};
use clip_engine::layout::{analysis_dimensions, decode_args, imgproc, Box, TWO_SHOT};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;
use tract_onnx::prelude::*;

const SFACE_MATCH: f32 = 0.363; // OpenCV's cosine threshold for "same person"
const CROP: usize = 112;
// ArcFace/SFace reference landmarks in the 112x112 crop (OpenCV alignCrop).
const DST: [[f32; 2]; 5] = [[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]];

/// Similarity transform (scale+rotation+translation) mapping the 5 landmarks onto DST,
/// then a bilinear warp to a 112x112 BGR crop.
fn align_crop(bgr: &[u8], w: usize, h: usize, row: &FaceRow) -> Vec<u8> {
    let src: Vec<[f32; 2]> = (0..5).map(|k| [row[4 + 2 * k], row[5 + 2 * k]]).collect();
    let mean = |p: &[[f32; 2]]| p.iter().fold([0.0, 0.0], |a, q| [a[0] + q[0] / 5.0, a[1] + q[1] / 5.0]);
    let (sm, dm) = (mean(&src), mean(&DST));
    let (mut a, mut b, mut n) = (0f32, 0f32, 0f32);
    for (s, d) in src.iter().zip(DST.iter()) {
        let (sx, sy, dx, dy) = (s[0] - sm[0], s[1] - sm[1], d[0] - dm[0], d[1] - dm[1]);
        a += sx * dx + sy * dy;
        b += sx * dy - sy * dx;
        n += sx * sx + sy * sy;
    }
    let (c, s) = (a / n, b / n);
    let det = c * c + s * s;
    let mut out = vec![0u8; CROP * CROP * 3];
    for v in 0..CROP {
        for u in 0..CROP {
            // Inverse map: p = R^-1 (q - dm) + sm.
            let (qx, qy) = (u as f32 - dm[0], v as f32 - dm[1]);
            let px = (c * qx + s * qy) / det + sm[0];
            let py = (-s * qx + c * qy) / det + sm[1];
            let (x0, y0) = (px.floor(), py.floor());
            if x0 < 0.0 || y0 < 0.0 || x0 as usize + 1 >= w || y0 as usize + 1 >= h {
                continue;
            }
            let (fx, fy, xi, yi) = (px - x0, py - y0, x0 as usize, y0 as usize);
            for ch in 0..3 {
                let at = |x: usize, y: usize| bgr[(y * w + x) * 3 + ch] as f32;
                let top = at(xi, yi) * (1.0 - fx) + at(xi + 1, yi) * fx;
                let bot = at(xi, yi + 1) * (1.0 - fx) + at(xi + 1, yi + 1) * fx;
                out[(v * CROP + u) * 3 + ch] = (top * (1.0 - fy) + bot * fy).round() as u8;
            }
        }
    }
    out
}

/// SFace embedding (L2-normalized). Input: RGB float 0-255, NCHW (OpenCV swapRB=true).
fn embed(model: &std::sync::Arc<TypedRunnableModel>, crop: &[u8]) -> Vec<f32> {
    let plane = CROP * CROP;
    let mut blob = vec![0f32; 3 * plane];
    for i in 0..plane {
        for ch in 0..3 {
            blob[ch * plane + i] = crop[i * 3 + (2 - ch)] as f32;
        }
    }
    let input = Tensor::from_shape(&[1, 3, CROP, CROP], &blob).unwrap();
    let out = model.run(tvec!(input.into_tvalue())).unwrap();
    let mut v: Vec<f32> = out[0].to_plain_array_view::<f32>().unwrap().iter().copied().collect();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter_mut().for_each(|x| *x /= norm);
    v
}

fn cos(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn iou(a: &Box, b: &Box) -> f64 {
    let (x1, y1) = (a.x.max(b.x), a.y.max(b.y));
    let (x2, y2) = ((a.x + a.w).min(b.x + b.w), (a.y + a.h).min(b.y + b.h));
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    inter / (a.area() + b.area() - inter).max(1e-9)
}

/// The "simple IoU tracker": greedy best-overlap match to the previous frame, per shot.
fn iou_track(frames: &[FrameInfo]) -> usize {
    let mut tracks: Vec<Box> = Vec::new(); // last box of each live track
    let mut live: Vec<usize> = Vec::new();
    let mut count = 0;
    for f in frames {
        let mut next = Vec::new();
        let mut used = vec![false; live.len()];
        for face in &f.faces {
            let best = (0..live.len()).filter(|&i| !used[i]).max_by(|&i, &j| iou(&tracks[live[i]], face).total_cmp(&iou(&tracks[live[j]], face)));
            match best {
                Some(i) if iou(&tracks[live[i]], face) >= 0.3 => {
                    used[i] = true;
                    tracks[live[i]] = *face;
                    next.push(live[i]);
                }
                _ => {
                    tracks.push(*face);
                    next.push(tracks.len() - 1);
                    count += 1;
                }
            }
        }
        live = next;
    }
    count
}

struct Face {
    t_ms: i64,
    frame: usize,
    boxn: Box,
    emb: Vec<f32>,
    crop: Vec<u8>,
}

fn write_sheet(path: &Path, rows: &[Vec<&[u8]>]) {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let (w, h) = (cols * CROP, rows.len().max(1) * CROP);
    let mut img = vec![40u8; w * h * 3];
    for (r, row) in rows.iter().enumerate() {
        for (c, crop) in row.iter().enumerate() {
            for y in 0..CROP {
                let dst = ((r * CROP + y) * w + c * CROP) * 3;
                img[dst..dst + CROP * 3].copy_from_slice(&crop[y * CROP * 3..(y + 1) * CROP * 3]);
            }
        }
    }
    let enc = jpeg_encoder::Encoder::new_file(path, 85).unwrap();
    enc.encode(&img, w as u16, h as u16, jpeg_encoder::ColorType::Bgr).unwrap();
}

/// Outline a normalized box on a BGR frame.
fn draw(img: &mut [u8], w: usize, h: usize, b: &Box, color: [u8; 3], t: usize) {
    let (x0, y0) = ((b.x * w as f64) as usize, (b.y * h as f64) as usize);
    let x1 = (((b.x + b.w) * w as f64) as usize).min(w - 1);
    let y1 = (((b.y + b.h) * h as f64) as usize).min(h - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            if x < x0 + t || x + t > x1 || y < y0 + t || y + t > y1 {
                img[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&color);
            }
        }
    }
}

/// Tile equal-size BGR images, `cols` per row.
fn tile(path: &Path, imgs: &[Vec<u8>], w: usize, h: usize, cols: usize) {
    let rows = imgs.len().div_ceil(cols);
    let (tw, th) = (cols * w, rows * h);
    let mut out = vec![40u8; tw * th * 3];
    for (i, img) in imgs.iter().enumerate() {
        let (ox, oy) = ((i % cols) * w, (i / cols) * h);
        for y in 0..h {
            let d = ((oy + y) * tw + ox) * 3;
            out[d..d + w * 3].copy_from_slice(&img[y * w * 3..(y + 1) * w * 3]);
        }
    }
    jpeg_encoder::Encoder::new_file(path, 80).unwrap().encode(&out, tw as u16, th as u16, jpeg_encoder::ColorType::Bgr).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (video, sface_path, out_dir) = (PathBuf::from(&args[1]), PathBuf::from(&args[2]), PathBuf::from(&args[3]));
    std::fs::create_dir_all(&out_dir).unwrap();
    let bin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../engine-bin");
    let yunet_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/models/face_detection_yunet_2023mar.onnx");

    let probe = Command::new(bin.join("ffprobe"))
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(&video)
        .output()
        .unwrap();
    let dims: Vec<u32> = String::from_utf8_lossy(&probe.stdout).trim().split(',').map(|s| s.parse().unwrap()).collect();
    let (w, h) = analysis_dimensions(dims[0], dims[1]);
    let (wu, hu) = (w as usize, h as usize);

    // --- decode (same args as the app) ---
    let t = Instant::now();
    let mut child = Command::new(bin.join("ffmpeg"))
        .args(decode_args(&video, 0, 3_600_000, w, h))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut raw = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut raw).unwrap();
    child.wait().unwrap();
    let decode_s = t.elapsed().as_secs_f64();
    let images: Vec<&[u8]> = raw.chunks_exact(wu * hu * 3).collect();

    // --- YuNet (same filtering as process_frame) ---
    let t = Instant::now();
    let det = YuNet::new(&yunet_path, wu, hu, FACE_SCORE_THRESHOLD, FACE_NMS_THRESHOLD, FACE_TOP_K).unwrap();
    let yunet_load_s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let mut frames: Vec<FrameInfo> = Vec::new();
    let mut rows_per_frame: Vec<Vec<FaceRow>> = Vec::new();
    for (i, img) in images.iter().enumerate() {
        let mut rows = det.detect(img).unwrap();
        rows.sort_by(|a, b| b[14].total_cmp(&a[14]));
        rows.truncate(MAX_FACES_PER_FRAME);
        rows.retain(|r| r[14] >= FACE_SCORE_THRESHOLD);
        let faces = rows
            .iter()
            .map(|r| Box { x: r[0] as f64 / w as f64, y: r[1] as f64 / h as f64, w: r[2] as f64 / w as f64, h: r[3] as f64 / h as f64 }.clamp())
            .collect();
        let patches = rows.iter().map(|r| imgproc::face_patch(img, wu, hu, r, 1.0, MIN_TALK_UNIT_PX)).collect();
        frames.push(FrameInfo { t_ms: (i as f64 * 1000.0 / ANALYSIS_FPS) as i64, faces, patches, hist: imgproc::hs_histogram(img, wu, hu), ..Default::default() });
        rows_per_frame.push(rows);
    }
    let yunet_s = t.elapsed().as_secs_f64();
    let n_faces: usize = rows_per_frame.iter().map(Vec::len).sum();

    // --- SFace on every detected face (production would embed only a few per track) ---
    let t = Instant::now();
    let sface = tract_onnx::onnx()
        .model_for_path(&sface_path)
        .and_then(|m| m.with_input_fact(0, f32::fact([1, 3, CROP, CROP]).into()))
        .and_then(|m| m.into_optimized())
        .and_then(|m| m.into_runnable())
        .unwrap();
    let sface_load_s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let mut faces: Vec<Face> = Vec::new();
    for (i, rows) in rows_per_frame.iter().enumerate() {
        for (r, boxn) in rows.iter().zip(&frames[i].faces) {
            let crop = align_crop(images[i], wu, hu, r);
            faces.push(Face { t_ms: frames[i].t_ms, frame: i, boxn: *boxn, emb: embed(&sface, &crop), crop });
        }
    }
    let sface_s = t.elapsed().as_secs_f64();

    // --- baseline: today's per-shot tracks ---
    let duration_ms = frames.last().map(|f| f.t_ms + 250).unwrap_or(0);
    let t = Instant::now();
    let shots = segment_shots(&frames, duration_ms);
    let mut tracks: Vec<(usize, FaceTrack)> = Vec::new(); // (shot index, track)
    let mut iou_tracks = 0;
    for (si, &(s, e)) in shots.iter().enumerate() {
        let sf: Vec<FrameInfo> = frames.iter().filter(|f| s <= f.t_ms && f.t_ms < e).cloned().collect();
        iou_tracks += iou_track(&sf);
        tracks.extend(track_faces(&sf).into_iter().map(|t| (si, t)));
    }
    let track_s = t.elapsed().as_secs_f64();
    let face_of = |t_ms: i64, b: &Box| faces.iter().position(|f| f.t_ms == t_ms && f.boxn == *b).unwrap();
    let track_faces_idx: Vec<Vec<usize>> = tracks.iter().map(|(_, tr)| tr.samples.iter().map(|(t, b)| face_of(*t, b)).collect()).collect();

    // ID-switch suspects inside baseline tracks: consecutive samples that SFace says are different people.
    let mut switch_suspects = 0;
    for idx in &track_faces_idx {
        switch_suspects += idx.windows(2).filter(|p| cos(&faces[p[0]].emb, &faces[p[1]].emb) < SFACE_MATCH).count();
    }

    // --- tracker + SFace: cluster tracks across shots by mean embedding ---
    let t = Instant::now();
    let track_emb: Vec<Vec<f32>> = track_faces_idx
        .iter()
        .map(|idx| {
            let mut m = vec![0f32; 128];
            for &i in idx {
                m.iter_mut().zip(&faces[i].emb).for_each(|(a, b)| *a += b);
            }
            let n = m.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            m.iter().map(|x| x / n).collect()
        })
        .collect();
    let mut order: Vec<usize> = (0..tracks.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(track_faces_idx[i].len()));
    let mut clusters: Vec<(Vec<f32>, Vec<usize>)> = Vec::new(); // (sum of track embeddings, tracks)
    for &ti in &order {
        let best = clusters
            .iter()
            .enumerate()
            .map(|(ci, (sum, _))| {
                let n = sum.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
                (ci, sum.iter().zip(&track_emb[ti]).map(|(a, b)| a / n * b).sum::<f32>())
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((ci, sim)) if sim >= SFACE_MATCH => {
                clusters[ci].0.iter_mut().zip(&track_emb[ti]).for_each(|(a, b)| *a += b);
                clusters[ci].1.push(ti);
            }
            _ => clusters.push((track_emb[ti].clone(), vec![ti])),
        }
    }
    let cluster_s = t.elapsed().as_secs_f64();

    // --- framing: today's classify_shot vs hybrid "follow the 60 s window's main person" ---
    let mut cluster_of = vec![0usize; tracks.len()];
    for (ci, (_, ts)) in clusters.iter().enumerate() {
        ts.iter().for_each(|&t| cluster_of[t] = ci);
    }
    let crop_w = (dims[1] as f64 * 9.0 / 16.0 / dims[0] as f64).min(1.0);
    let crop_at = |t: usize| {
        let m = tracks[t].1.median_box();
        Box { x: (m.cx() - crop_w / 2.0).clamp(0.0, 1.0 - crop_w), y: 0.0, w: crop_w, h: 1.0 }
    };
    let (mut changed_imgs, mut shot_json) = (Vec::new(), Vec::new());
    let (mut changed_s, mut total_s) = (0.0, 0.0);
    for (si, &(s, e)) in shots.iter().enumerate() {
        let window = s / 60_000;
        let n_frames = frames.iter().filter(|f| s <= f.t_ms && f.t_ms < e).count();
        let ids: Vec<usize> = (0..tracks.len()).filter(|&t| tracks[t].0 == si).collect();
        let tr: Vec<FaceTrack> = ids.iter().map(|&t| tracks[t].1.clone()).collect();
        let (shot, main) = classify_shot(&tr, n_frames, dims[0], dims[1]);
        let current = main.map(|m| ids[tr.iter().position(|t| t.samples == m.samples).unwrap()]);
        let mut time = vec![0usize; clusters.len()];
        for t in 0..tracks.len() {
            if shots[tracks[t].0].0 / 60_000 == window {
                time[cluster_of[t]] += tracks[t].1.samples.len();
            }
        }
        let main_id = (0..clusters.len()).max_by_key(|&c| time[c]).unwrap();
        let min = (MIN_TRACK_PRESENCE * n_frames as f64).max(1.0);
        let hybrid = if shot.layout == TWO_SHOT {
            current
        } else {
            ids.iter()
                .copied()
                .filter(|&t| {
                    let m = tracks[t].1.median_box();
                    cluster_of[t] == main_id && tracks[t].1.samples.len() as f64 >= min && m.h >= MIN_FACE_HEIGHT && !is_corner_overlay(&m)
                })
                .max_by_key(|&t| tracks[t].1.samples.len())
                .or(current)
        };
        let secs = (e - s) as f64 / 1000.0;
        total_s += secs;
        let differs = hybrid != current;
        if differs {
            changed_s += secs;
            let fi = (((s + e) / 2) as f64 * ANALYSIS_FPS / 1000.0) as usize;
            let mut img = images[fi.min(images.len() - 1)].to_vec();
            if let Some(t) = current { draw(&mut img, wu, hu, &crop_at(t), [0, 0, 255], 4) } // red = current
            if let Some(t) = hybrid { draw(&mut img, wu, hu, &crop_at(t), [0, 255, 0], 2) } // green = hybrid
            changed_imgs.push(img);
        }
        shot_json.push(serde_json::json!({
            "shot": si, "start_s": s as f64 / 1000.0, "end_s": e as f64 / 1000.0, "layout": shot.layout,
            "current_person": current.map(|t| cluster_of[t]), "hybrid_person": hybrid.map(|t| cluster_of[t]),
            "window_main_person": main_id, "differs": differs,
        }));
    }
    for (i, chunk) in changed_imgs.chunks(8).enumerate() {
        tile(&out_dir.join(format!("framing_diff_{i}.jpg")), chunk, wu, hu, 2);
    }
    let framing = serde_json::json!({ "shots": shot_json.len(), "shots_changed": changed_imgs.len(), "seconds_changed": changed_s, "seconds_total": total_s, "per_shot": shot_json });
    std::fs::write(out_dir.join("framing.json"), serde_json::to_string_pretty(&framing).unwrap()).unwrap();
    println!("FRAMING shots={} changed={} changed_s={:.1}/{:.1}", shot_json.len(), changed_imgs.len(), changed_s, total_s);

    // --- sheets: baseline (one crop per track), SFace (one row per identity) ---
    let mid = |ti: usize| faces[track_faces_idx[ti][track_faces_idx[ti].len() / 2]].crop.as_slice();
    let baseline_rows: Vec<Vec<&[u8]>> = order.chunks(16).map(|c| c.iter().map(|&ti| mid(ti)).collect()).collect();
    write_sheet(&out_dir.join("baseline_tracks.jpg"), &baseline_rows);
    let id_rows: Vec<Vec<&[u8]>> = clusters.iter().map(|(_, ts)| ts.iter().take(24).map(|&ti| mid(ti)).collect()).collect();
    write_sheet(&out_dir.join("sface_identities.jpg"), &id_rows);

    let first_frames = |ts: &Vec<usize>| -> Vec<usize> { ts.iter().map(|&t| faces[track_faces_idx[t][0]].frame).collect() };
    let per_track = |min: usize| track_faces_idx.iter().filter(|i| i.len() >= min).count();
    let screen = |ts: &Vec<usize>| -> usize { ts.iter().map(|&t| track_faces_idx[t].len()).sum() };
    let cluster_json: Vec<serde_json::Value> = clusters
        .iter()
        .map(|(_, ts)| {
            let mut s: Vec<usize> = ts.iter().map(|&t| tracks[t].0).collect();
            s.sort();
            s.dedup();
            serde_json::json!({ "tracks": ts.len(), "screen_s": screen(ts) as f64 / ANALYSIS_FPS, "shots": s.len(), "first_frames": first_frames(ts) })
        })
        .collect();
    let ids_ge_1s = clusters.iter().filter(|(_, ts)| screen(ts) >= 4).count();
    let report = serde_json::json!({
        "video": video, "analysis_size": [w, h], "frames": frames.len(), "faces": n_faces, "shots": shots.len(),
        "timing_s": {
            "decode": decode_s, "yunet_load": yunet_load_s, "yunet_total": yunet_s, "yunet_ms_per_frame": yunet_s * 1000.0 / frames.len() as f64,
            "sface_load": sface_load_s, "sface_all_faces": sface_s, "sface_ms_per_face": sface_s * 1000.0 / n_faces.max(1) as f64,
            "tracking": track_s, "clustering": cluster_s,
        },
        "baseline": {
            "tracks": tracks.len(), "tracks_ge_1s": per_track(4), "iou_tracker_tracks": iou_tracks,
            "consecutive_pairs_sface_says_different": switch_suspects,
        },
        "sface": {
            "identities": clusters.len(),
            "identities_ge_1s": ids_ge_1s,
            "clusters": cluster_json,
        },
    });
    std::fs::write(out_dir.join("report.json"), serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
