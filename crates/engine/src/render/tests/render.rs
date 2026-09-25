//! Ports of engine/tests/test_render_fallback.py, test_render_timing.py,
//! test_video_speed.py (non-media parts) and test_clip_editor.py.
use super::support::*;
use crate::error::{EngineError, Result};
use crate::layout::types::{ClipLayoutPlan, ShotLayout, SCREEN, SCREEN_CAM, TALKING_HEAD};
use crate::layout::LayoutRequest;
use crate::render::clip_editor::{
    compute_keep_intervals, is_filler, reaction_intervals, remap_segments, TimeMap, WindowWord, KEEP_PAUSE_MS,
    MIN_PIECE_MS, TAIL_MS,
};
use crate::render::layout_renderer::{build_layout_graph, GraphOptions};
use crate::render::service::{
    check_stream_timing, output_chapters, render_error, AudioPacketChecker, EditJob, FfmpegJob, RenderRequest,
    RenderResult, Renderer, DISK_FULL,
};
use crate::render::{scaled_duration_ms, validate_video_speed, AnalyzeFn};
use crate::types::TranscriptSegment;
use futures::future::BoxFuture;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------- fallback ladder

/// Speech with a 3 s pause in the middle, so tight pacing makes a cut.
fn transcript() -> Vec<TranscriptSegment> {
    let first = (0..8).map(|i| word(&format!("w{i}"), 300 + i * 400, 650 + i * 400)).collect();
    let second = (0..8).map(|i| word(&format!("v{i}"), 6500 + i * 400, 6850 + i * 400)).collect();
    vec![segment(300, 3450, "First part.", first), segment(6500, 9650, "Second part.", second)]
}

fn smart_plan(window_ms: i64) -> ClipLayoutPlan {
    ClipLayoutPlan {
        vision_cost_usd: 0.002,
        ..plan(
            vec![ShotLayout {
                source: "vision".into(),
                cam_box: Some(b(0.62, 0.55, 0.36, 0.43)),
                screen_box: Some(b(0.0, 0.0, 1.0, 1.0)),
                ..shot(0, window_ms, SCREEN_CAM)
            }],
            1920,
            1080,
        )
    }
}

fn analyzer(calls: Arc<Mutex<Vec<(String, bool)>>>, f: fn(i64) -> Option<ClipLayoutPlan>) -> AnalyzeFn {
    Arc::new(move |req: LayoutRequest, _c: CancellationToken| {
        calls.lock().unwrap().push((req.style.clone(), req.vision));
        let p = f(req.duration_ms);
        Box::pin(async move { Ok(p) })
    })
}

fn service() -> Renderer {
    let mut r = renderer(FakeRunner::new(|_, args| {
        if args.iter().any(|a| a == "stream=width,height") {
            ok(b"1920x1080\n")
        } else {
            ok(b"1\n")
        }
    }));
    r.analyzer = Some(analyzer(Arc::new(Mutex::new(Vec::new())), |w| Some(smart_plan(w))));
    r
}

fn request_for(dir: &Path) -> RenderRequest {
    RenderRequest {
        video_path: PathBuf::from("src.mp4"),
        output_path: dir.join("clip.mp4"),
        start_time_ms: 0,
        end_time_ms: 10_000,
        source_width: 1920,
        source_height: 1080,
        transcript_segments: transcript(),
        ..Default::default()
    }
}

type Calls = Arc<Mutex<Vec<(bool, bool, i64)>>>;

/// Renders with an edit stub that fails the first `failures` attempts, then writes a file.
fn render_stubbed(r: &Renderer, request: &RenderRequest, failures: usize, error: fn() -> EngineError) -> (Result<RenderResult>, Calls) {
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let result = block_on(r.render_clip_with(request, &CancellationToken::new(), move |job: EditJob| {
        let mut c = seen.lock().unwrap();
        c.push((job.plan.is_letterbox_only(), job.time_map.has_cuts(), job.time_map.output_ms));
        let outcome = if c.len() <= failures {
            Err(error())
        } else {
            std::fs::write(&job.request.output_path, [0u8; 1024]).map_err(EngineError::from)
        };
        Box::pin(async move { outcome }) as BoxFuture<'_, Result<()>>
    }));
    (result, calls)
}

fn ffmpeg_failed() -> EngineError {
    render_error("FFmpeg failed: Cannot select channel layout")
}

fn flags(calls: &Calls) -> Vec<(bool, bool)> {
    calls.lock().unwrap().iter().map(|c| (c.0, c.1)).collect()
}

#[test]
fn every_fallback_keeps_export_speed() {
    for failures in 0..3 {
        let tmp = tempfile::tempdir().unwrap();
        let request = RenderRequest { video_speed: 1.5, ..request_for(tmp.path()) };
        let (result, calls) = render_stubbed(&service(), &request, failures, ffmpeg_failed);
        let result = result.unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), failures + 1);
        assert_eq!(result.duration_ms, (calls[calls.len() - 1].2 as f64 / 1.5).round() as i64);
    }
}

#[test]
fn tail_only_cut_has_natural_timing_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let request = RenderRequest {
        transcript_segments: vec![segment(300, 1050, "Short sentence.", vec![word("Short", 300, 650), word("sentence.", 700, 1050)])],
        ..request_for(tmp.path())
    };
    let (result, calls) = render_stubbed(&service(), &request, 2, ffmpeg_failed);
    let result = result.unwrap();
    assert_eq!(flags(&calls), vec![(false, true), (true, true), (true, false)]);
    assert_eq!(result.render_fallback.as_deref(), Some("letterbox_natural"));
    assert_eq!(result.duration_ms, 10_150);
    assert_eq!(result.removed_ms, 0);
}

#[test]
fn smart_render_as_planned() {
    let tmp = tempfile::tempdir().unwrap();
    let (result, calls) = render_stubbed(&service(), &request_for(tmp.path()), 0, ffmpeg_failed);
    let result = result.unwrap();
    assert_eq!(flags(&calls), vec![(false, true)]);
    assert_eq!(result.render_fallback, None);
    assert_eq!(result.layout_type, SCREEN_CAM);
    assert!(result.removed_ms > 0);
    assert_eq!(result.layout_shots.len(), 1);
    let entry = result.clip_layout_entry(0, &request_for(tmp.path()));
    assert_eq!(entry["framing_status"], "smart");
}

#[test]
fn letterbox_keeps_pacing_cuts() {
    let tmp = tempfile::tempdir().unwrap();
    let (result, calls) = render_stubbed(&service(), &request_for(tmp.path()), 1, ffmpeg_failed);
    let result = result.unwrap();
    assert_eq!(flags(&calls), vec![(false, true), (true, true)]);
    assert_eq!(result.render_fallback.as_deref(), Some("letterbox"));
    assert_eq!(result.layout_type, "fit");
    assert!(result.removed_ms > 0);
    // The vision call was made (and paid for) even though a fallback rendered.
    assert!((result.layout_cost_usd - 0.002).abs() < 1e-12);
    assert_eq!(result.clip_layout_entry(1, &request_for(tmp.path()))["framing_status"], "fallback");
}

#[test]
fn natural_timing_is_the_last_resort() {
    let tmp = tempfile::tempdir().unwrap();
    let (result, calls) = render_stubbed(&service(), &request_for(tmp.path()), 2, ffmpeg_failed);
    let result = result.unwrap();
    assert_eq!(flags(&calls), vec![(false, true), (true, true), (true, false)]);
    assert_eq!(result.render_fallback.as_deref(), Some("letterbox_natural"));
    assert_eq!(result.removed_ms, 0);
}

#[test]
fn error_raised_when_every_step_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let (result, calls) = render_stubbed(&service(), &request_for(tmp.path()), 3, ffmpeg_failed);
    let err = result.unwrap_err().to_string();
    assert!(err.contains("video render failed"), "{err}");
    assert_eq!(calls.lock().unwrap().len(), 3);
}

#[test]
fn non_rendering_error_moves_down_the_ladder_and_last_step_raises() {
    let tmp = tempfile::tempdir().unwrap();
    let (result, calls) = render_stubbed(&service(), &request_for(tmp.path()), 1, || EngineError::msg("bad overlay geometry"));
    assert_eq!(result.unwrap().render_fallback.as_deref(), Some("letterbox"));
    assert_eq!(flags(&calls).iter().map(|c| c.0).collect::<Vec<_>>(), vec![false, true]);

    let (result, _) = render_stubbed(&service(), &request_for(tmp.path()), 9, || EngineError::msg("always broken"));
    let err = result.unwrap_err().to_string();
    assert!(err.contains("video render failed") && err.contains("always broken"), "{err}");
}

#[test]
fn disk_full_is_reported_as_such() {
    let tmp = tempfile::tempdir().unwrap();
    let (result, _) =
        render_stubbed(&service(), &request_for(tmp.path()), 9, || render_error("FFmpeg failed: No space left on device"));
    assert_eq!(result.unwrap_err().to_string(), DISK_FULL);
    let io = crate::render::service::io_render_error(std::io::Error::from_raw_os_error(28));
    if cfg!(unix) {
        assert_eq!(io.to_string(), DISK_FULL);
    }
}

#[test]
fn cancellation_is_not_a_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let (result, calls) = render_stubbed(&service(), &request_for(tmp.path()), 1, || EngineError::Cancelled);
    assert!(matches!(result, Err(EngineError::Cancelled)));
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn pacing_still_cuts_without_captions() {
    let tmp = tempfile::tempdir().unwrap();
    let request = RenderRequest { include_captions: false, ..request_for(tmp.path()) };
    let (result, _) = render_stubbed(&service(), &request, 0, ffmpeg_failed);
    assert!(result.unwrap().removed_ms > 0);
}

#[test]
fn no_caption_file_when_captions_off() {
    let tmp = tempfile::tempdir().unwrap();
    let request = RenderRequest { include_captions: false, ..request_for(tmp.path()) };
    let tm = TimeMap::new(vec![(0, 10_000)], None);
    let path = service().generate_captions(&request, 1080, 1920, 0, &tm, &smart_plan(10_000), false, None).unwrap();
    assert!(path.is_none());
}

/// Five words with 1.5 s pauses: dead air on a talking head (700 ms limit),
/// but under the 2 s limit used when shot content is unknown.
fn paused_transcript() -> Vec<TranscriptSegment> {
    let words: Vec<_> = (0..5).map(|i| word(&format!("w{i}"), 300 + i * 1850, 650 + i * 1850)).collect();
    let end = words[4].end_time_ms;
    vec![segment(300, end, "Five slow words.", words)]
}

fn plain_service(calls: Arc<Mutex<Vec<(String, bool)>>>, f: fn(i64) -> Option<ClipLayoutPlan>) -> Renderer {
    let mut r = service();
    r.analyzer = Some(analyzer(calls, f));
    r
}

#[test]
fn talking_head_gets_the_talking_head_limit_without_smart_framing() {
    for (style, aspect) in [("fit", "9:16"), ("auto", "16:9")] {
        let tmp = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let r = plain_service(calls.clone(), |w| {
            Some(plan(vec![ShotLayout { people: vec![b(0.45, 0.2, 0.1, 0.25)], ..shot(0, w, TALKING_HEAD) }], 1920, 1080))
        });
        let request = RenderRequest {
            transcript_segments: paused_transcript(),
            layout_style: style.into(),
            aspect_ratio: aspect.into(),
            ..request_for(tmp.path())
        };
        let (result, _) = render_stubbed(&r, &request, 0, ffmpeg_failed);
        let result = result.unwrap();
        assert_eq!(*calls.lock().unwrap(), vec![("auto".to_string(), false)]);
        // Four 1.5 s pauses cut down to a 260 ms breath each.
        assert!(result.removed_ms >= 4 * 1000, "{}", result.removed_ms);
        assert_eq!(result.layout_type, "fit");
        assert_eq!(result.layout_cost_usd, 0.0);
        assert_eq!(result.clip_layout_entry(0, &request)["framing_status"], "classic");
    }
}

#[test]
fn unknown_content_keeps_the_gentle_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let r = plain_service(Arc::new(Mutex::new(Vec::new())), |_| None);
    let request =
        RenderRequest { transcript_segments: paused_transcript(), layout_style: "fit".into(), ..request_for(tmp.path()) };
    let (result, _) = render_stubbed(&r, &request, 0, ffmpeg_failed);
    // The 1.5 s pauses stay; only the long tail is trimmed.
    let expected = paused_transcript()[0].end_time_ms + TAIL_MS;
    assert!((result.unwrap().duration_ms - expected).abs() <= 40);
}

#[test]
fn no_analysis_when_pacing_is_natural() {
    let tmp = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let r = plain_service(calls.clone(), |_| None);
    let request = RenderRequest {
        transcript_segments: paused_transcript(),
        layout_style: "fit".into(),
        pacing: "natural".into(),
        ..request_for(tmp.path())
    };
    render_stubbed(&r, &request, 0, ffmpeg_failed).0.unwrap();
    assert!(calls.lock().unwrap().is_empty());
}

// ---------------------------------------------------------------- export timing guard

fn stream(kind: &str, start: &str, duration: &str) -> serde_json::Value {
    json!({"codec_type": kind, "start_time": start, "duration": duration})
}

#[test]
fn rejects_unusable_exports() {
    let v = |d: &str| stream("video", "0", d);
    let a = |s: &str, d: &str| stream("audio", s, d);
    let cases = vec![
        vec![],
        vec![v("10")],
        vec![a("0", "10")],
        vec![v("10"), a("0.3", "10")],
        vec![stream("video", "0.066", "10"), a("0", "10")],
        vec![v("9"), a("0", "10")],
        vec![v("10"), a("0", "9")],
        vec![v("10"), a("0", "nan")],
        vec![v("10"), a("inf", "10")],
        vec![v("10"), json!({"codec_type": "audio"})],
        vec![v("10"), a("0", "10"), a("0", "10")],
    ];
    for streams in cases {
        let body = serde_json::to_vec(&json!({ "streams": streams })).unwrap();
        assert!(check_stream_timing(&body, 10_000, "30", true).is_err(), "{streams:?}");
        // Through the renderer, the failure carries the fixed message.
        let r = renderer(FakeRunner::new(move |_, _| ok(&body)));
        let err = block_on(r.validate_output_timing(Path::new("output.mp4"), 10_000, "30", true, &CancellationToken::new()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("timing validation failed") && err.contains("video render failed"), "{err}");
    }
}

#[test]
fn accepts_codec_and_frame_rounding() {
    for (with_audio, fps, vd, ad) in [
        (true, "30", "10", "10.020"),
        (false, "30", "10", "10"),
        (true, "30000/1001", "10.010", "10"),
        // Audio one frame short at the tail (seen on real 29.97 fps exports).
        (true, "30000/1001", "10", "9.967"),
    ] {
        let mut streams = vec![stream("video", "0", vd)];
        if with_audio {
            streams.push(stream("audio", "0", ad));
        }
        let body = serde_json::to_vec(&json!({ "streams": streams })).unwrap();
        assert!(check_stream_timing(&body, 10_000, fps, with_audio).is_ok(), "{fps} {vd} {ad}");
    }
}

#[test]
fn rejects_audio_packet_timing_even_when_stream_endpoints_match() {
    let cases: Vec<&[u8]> = vec![
        b"",
        b"pts_time=nan|duration_time=0.021333\n",
        b"pts_time=0|duration_time=nan\n",
        b"pts_time=0|duration_time=0\n",
        b"pts_time=0|duration_time=0.101333\n",
        b"pts_time=0|duration_time=0.021333\npts_time=0.101333|duration_time=0.021333\n",
        b"pts_time=0|duration_time=0.021333\npts_time=0|duration_time=0.021333\n",
        b"pts_time=0|duration_time=0.021333\npts_time=-0.021333|duration_time=0.021333\n",
        b"pts_time=N/A|duration_time=N/A\n",
        b"duration_time=0.021333\n",
    ];
    for packets in cases {
        let body = serde_json::to_vec(&json!({"streams": [stream("video", "0", "10"), stream("audio", "0", "10")]})).unwrap();
        let runner = FakeRunner::new(move |_, _| ok(&body));
        let lines: Vec<&[u8]> = packets.split_inclusive(|c| *c == b'\n').collect();
        let runner = runner.with_lines(&lines, 0);
        let r = renderer(runner);
        let err = block_on(r.validate_output_timing(Path::new("output.mp4"), 10_000, "30", true, &CancellationToken::new()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("timing validation failed"), "{err}");
    }
}

#[test]
fn audio_packet_probe_preserves_priming_and_final_partial_packet() {
    for code in [0, 1] {
        let mut checker = AudioPacketChecker::default();
        for line in [
            &b"pts_time=-0.021333|duration_time=0.021333|\n"[..],
            b"pts_time=0.000000|duration_time=0.021333\n",
            b"pts_time=0.021333|duration_time=0.005000\n",
        ] {
            checker.line(line).unwrap();
        }
        let result = checker.finish(code);
        if code == 0 {
            result.unwrap();
        } else {
            assert!(result.unwrap_err().contains("could not verify"));
        }
    }
}

#[test]
fn failed_probe_does_not_silently_drop_audio() {
    let r = renderer(FakeRunner::new(|_, _| fail(b"")));
    let err = block_on(r.has_audio(Path::new("source.mp4"), &CancellationToken::new())).unwrap_err().to_string();
    assert!(err.contains("source audio"), "{err}");
}

#[test]
fn failed_export_is_removed_before_fallback() {
    for encoder_failure in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let output = tmp.path().join("clip.mp4");
        let out2 = output.clone();
        let runner = FakeRunner::new(move |program, _| {
            if program == Path::new("ffmpeg") {
                std::fs::write(&out2, b"broken media").unwrap();
                return if encoder_failure { fail(b"FFmpeg failed") } else { ok(b"") };
            }
            // The timing probe rejects the export.
            ok(br#"{"streams": []}"#)
        });
        let r = renderer(runner);
        let job = FfmpegJob {
            input_path: Path::new("source.mp4"),
            output_path: &output,
            start_time_ms: 0,
            duration_ms: 10_000,
            filter_complex: "graph",
            audio_label: None,
            extra_inputs: &[],
            fps: "30",
            output_size: (1080, 1920),
            output_duration_ms: None,
        };
        assert!(block_on(r.run_ffmpeg_complex(&job, &CancellationToken::new())).is_err());
        assert!(!output.exists());
    }
}

#[test]
fn empty_edit_is_not_replaced_with_entire_source() {
    let p = plan(vec![shot(0, 1000, TALKING_HEAD)], 64, 64);
    let err = build_layout_graph(&p, 64, 64, &GraphOptions { keeps: Some(&[]), ..Default::default() }).unwrap_err();
    assert!(err.to_string().contains("no video"));
}

#[test]
fn short_filter_graph_stays_inline() {
    let runner = FakeRunner::new(|_, _| ok(b""));
    let r = renderer(runner.clone());
    let args = vec!["-filter_complex".to_string(), "[0:v]null[out]".to_string()];
    block_on(r.run_cmd(&args, &CancellationToken::new())).unwrap();
    assert_eq!(runner.calls(), vec![(PathBuf::from("ffmpeg"), args)]);
}

fn big_graph() -> String {
    format!("[0:v]{}null[out]", "null,".repeat(2000))
}

#[test]
fn large_filter_graph_uses_private_script_and_cleans_up() {
    for failing in [false, true] {
        let graph = big_graph();
        let expected = graph.clone();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let runner = FakeRunner::new(move |_, args| {
            assert_eq!(args[0], "-/filter_complex");
            seen2.lock().unwrap().push(args[1].clone());
            assert_eq!(std::fs::read_to_string(&args[1]).unwrap(), expected);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(&args[1]).unwrap().permissions().mode() & 0o077, 0);
            }
            if failing {
                Err(EngineError::msg("FFmpeg launch failed"))
            } else {
                ok(b"")
            }
        });
        let r = renderer(runner);
        let result = block_on(r.run_cmd(&["-filter_complex".to_string(), graph], &CancellationToken::new()));
        assert_eq!(result.is_err(), failing);
        if failing {
            assert!(result.unwrap_err().to_string().contains("FFmpeg launch failed"));
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(!Path::new(&seen[0]).exists());
    }
}

#[test]
fn large_filter_graph_retries_only_unsupported_file_option() {
    for legacy_succeeds in [true, false] {
        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let runner = FakeRunner::new(move |_, args| {
            seen2.lock().unwrap().push((args[0].clone(), args[1].clone()));
            assert!(Path::new(&args[1]).is_file());
            if args[0] == "-/filter_complex" {
                return fail(b"Unrecognized option '/filter_complex'.\nError splitting the argument list: Option not found");
            }
            assert_eq!(args[0], "-filter_complex_script");
            if legacy_succeeds {
                ok(b"")
            } else {
                fail(b"Invalid graph")
            }
        });
        let r = renderer(runner);
        let result = block_on(r.run_cmd(&["-filter_complex".to_string(), "null,".repeat(2000)], &CancellationToken::new()));
        if legacy_succeeds {
            result.unwrap();
        } else {
            assert!(result.unwrap_err().to_string().contains("Invalid graph"));
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), vec!["-/filter_complex", "-filter_complex_script"]);
        assert_eq!(seen[0].1, seen[1].1);
        assert!(!Path::new(&seen[0].1).exists());
    }
}

#[test]
fn large_filter_graph_does_not_retry_render_failures() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let runner = FakeRunner::new(move |_, args| {
        seen2.lock().unwrap().push(args[1].clone());
        assert!(Path::new(&args[1]).is_file());
        fail(b"A filter failed")
    });
    let r = renderer(runner);
    let err = block_on(r.run_cmd(&["-filter_complex".to_string(), "null,".repeat(2000)], &CancellationToken::new()))
        .unwrap_err()
        .to_string();
    assert!(err.contains("A filter failed") && err.contains("video render failed"));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(!Path::new(&seen[0]).exists());
}

#[test]
fn stderr_tail_is_bounded() {
    let runner = FakeRunner::new(|_, _| {
        let mut stderr = vec![b'x'; 5000];
        stderr.extend_from_slice(b"END");
        fail(&stderr)
    });
    let err = block_on(renderer(runner).run_cmd(&["-y".to_string()], &CancellationToken::new())).unwrap_err().to_string();
    assert!(err.ends_with("END") && err.len() < 1100, "{}", err.len());
}

// ---------------------------------------------------------------- video speed

#[test]
fn engine_rejects_invalid_speed() {
    for speed in [0.0, 0.5, 2.01, f64::NAN, f64::INFINITY] {
        assert!(validate_video_speed(speed).unwrap_err().to_string().contains("Video speed"));
        let tmp = tempfile::tempdir().unwrap();
        let request = RenderRequest { video_speed: speed, ..request_for(tmp.path()) };
        let (result, calls) = render_stubbed(&service(), &request, 0, ffmpeg_failed);
        assert!(result.unwrap_err().to_string().contains("Video speed"));
        assert!(calls.lock().unwrap().is_empty());
    }
}

#[test]
fn subtitles_and_chapters_share_the_sped_up_edited_timeline() {
    let tmp = tempfile::tempdir().unwrap();
    let transcript = vec![segment(
        34_000,
        38_000,
        "After the cut.",
        vec![word("After", 34_000, 35_000), word("the", 35_000, 36_000), word("cut.", 36_000, 38_000)],
    )];
    let original = transcript.clone();
    let request = RenderRequest {
        video_path: PathBuf::from("source.mp4"),
        output_path: tmp.path().join("clip.mp4"),
        start_time_ms: 10_000,
        end_time_ms: 130_000,
        source_width: 64,
        source_height: 64,
        video_speed: 2.0,
        longform: true,
        transcript_segments: transcript,
        chapters: vec![(10_000, "Start".into()), (34_000, "Too close".into()), (60_000, "Next".into())],
        ..Default::default()
    };
    let tm = TimeMap::new(vec![(0, 10_000), (20_000, 120_000)], Some(120_000));
    // Window offset 24 s minus 10 s removed, divided by 2 = 7 s.
    let sidecar = service().write_subtitles(&request, 10_000, &tm).expect("sidecar");
    // SRT readability adds the existing 400 ms hold after the final word.
    assert!(std::fs::read_to_string(sidecar).unwrap().contains("00:00:07,000 --> 00:00:09,400"));
    assert_eq!(
        output_chapters(&request, 10_000, &tm).unwrap(),
        vec![(0, "Start".to_string()), (20_000, "Next".to_string())]
    );
    assert_eq!(request.transcript_segments, original);
    assert_eq!(scaled_duration_ms(11_000, 1.5).unwrap(), 7333);
}

// ---------------------------------------------------------------- clip editor

fn speech(spans: &[(i64, i64, &str)]) -> Vec<WindowWord> {
    spans.iter().map(|(s, e, t)| WindowWord { start_ms: *s, end_ms: *e, text: t.to_string() }).collect()
}

fn w(spans: &[(i64, i64)]) -> Vec<WindowWord> {
    speech(&spans.iter().map(|(s, e)| (*s, *e, "word")).collect::<Vec<_>>())
}

fn plan_of(shots: &[(i64, i64, &str)]) -> ClipLayoutPlan {
    plan(shots.iter().map(|(a, bb, l)| shot(*a, *bb, l)).collect(), 1920, 1080)
}

fn removed(keeps: &[(i64, i64)], window_ms: i64) -> i64 {
    window_ms - keeps.iter().map(|(s, e)| e - s).sum::<i64>()
}

#[test]
fn continuous_speech_is_untouched() {
    let words = w(&(0..20).map(|i| (100 + i * 400, 450 + i * 400)).collect::<Vec<_>>());
    assert_eq!(compute_keep_intervals(&words, 8200, None, &[], false), vec![(0, 8200)]);
}

#[test]
fn long_pause_on_talking_head_is_compressed() {
    let p = plan_of(&[(0, 4200, TALKING_HEAD)]);
    let keeps = compute_keep_intervals(&w(&[(100, 900), (3000, 3800)]), 4200, Some(&p), &[], false);
    assert_eq!(keeps.len(), 2);
    assert!(keeps[1].0 - keeps[0].1 > 0);
    assert!((2100 - removed(&keeps, 4200) - KEEP_PAUSE_MS).abs() <= 40);
}

#[test]
fn screen_shot_pauses_are_protected() {
    let p = plan_of(&[(0, 5000, SCREEN)]);
    assert_eq!(compute_keep_intervals(&w(&[(100, 900), (4000, 4800)]), 5000, Some(&p), &[], false), vec![(0, 5000)]);
}

#[test]
fn screen_cam_and_unknown_layouts_only_cut_long_waits() {
    let p = plan_of(&[(0, 12000, SCREEN_CAM)]);
    for plan in [Some(&p), None] {
        let short = compute_keep_intervals(&w(&[(100, 900), (2400, 3000), (3000, 11900)]), 12000, plan, &[], false);
        assert_eq!(short, vec![(0, 12000)]);
        let long = compute_keep_intervals(&w(&[(100, 900), (4000, 4800), (4800, 11900)]), 12000, plan, &[], false);
        assert!(removed(&long, 12000) > 2000);
    }
}

#[test]
fn filler_gap_is_always_compressed() {
    let words = speech(&[(100, 900, "So"), (1000, 1500, "um,"), (1600, 2300, "anyway"), (2300, 9000, "rest")]);
    let p = plan_of(&[(0, 9100, TALKING_HEAD)]);
    let keeps = compute_keep_intervals(&words, 9100, Some(&p), &[], false);
    assert!(removed(&keeps, 9100) >= 400);
    assert!(keeps.iter().all(|(s, e)| !(*s <= 1250 && 1250 < *e)));
    assert!(keeps.iter().all(|(s, e)| !(*s < 1500 && *e > 1000)));
}

#[test]
fn reaction_is_never_cut() {
    let p = plan_of(&[(0, 5000, TALKING_HEAD)]);
    let keeps = compute_keep_intervals(&w(&[(100, 900), (4000, 4800)]), 5000, Some(&p), &[(900, 4000)], false);
    assert_eq!(keeps, vec![(0, 5000)]);
}

#[test]
fn lead_in_and_tail_silence_trimmed() {
    let p = plan_of(&[(0, 8000, TALKING_HEAD)]);
    let keeps = compute_keep_intervals(&w(&[(1500, 2300), (2300, 6000)]), 8000, Some(&p), &[], false);
    assert!(keeps[0].0 > 1000 && keeps[keeps.len() - 1].1 < 7000);
}

#[test]
fn tail_only_cut_survives_non_frame_aligned_window_end() {
    let p = plan_of(&[(0, 10150, SCREEN_CAM)]);
    let keeps = compute_keep_intervals(&w(&[(300, 650), (700, 1050)]), 10150, Some(&p), &[], false);
    assert_eq!(keeps.len(), 1);
    assert_eq!(keeps[0].0, 0);
    assert!((1400..=1500).contains(&keeps[0].1));
}

#[test]
fn detected_layout_wins_over_forced_style() {
    let mut p = plan_of(&[(0, 5000, SCREEN)]);
    p.shots[0].detected_layout = Some(TALKING_HEAD.into());
    assert_eq!(compute_keep_intervals(&w(&[(100, 900), (4000, 4800)]), 5000, Some(&p), &[], false).len(), 2);
}

#[test]
fn pieces_on_frame_grid_and_not_slivers() {
    let words = w(&(0..8).map(|i| (i * 1700, i * 1700 + 500)).collect::<Vec<_>>());
    let p = plan_of(&[(0, 14000, TALKING_HEAD)]);
    let keeps = compute_keep_intervals(&words, 14000, Some(&p), &[], false);
    let last = keeps[keeps.len() - 1];
    for &(start, end) in &keeps {
        assert!(end - start >= MIN_PIECE_MS || (start, end) == last);
        for t in [start, end] {
            let frames = t as f64 / (1000.0 / 30.0);
            assert!((frames - frames.round()).abs() < 0.05 || t == 14000);
        }
    }
}

#[test]
fn longform_keeps_natural_pauses() {
    let p = plan_of(&[(0, 6000, TALKING_HEAD)]);
    let words = w(&[(100, 900), (2000, 2800), (5000, 5900)]);
    let short = compute_keep_intervals(&words, 6000, Some(&p), &[], false);
    let long = compute_keep_intervals(&words, 6000, Some(&p), &[], true);
    assert!(removed(&long, 6000) < removed(&short, 6000));
}

#[test]
fn fillers() {
    for word in ["um", "Uh,", "umm...", "Hmm"] {
        assert!(is_filler(word), "{word}");
    }
    for word in ["so", "like", "ah", "umbrella", "her"] {
        assert!(!is_filler(word), "{word}");
    }
}

#[test]
fn time_map_detects_tail_only_cut_when_window_length_is_known() {
    let tm = TimeMap::new(vec![(0, 2400)], Some(5000));
    assert_eq!(tm.output_ms, 2400);
    assert_eq!(tm.removed_ms(), 2600);
    assert!(tm.has_cuts());
    assert_eq!(tm.cut_count(), 1);
}

#[test]
fn time_map_maps_and_clamps() {
    let tm = TimeMap::new(vec![(0, 1000), (2000, 3000)], None);
    assert_eq!(tm.output_ms, 2000);
    assert_eq!(tm.to_output(500), Some(500));
    assert_eq!(tm.to_output(2500), Some(1500));
    assert_eq!(tm.to_output(1500), None);
    assert_eq!(tm.to_output_clamped(1500), 1000);
}

#[test]
fn remap_segments_drops_fillers_and_shifts_words() {
    let words = vec![word("Hello", 10_000, 10_400), word("um", 10_500, 10_800), word("world.", 12_000, 12_400)];
    let segs = vec![segment(10_000, 12_400, "Hello um world.", words)];
    let out = remap_segments(&segs, 10_000, &TimeMap::new(vec![(0, 450), (1900, 2600)], None));
    assert_eq!(out[0].words.iter().map(|w| w.word.as_str()).collect::<Vec<_>>(), vec!["Hello", "world."]);
    assert_eq!(out[0].words[1].start_time_ms, 10_000 + 450 + 100);
}

#[test]
fn keeps_filler_caption_when_audio_was_not_cut() {
    let segs = vec![segment(10_500, 10_800, "um", vec![word("um", 10_500, 10_800)])];
    let out = remap_segments(&segs, 10_000, &TimeMap::new(vec![(0, 2000)], None));
    assert_eq!(out[0].words.iter().map(|w| w.word.as_str()).collect::<Vec<_>>(), vec!["um"]);
}

#[test]
fn reaction_intervals_follow_event_sentences() {
    let segs = vec![
        TranscriptSegment { audio_events: vec!["(laughter)".into()], ..segment(0, 1000, "Joke.", vec![]) },
        segment(3000, 4000, "Next.", vec![]),
    ];
    assert_eq!(reaction_intervals(&segs, 0, 5000), vec![(1000, 3000)]);
}

// ---------------------------------------------------------------- many clips

#[test]
fn render_clips_keeps_order_reports_progress_and_isolates_failures() {
    let tmp = tempfile::tempdir().unwrap();
    // The first clip's source probe fails the audio check; the second renders.
    let runner = FakeRunner::new(|program, args| {
        if program == Path::new("ffprobe") {
            if args.iter().any(|a| a == "stream=index") {
                return if args.last().unwrap().ends_with("bad.mp4") { fail(b"") } else { ok(b"") };
            }
            if args.iter().any(|a| a == "stream=codec_type,start_time,duration") {
                return ok(br#"{"streams":[{"codec_type":"video","start_time":"0","duration":"10.15"}]}"#);
            }
            return ok(b"64x64\n");
        }
        std::fs::write(args.last().unwrap(), b"mp4").unwrap();
        ok(b"")
    });
    let r = renderer(runner);
    let good = RenderRequest {
        video_path: PathBuf::from("good.mp4"),
        output_path: tmp.path().join("clip_01.mp4"),
        end_time_ms: 10_000,
        pacing: "natural".into(),
        ..Default::default()
    };
    let bad = RenderRequest { video_path: PathBuf::from("bad.mp4"), output_path: tmp.path().join("clip_00.mp4"), ..good.clone() };
    let seen: Arc<Mutex<Vec<(u32, u32, f64)>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let progress: crate::types::ProgressFn =
        Arc::new(move |p: crate::types::Progress| seen2.lock().unwrap().push((p.clips_done, p.clips_total, p.percent)));
    let results = block_on(crate::render::render_clips(&r, &[bad, good], 2, Some(progress), &CancellationToken::new()));
    assert!(results[0].as_ref().unwrap_err().to_string().contains("video render failed"));
    let ok_result = results[1].as_ref().unwrap();
    assert_eq!(ok_result.duration_ms, 10_150);
    assert_eq!(ok_result.output_path, tmp.path().join("clip_01.mp4"));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!((seen[1].0, seen[1].1), (2, 2));
    assert!((seen[1].2 - 90.0).abs() < 1e-9);
}
