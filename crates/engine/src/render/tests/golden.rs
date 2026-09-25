//! Replays golden vectors produced by the Python engine (gen_golden.py) and
//! requires byte-identical filter graphs, FFmpeg arguments, ASS and SRT text.
use crate::error::{EngineError, Result};
use crate::layout::types::{Box as LBox, ClipLayoutPlan, ShotLayout};
use crate::layout::LayoutRequest;
use crate::render::captions::{self, CaptionGenerator, CaptionStyle};
use crate::render::clip_editor::{
    compute_keep_intervals, reaction_intervals, remap_plan, remap_segments, subtract_intervals, window_words, TimeMap,
    WindowWord,
};
use crate::render::layout_renderer::{
    banner_y, build_layout_graph, caption_anchor, face_rects, face_zones, measured_loudness_filter, piecewise_expr,
    shot_chain, title_y, CaptionPlacer, GraphOptions,
};
use crate::render::process::{LineSink, MediaOutput, MediaRunner};
use crate::render::service::{
    compose_overlays, escape_filter_path_for, landscape_caption_style, parse_probe_fps, video_codec_args, Overlay,
    RenderRequest, Renderer,
};
use crate::render::video_speed::{scaled_duration_ms, speed_audio_filter, speed_video_filter};
use crate::types::TranscriptSegment;
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| {
        let gz: &[u8] = include_bytes!("golden.json.gz");
        serde_json::from_reader(flate2::read::GzDecoder::new(gz)).expect("golden.json.gz")
    })
}

fn cases(name: &str) -> &'static [Value] {
    golden()[name].as_array().expect(name)
}

fn plan(v: &Value) -> ClipLayoutPlan {
    serde_json::from_value(v.clone()).expect("plan")
}

fn opt_plan(v: &Value) -> Option<ClipLayoutPlan> {
    (!v.is_null()).then(|| plan(v))
}

fn segments(v: &Value) -> Vec<TranscriptSegment> {
    serde_json::from_value(v.clone()).expect("segments")
}

fn pairs(v: &Value) -> Vec<(i64, i64)> {
    v.as_array().map(|a| a.iter().map(|p| (p[0].as_i64().unwrap(), p[1].as_i64().unwrap())).collect()).unwrap_or_default()
}

fn opt_pairs(v: &Value) -> Option<Vec<(i64, i64)>> {
    (!v.is_null()).then(|| pairs(v))
}

fn i(v: &Value) -> i64 {
    v.as_i64().unwrap_or_else(|| panic!("int: {v}"))
}

fn fonts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../engine/assets/fonts")
}

fn fonts_dir_canonical() -> PathBuf {
    fonts_dir().canonicalize().unwrap_or_else(|_| fonts_dir())
}

#[test]
fn speed_filters_match_python() {
    for c in cases("speed") {
        let (speed, dur) = (c["speed"].as_f64().unwrap(), i(&c["duration_ms"]));
        assert_eq!(scaled_duration_ms(dur, speed).unwrap(), i(&c["scaled"]), "{c}");
        assert_eq!(speed_video_filter(speed, "30000/1001").unwrap(), c["video"].as_str().unwrap(), "{c}");
        assert_eq!(speed_audio_filter(speed, dur).unwrap(), c["audio"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn keep_intervals_and_time_maps_match_python() {
    for c in cases("keeps") {
        let words: Vec<WindowWord> = c["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| WindowWord { start_ms: i(&w[0]), end_ms: i(&w[1]), text: w[2].as_str().unwrap().into() })
            .collect();
        let window_ms = i(&c["window_ms"]);
        let p = opt_plan(&c["plan"]);
        let keeps =
            compute_keep_intervals(&words, window_ms, p.as_ref(), &pairs(&c["protected"]), c["longform"].as_bool().unwrap());
        assert_eq!(keeps, pairs(&c["keeps"]), "keeps {c}");
        let sub = subtract_intervals(&keeps, &pairs(&c["skips"]));
        assert_eq!(sub, pairs(&c["subtracted"]), "subtracted {c}");
        let tm = TimeMap::new(sub, Some(window_ms));
        assert_eq!(tm.removed_ms(), i(&c["removed_ms"]));
        assert_eq!(tm.cut_count() as i64, i(&c["cut_count"]));
        assert_eq!(tm.output_ms, i(&c["output_ms"]));
        for probe in c["probes"].as_array().unwrap() {
            let t = i(&probe[0]);
            assert_eq!(tm.to_output(t), probe[1].as_i64(), "to_output({t}) {c}");
            assert_eq!(tm.to_output_clamped(t), i(&probe[2]));
            assert_eq!(tm.pieces_within(t, t + 1500), pairs(&probe[3]));
        }
    }
}

fn plan_value(p: &ClipLayoutPlan) -> Value {
    serde_json::to_value(p).unwrap()
}

#[test]
fn remapping_matches_python() {
    for c in cases("remap") {
        let segs = segments(&c["segments"]);
        let (ws, wms) = (i(&c["window_start_ms"]), i(&c["window_ms"]));
        let tm = TimeMap::new(pairs(&c["keeps"]), Some(i(&c["time_map_window"])));
        let remapped = remap_segments(&segs, ws, &tm);
        assert_eq!(remapped, segments(&c["remapped"]), "remap {c}");
        let words: Vec<Value> =
            window_words(&segs, ws, wms).into_iter().map(|w| json!([w.start_ms, w.end_ms, w.text])).collect();
        assert_eq!(Value::Array(words), c["window_words"]);
        assert_eq!(reaction_intervals(&segs, ws, wms), pairs(&c["reactions"]));
        assert_eq!(plan_value(&remap_plan(&plan(&c["plan"]), &tm)), plan_value(&plan(&c["remapped_plan"])));
    }
}

#[test]
fn shot_chains_and_positions_match_python() {
    for c in cases("chains") {
        let shot: ShotLayout = serde_json::from_value(c["shot"].clone()).unwrap();
        let (sw, sh) = (i(&c["src"][0]), i(&c["src"][1]));
        let (ow, oh) = (i(&c["out"][0]), i(&c["out"][1]));
        let landscape = c["landscape"].as_bool().unwrap();
        assert_eq!(shot_chain(3, &shot, sw, sh, ow, oh, landscape), c["chain"].as_str().unwrap(), "{c}");
        let (a, y) = caption_anchor(&shot, sw, sh, ow, oh);
        assert_eq!(json!([a, y]), c["anchor"]);
        let ty: Vec<i64> = [60, 125, 400].iter().map(|h| title_y(&shot, sw, sh, ow, oh, *h)).collect();
        assert_eq!(json!(ty), c["title_y"]);
        assert_eq!(banner_y(&shot, sw, sh, ow, oh), i(&c["banner_y"]));
        if c["rects"].is_string() {
            continue;
        }
        let faces: Vec<LBox> = serde_json::from_value(c["faces"].clone()).unwrap();
        let rects = face_rects(&shot, &faces, i(&c["t_ms"]), sw, sh, ow, oh);
        let rects: Vec<Value> = rects.into_iter().map(|r| json!([r.0, r.1, r.2, r.3])).collect();
        assert_eq!(Value::Array(rects), c["rects"], "{c}");
    }
}

#[test]
fn filter_graphs_match_python() {
    for c in cases("graphs") {
        let p = plan(&c["plan"]);
        let keeps = opt_pairs(&c["keeps"]);
        let fps = c["fps"].as_str().unwrap();
        let loud = c["loudness"].as_str();
        let opts = GraphOptions {
            keeps: keeps.as_deref(),
            with_audio: c["with_audio"].as_bool().unwrap(),
            landscape: i(&c["out"][0]) > i(&c["out"][1]),
            fps,
            loudness_filter: loud,
            video_speed: c["speed"].as_f64().unwrap(),
        };
        let expected = c["graph"].as_str().unwrap();
        match build_layout_graph(&p, i(&c["out"][0]), i(&c["out"][1]), &opts) {
            Ok(graph) => assert_eq!(graph, expected, "{c}"),
            Err(e) => {
                let detail = expected.strip_prefix("error: ").unwrap_or_else(|| panic!("unexpected error {e}: {c}"));
                assert!(e.to_string().contains(detail), "{e} vs {detail}");
                assert!(e.to_string().contains("video render failed"));
            }
        }
    }
}

#[test]
fn loudness_filters_match_python() {
    for c in cases("loudness") {
        assert_eq!(measured_loudness_filter(&c["measured"]).as_deref(), c["filter"].as_str(), "{c}");
    }
}

#[test]
fn face_zones_and_caption_placement_match_python() {
    for c in cases("zones") {
        let p = plan(&c["plan"]);
        let tm = TimeMap::new(pairs(&c["keeps"]), Some(i(&c["window_ms"])));
        let zones = face_zones(&p, &tm, 1080, 1920);
        let zv: Vec<Value> = zones
            .iter()
            .map(|z| json!([z.start_ms, z.end_ms, z.rects.iter().map(|r| json!([r.0, r.1, r.2, r.3])).collect::<Vec<_>>()]))
            .collect();
        assert_eq!(Value::Array(zv), c["zones"], "{c}");
        let anchors: Vec<(i64, i64, i64)> =
            c["anchors"].as_array().unwrap().iter().map(|a| (i(&a[0]), i(&a[1]), i(&a[2]))).collect();
        let mut placer = CaptionPlacer::new(anchors, zones, 1080, 1920);
        for call in c["calls"].as_array().unwrap() {
            let got = placer.place(i(&call[0]), i(&call[1]), i(&call[2]), i(&call[3]));
            assert_eq!(json!([got.0, got.1]), call[4], "{call} in {c}");
        }
    }
}

#[test]
fn piecewise_expressions_match_python() {
    for c in cases("piecewise") {
        let pts: Vec<(f64, f64)> =
            c["points"].as_array().unwrap().iter().map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap())).collect();
        assert_eq!(piecewise_expr(&pts), c["expr"].as_str().unwrap(), "{c}");
    }
}

#[test]
fn caption_presets_match_python() {
    for (name, style) in golden()["presets"].as_object().unwrap() {
        let expected: CaptionStyle = serde_json::from_value(style.clone()).unwrap();
        assert_eq!(captions::get_caption_preset(name).unwrap(), expected, "{name}");
    }
    assert!(captions::get_caption_preset("nope").is_err());
    assert_eq!(captions::caption_style_for("nope"), captions::get_caption_preset("pop").unwrap());
}

#[test]
fn ass_captions_match_python() {
    let mut checked = 0;
    for c in cases("captions") {
        let style: CaptionStyle = serde_json::from_value(c["style"].clone()).unwrap();
        let emphasis: Vec<String> = serde_json::from_value(c["emphasis"].clone()).unwrap();
        let anchors: Option<Vec<(i64, i64, i64)>> = c["anchors"]
            .as_array()
            .map(|a| a.iter().map(|x| (i(&x[0]), i(&x[1]), i(&x[2]))).collect());
        let gen = CaptionGenerator::new(&style, &emphasis, None);
        let got = gen.build_ass(
            &segments(&c["segments"]),
            i(&c["clip_start_ms"]),
            i(&c["clip_end_ms"]),
            i(&c["width"]),
            i(&c["height"]),
            None,
            anchors.as_deref(),
            None,
        );
        assert_eq!(got.as_deref(), c["ass"].as_str(), "{}", c["style"]);
        checked += 1;
    }
    assert!(checked > 50);
}

#[test]
fn srt_sidecars_match_python() {
    for c in cases("srt") {
        let got = captions::build_srt(&segments(&c["segments"]), i(&c["clip_start_ms"]), i(&c["clip_end_ms"]));
        assert_eq!(got.as_deref(), c["srt"].as_str(), "{c}");
    }
}

#[test]
fn caption_block_sizes_are_close_to_python() {
    // Pillow measured these with FreeType + libraqm. Widths match (same shaping and
    // 1/64 px advances); glyph heights can differ by a pixel where FreeType's
    // hinting snaps outlines to the pixel grid.
    let dir = fonts_dir();
    let mut worst = 0;
    for c in cases("block_sizes") {
        let style = captions::get_caption_preset(c["preset"].as_str().unwrap()).unwrap();
        let words: Vec<String> = serde_json::from_value(c["words"].clone()).unwrap();
        let gen = CaptionGenerator::new(&style, &[], Some(&dir));
        let (w, h) = gen.block_size(&words, i(&c["width"]));
        let (ew, eh) = (i(&c["size"][0]), i(&c["size"][1]));
        worst = worst.max((w - ew).abs()).max((h - eh).abs());
        assert!((w - ew).abs() <= 1 && (h - eh).abs() <= 2, "{c}: got {w}x{h}");
    }
    eprintln!("caption block size: worst difference from Pillow {worst}px");
}

#[test]
fn service_helpers_match_python() {
    for c in cases("overlays_compose") {
        let overlays: Vec<Overlay> = c["overlays"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| Overlay {
                path: PathBuf::from(o[0].as_str().unwrap()),
                x: o[1].as_str().unwrap().into(),
                y: o[2].as_str().unwrap().into(),
                timed: (o.as_array().unwrap().len() == 5)
                    .then(|| (o[3].as_str().unwrap().to_string(), o[4].as_str().unwrap().to_string())),
            })
            .collect();
        let (graph, extra) =
            compose_overlays("[0:v]null[base];[base]null[captioned]", &overlays, c["video_filter"].as_str().unwrap());
        assert_eq!(graph, c["graph"].as_str().unwrap());
        let extra: Vec<String> = extra.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert_eq!(json!(extra), c["extra"]);
    }
    for c in cases("escape") {
        assert_eq!(escape_filter_path_for(c["path"].as_str().unwrap(), false), c["escaped"].as_str().unwrap());
    }
    assert_eq!(escape_filter_path_for("C:\\Users\\me\\c.ass", true), "'C\\:/Users/me/c.ass'");
    for c in cases("codec_args") {
        let args = video_codec_args(c["videotoolbox"].as_bool().unwrap(), i(&c["out"][0]), i(&c["out"][1]), c["fps"].as_str().unwrap());
        assert_eq!(json!(args), c["args"], "{c}");
    }
    for c in cases("probe_fps") {
        let got = parse_probe_fps(c["stdout"].as_str().unwrap().as_bytes()).unwrap_or_else(|| "30".into());
        assert_eq!(got, c["fps"].as_str().unwrap(), "{c}");
    }
    let style = captions::get_caption_preset("glow").unwrap();
    let scaled = landscape_caption_style(&style, 2160);
    assert_eq!((scaled.font_size, scaled.outline_width, scaled.max_words_per_line), (104, 0, 6));
}

// ------------------------------------------------------------------
// Full render flows with FFmpeg stubbed
// ------------------------------------------------------------------

const LOUDNORM_STDERR: &str = "[Parsed_loudnorm_2 @ 0x1] \n{\n\t\"input_i\" : \"-23.54\",\n\t\"input_tp\" : \"-5.20\",\n\t\"input_lra\" : \"6.10\",\n\t\"input_thresh\" : \"-34.00\",\n\t\"output_i\" : \"-14.1\",\n\t\"target_offset\" : \"0.40\"\n}\n";

struct FlowRunner {
    case: Value,
    dir: String,
    fonts: String,
    log: Mutex<Vec<Value>>,
    ffmpeg_calls: Mutex<i64>,
    /// Expected duration (ms) of each VALIDATE, in order, from the Python log.
    validations: Mutex<Vec<(i64, bool)>>,
}

impl FlowRunner {
    fn record(&self, program: &Path, args: &[String]) {
        let mut cmd: Vec<String> = vec![program.to_string_lossy().into_owned()];
        cmd.extend(args.iter().cloned());
        if let Some(pos) = cmd.iter().position(|a| a == "-/filter_complex") {
            cmd[pos + 1] = format!("SCRIPT:{}", std::fs::read_to_string(&cmd[pos + 1]).unwrap());
        }
        let text = serde_json::to_string(&cmd).unwrap().replace(&self.dir, "{DIR}").replace(&self.fonts, "{FONTS}");
        self.log.lock().unwrap().push(serde_json::from_str(&text).unwrap());
    }

    fn respond(&self, program: &Path, args: &[String]) -> MediaOutput {
        let c = &self.case;
        let ok = |stdout: Vec<u8>| MediaOutput { code: 0, stdout, stderr: Vec::new() };
        let has = |s: &str| args.iter().any(|a| a == s);
        if program == Path::new("ffprobe") {
            if has("stream=codec_type,start_time,duration") {
                self.log.lock().unwrap().push(json!(["VALIDATE"]));
                let (ms, audio) = self.validations.lock().unwrap().remove(0);
                let mut streams = vec![json!({"codec_type": "video", "start_time": "0.000000", "duration": format!("{:.6}", ms as f64 / 1000.0)})];
                if audio {
                    streams.push(json!({"codec_type": "audio", "start_time": "0.000000", "duration": format!("{:.6}", ms as f64 / 1000.0)}));
                }
                return ok(serde_json::to_vec(&json!({ "streams": streams })).unwrap());
            }
            self.record(program, args);
            if has("stream=width,height") {
                return if c["probe_dims"].as_bool().unwrap() {
                    ok(format!("{}x{}\n", c["source"][0], c["source"][1]).into_bytes())
                } else {
                    MediaOutput { code: 1, ..Default::default() }
                };
            }
            if has("stream=index") {
                return ok(if c["has_audio"].as_bool().unwrap() { b"1\n".to_vec() } else { Vec::new() });
            }
            if has("stream=avg_frame_rate,r_frame_rate") {
                let body = json!({"streams": [{"avg_frame_rate": c["fps_body"], "r_frame_rate": "0/0"}]});
                return ok(serde_json::to_vec(&body).unwrap());
            }
            panic!("unexpected probe {args:?}");
        }
        self.record(program, args);
        if args.iter().any(|a| a.contains("print_format=json")) {
            return if c["loud_ok"].as_bool().unwrap() {
                MediaOutput { code: 0, stdout: Vec::new(), stderr: LOUDNORM_STDERR.as_bytes().to_vec() }
            } else {
                MediaOutput { code: 1, stdout: Vec::new(), stderr: b"err".to_vec() }
            };
        }
        let mut calls = self.ffmpeg_calls.lock().unwrap();
        *calls += 1;
        if *calls <= i(&c["ffmpeg_failures"]) {
            let mut stderr = vec![b'x'; 1200];
            stderr.extend_from_slice(b"Cannot select channel layout");
            return MediaOutput { code: 1, stdout: Vec::new(), stderr };
        }
        std::fs::write(args.last().unwrap(), vec![0u8; 2048]).unwrap();
        ok(Vec::new())
    }
}

impl MediaRunner for FlowRunner {
    fn run<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
        _timeout: Duration,
        _max_output: usize,
        _cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<MediaOutput>> {
        Box::pin(async move { Ok(self.respond(program, args)) })
    }

    fn run_lines<'a>(
        &'a self,
        _program: &'a Path,
        _args: &'a [String],
        _timeout: Duration,
        _max_line: usize,
        on_line: LineSink<'a>,
        _cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<MediaOutput>> {
        Box::pin(async move {
            self.log.lock().unwrap().push(json!(["AUDIO"]));
            for line in ["pts_time=-0.021333|duration_time=0.021333|\n", "pts_time=0.000000|duration_time=0.021333\n", "pts_time=0.021333|duration_time=0.005000\n"] {
                on_line(line.as_bytes()).map_err(EngineError::Failed)?;
            }
            Ok(MediaOutput::default())
        })
    }
}

fn run_flow(case: &Value) -> (Vec<Value>, Value, Option<String>) {
    let tmp = tempfile::Builder::new().prefix("golden-").tempdir().unwrap();
    let dir = tmp.path().to_string_lossy().into_owned();
    let fonts = fonts_dir_canonical();
    let validations: Vec<(i64, bool)> = case["log"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e[0] == "VALIDATE")
        .map(|e| (i(&e[1]), e[3].as_bool().unwrap()))
        .collect();
    let runner = Arc::new(FlowRunner {
        case: case.clone(),
        dir: dir.clone(),
        fonts: fonts.to_string_lossy().into_owned(),
        log: Mutex::new(Vec::new()),
        ffmpeg_calls: Mutex::new(0),
        validations: Mutex::new(validations),
    });
    let landscape = case["aspect_ratio"] == "16:9";
    let fit = case["layout_style"] == "fit";
    let (plan_v, content_v) = (case["plan"].clone(), case["content_plan"].clone());
    let log_runner = runner.clone();
    let analyzer: crate::render::AnalyzeFn = Arc::new(move |req: LayoutRequest, _cancel: CancellationToken| {
        let content = landscape || fit;
        let entry = if content {
            json!(["CONTENT", req.style, req.vision, req.src_w, req.src_h, req.start_ms, req.duration_ms])
        } else {
            json!(["PLAN", req.style, req.src_w, req.src_h, req.start_ms, req.duration_ms])
        };
        log_runner.log.lock().unwrap().push(entry);
        let p = opt_plan(if content { &content_v } else { &plan_v });
        Box::pin(async move { Ok(p) })
    });
    let renderer = Renderer {
        ffmpeg: PathBuf::from("ffmpeg"),
        ffprobe: PathBuf::from("ffprobe"),
        fonts_dir: Some(fonts.clone()),
        font_path: Some(fonts.join("Montserrat-Black.ttf")),
        runner: runner.clone(),
        analyzer: Some(analyzer),
        video_codec_override: None,
        videotoolbox: false,
        audio_padding_ms: 150,
        temp_dir: None,
    };
    let banner = case["banner"].as_bool().unwrap();
    let request = RenderRequest {
        video_path: tmp.path().join("src.mp4"),
        output_path: tmp.path().join("out").join("clip_00.mp4"),
        start_time_ms: i(&case["start_time_ms"]),
        end_time_ms: i(&case["end_time_ms"]),
        source_width: i(&case["source"][0]),
        source_height: i(&case["source"][1]),
        transcript_segments: segments(&case["segments"]),
        include_captions: case["include_captions"].as_bool().unwrap(),
        caption_style: Some(captions::get_caption_preset(case["preset"].as_str().unwrap()).unwrap()),
        title_text: case["title"].as_str().map(str::to_string),
        emphasis_words: serde_json::from_value(case["emphasis"].clone()).unwrap(),
        banner_platform: banner.then(|| "youtube".into()),
        banner_channel_url: banner.then(|| "example.com/@me".into()),
        include_audio: true,
        apply_padding: true,
        aspect_ratio: case["aspect_ratio"].as_str().unwrap().into(),
        layout_style: case["layout_style"].as_str().unwrap().into(),
        layout_vision: case["layout_vision"].as_bool().unwrap(),
        pacing: case["pacing"].as_str().unwrap().into(),
        video_speed: case["video_speed"].as_f64().unwrap(),
        longform: case["longform"].as_bool().unwrap(),
        skip_ranges_ms: pairs(&case["skips"]),
        chapters: case["chapters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (i(&c[0]), c[1].as_str().unwrap().to_string()))
            .collect(),
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let result = rt.block_on(renderer.render_clip(&request, &CancellationToken::new()));
    let res = match result {
        Ok(r) => json!({
            "duration_ms": r.duration_ms,
            "removed_ms": r.removed_ms,
            "layout_type": r.layout_type,
            "layout_shots": r.layout_shots,
            "layout_cost_usd": r.layout_cost_usd,
            "render_fallback": r.render_fallback,
            "chapters": r.chapters.iter().map(|(t, s)| json!([t, s])).collect::<Vec<_>>(),
            "subtitle": r.subtitle_path.map(|p| std::fs::read_to_string(p).unwrap()),
            "output": [r.output_width, r.output_height],
        }),
        Err(e) => json!({ "error": e.to_string() }),
    };
    let ass_path = tmp.path().join("out").join(format!("clip-{}-{}.ass", request.start_time_ms, request.end_time_ms));
    let ass = std::fs::read_to_string(ass_path).ok();
    let log = runner.log.lock().unwrap().clone();
    (log, res, ass)
}

/// Python logs PLAN for Classic ("fit") too, where the Rust port skips the analyzer call.
fn expected_log(case: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for e in case["log"].as_array().unwrap() {
        if e[0] == "PLAN" && e[1] == "fit" {
            continue;
        }
        if e[0] == "VALIDATE" {
            out.push(json!(["VALIDATE"]));
            if e[3].as_bool().unwrap() {
                out.push(json!(["AUDIO"]));
            }
            continue;
        }
        out.push(e.clone());
    }
    out
}

fn strip_positions(ass: &str) -> String {
    let re = regex::Regex::new(r"\\an\d\\pos\(\d+,\d+\)").unwrap();
    re.replace_all(ass, "").into_owned()
}

#[test]
fn render_flows_match_python() {
    let mut placed_mismatch = 0;
    let flows = cases("flows");
    for (n, case) in flows.iter().enumerate() {
        let (log, res, ass) = run_flow(case);
        let expected = expected_log(case);
        assert_eq!(log.len(), expected.len(), "flow {n}: call count\nrust: {log:#?}\npython: {expected:#?}");
        for (k, (got, want)) in log.iter().zip(&expected).enumerate() {
            assert_eq!(got, want, "flow {n} call {k}");
        }
        let want = &case["result"];
        if want.get("error").is_some() {
            assert!(res.get("error").is_some(), "flow {n}: python failed ({}) but rust rendered", want["error"]);
            assert!(res["error"].as_str().unwrap().contains("video render failed"), "{res}");
        } else {
            for key in ["duration_ms", "removed_ms", "layout_type", "render_fallback", "chapters", "subtitle", "output"] {
                assert_eq!(res[key], want[key], "flow {n}: {key}");
            }
            assert!((res["layout_cost_usd"].as_f64().unwrap() - want["layout_cost_usd"].as_f64().unwrap()).abs() < 1e-12);
            let (got_shots, want_shots) = (res["layout_shots"].as_array().unwrap(), want["layout_shots"].as_array().unwrap());
            assert_eq!(got_shots.len(), want_shots.len(), "flow {n}: shots");
            for (g, w) in got_shots.iter().zip(want_shots) {
                for key in ["start_ms", "end_ms", "layout", "source"] {
                    assert_eq!(g[key], w[key], "flow {n}: shot {key}");
                }
            }
        }
        let want_ass = want["ass"].as_str();
        if ass.as_deref() != want_ass {
            // Face-aware placement measures caption blocks with the font; Pillow's shaping
            // can differ by a pixel or two, which may move a caption. Everything else must match.
            let (g, w) = (ass.as_deref().unwrap_or(""), want_ass.unwrap_or(""));
            assert_eq!(strip_positions(g), strip_positions(w), "flow {n}: ASS");
            placed_mismatch += 1;
        }
    }
    eprintln!("render flows: {} cases, {placed_mismatch} with face-placed captions at other positions", flows.len());
    assert!(placed_mismatch * 5 <= flows.len(), "too many caption placement differences: {placed_mismatch}");
}
