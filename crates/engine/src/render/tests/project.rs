//! Editable projects: the graph for a range matches the whole export, and an untouched
//! project re-renders the clip it came from. Media checks are skipped without FFmpeg;
//! captions need libass (point TEST_FFMPEG / TEST_FFPROBE at engine-bin/).
use super::support::*;
use crate::layout::types::{Box as LBox, ShotLayout, SCREEN, STACK, TALKING_HEAD};
use crate::render::layout_renderer::{caption_anchor, stack_panels};
use crate::render::process::SystemRunner;
use crate::render::project::{
    AudioItem, ColorAdjust, Crop, CropKey, LogoLayer, MediaFile, OverlayBox, PipItem, Project, ProjectGraph, ProjectRender, TextLayer, VideoItem, FRAMING_AUTO, FRAMING_CROP,
    FRAMING_FIT, FRAMING_SPLIT, FRAMING_SPLIT3,
};
use crate::render::service::{RenderRequest, Renderer};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn item(id: &str, src_in_ms: i64, src_out_ms: i64) -> VideoItem {
    VideoItem { id: id.into(), src_in_ms, src_out_ms, framing: FRAMING_AUTO.into(), ..Default::default() }
}

fn project(video: Vec<VideoItem>) -> Project {
    let mut talking = shot(1_000, 9_000, TALKING_HEAD);
    talking.focus_path = vec![(0, 0.3, 0.5), (4_000, 0.7, 0.5)];
    Project {
        version: 1,
        source_width: 320,
        source_height: 240,
        width: 180,
        height: 320,
        fps: "30".into(),
        aspect_ratio: "9:16".into(),
        video,
        shots: vec![talking],
        face_samples: vec![],
        transcript: vec![segment(
            0,
            9_000,
            "one two three four",
            vec![word("one", 1_100, 1_900), word("two", 2_100, 2_900), word("three", 4_100, 4_900), word("four", 5_100, 5_900)],
        )],
        captions: true,
        caption_preset: "pop".into(),
        emphasis_words: vec![],
        title: Some("Title".into()),
        banner_platform: None,
        banner_channel_url: None,
        logos: vec![],
        speed: 1.0,
        texts: vec![],
        audio: vec![],
        title_box: None,
        banner_box: None,
        media: vec![],
        pip: vec![],
        backdrop: None,
    }
}

fn text_layer(start_ms: i64, end_ms: i64) -> TextLayer {
    TextLayer { id: "t0".into(), text: "Big {news}\nhere".into(), start_ms, end_ms, x: 0.5, y: 0.25, size: 40, preset: "spotlight".into(), animation_in: "none".into(), animation_out: "none".into() }
}

#[test]
fn a_range_reads_only_the_pieces_it_covers() {
    // Reordered: source 4-6 s plays first, then 1-3 s.
    let p = project(vec![item("a", 4_000, 6_000), item("b", 1_000, 3_000)]);
    let ProjectGraph { reads, graph, .. } = p.pieces_graph((1_500, 2_500), Path::new("s.mp4"), true, Path::new("."), "anull").unwrap();
    assert_eq!(reads, [(5_500, 500), (1_000, 500)]);
    assert!(graph.contains("[p0_base][pa0][p1_base][pa1]concat=n=2:v=1:a=1[vcat][acat]"), "{graph}");
    // The range's clock starts where it sits on the timeline.
    assert!(graph.contains("[vcat]setpts=PTS+1.500000/TB[base]"), "{graph}");
    assert!(graph.contains("[0:v]") && graph.contains("[1:a:0]") && !graph.contains("[p0_0:v]"), "{graph}");
}

#[test]
fn a_trimmed_piece_keeps_the_pan_where_it_was() {
    // The shot starts at 1 s with focus points at +0 s and +4 s; the piece starts at 3 s.
    let p = project(vec![item("a", 3_000, 6_000)]);
    let graph = p.pieces_graph((0, 3_000), Path::new("s.mp4"), false, Path::new("."), "anull").unwrap().graph;
    let whole = project(vec![item("a", 1_000, 6_000)]);
    let whole_graph = whole.pieces_graph((0, 5_000), Path::new("s.mp4"), false, Path::new("."), "anull").unwrap().graph;
    // Same pan points, shifted by the 2 s trimmed off: -2 s and +2 s from the piece start.
    assert!(graph.contains("if(lt(t,2.000),"), "{graph}");
    assert!(whole_graph.contains("if(lt(t,4.000),"), "{whole_graph}");

    let mut fit = project(vec![item("a", 3_000, 6_000)]);
    fit.video[0].framing = FRAMING_FIT.into();
    let fit_graph = fit.pieces_graph((0, 3_000), Path::new("s.mp4"), false, Path::new("."), "anull").unwrap().graph;
    assert!(!fit_graph.contains("if(lt(t,"), "{fit_graph}");
}

#[test]
fn validate_rejects_pieces_outside_the_source() {
    assert!(project(vec![item("a", 1_000, 3_000)]).validate(10_000).is_ok());
    assert!(project(vec![item("a", 1_000, 12_000)]).validate(10_000).is_err());
    assert!(project(vec![item("a", 3_000, 3_000)]).validate(10_000).is_err());
    assert!(project(vec![]).validate(10_000).is_err());
    let mut loud = project(vec![item("a", 1_000, 3_000)]);
    loud.video[0].volume = 3.0;
    assert!(loud.validate(10_000).is_err());
}

// ---- media -----------------------------------------------------------------------

fn real_renderer(ffmpeg: &Path, ffprobe: &Path) -> Renderer {
    let mut r = renderer(Arc::new(SystemRunner));
    r.ffmpeg = ffmpeg.to_path_buf();
    r.ffprobe = ffprobe.to_path_buf();
    r.video_codec_override = Some(["-c:v", "mpeg4", "-q:v", "2", "-bf", "2"].map(String::from).to_vec());
    r
}

fn has_libass(ffmpeg: &Path) -> bool {
    let out = Command::new(ffmpeg).args(["-hide_banner", "-filters"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).lines().any(|l| l.split_whitespace().nth(1) == Some("ass"))
}

/// 10 s of testsrc2 (every frame differs) with a tone.
fn source_video(ffmpeg: &Path, dir: &Path) -> PathBuf {
    let path = dir.join("source.mkv");
    let status = Command::new(ffmpeg)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=10"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=10", "-c:v", "ffv1", "-c:a", "pcm_s16le"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    path
}

/// One frame at `t` seconds as 90x160 grayscale bytes.
fn frame(ffmpeg: &Path, path: &Path, t: f64) -> Vec<u8> {
    let out = Command::new(ffmpeg)
        .args(["-v", "error", "-ss", &format!("{t:.3}"), "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf", "scale=90:160:flags=bicubic", "-pix_fmt", "gray", "-f", "rawvideo", "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.stdout.len(), 90 * 160);
    out.stdout
}

/// RMS of `seconds` of mono audio from `start`.
fn rms(ffmpeg: &Path, path: &Path, start: f64, seconds: f64) -> f64 {
    let out = Command::new(ffmpeg)
        .args(["-v", "error", "-ss", &format!("{start:.3}"), "-t", &format!("{seconds:.3}"), "-i"])
        .arg(path)
        .args(["-ac", "1", "-f", "f32le", "-"])
        .output()
        .unwrap();
    let samples: Vec<f64> =
        out.stdout.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64).collect();
    assert!(!samples.is_empty());
    (samples.iter().map(|s| s * s).sum::<f64>() / samples.len() as f64).sqrt()
}

fn mean_diff(a: &[u8], b: &[u8]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len() as f64
}

#[test]
fn a_preview_range_shows_the_exports_frames() {
    let Some((ffmpeg, ffprobe)) = ffmpeg_tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let source = source_video(&ffmpeg, dir.path());
    let r = real_renderer(&ffmpeg, &ffprobe);
    let mut p = project(vec![item("a", 4_000, 6_000), item("b", 1_000, 3_000), item("c", 6_000, 7_000)]);
    p.captions = has_libass(&ffmpeg);
    let cancel = CancellationToken::new();

    let loudness = block_on(r.measure_project_loudness(&p, &source, dir.path(), &cancel)).unwrap();
    assert!(loudness.as_deref().is_some_and(|f| f.contains("linear=true")), "{loudness:?}");
    let export = dir.path().join("export.mp4");
    let job = ProjectRender { project: &p, source: &source, output_path: &export, range: None, scale_to: None, loudness_filter: loudness, media_dir: dir.path(), progress: None };
    block_on(r.render_project(&job, &cancel)).unwrap();
    // The middle of the timeline, across the cut between the second and third pieces, at half size.
    let preview = dir.path().join("preview.mp4");
    let job = ProjectRender { range: Some((3_000, 4_500)), scale_to: Some((90, 160)), output_path: &preview, ..job };
    block_on(r.render_project(&job, &cancel)).unwrap();

    for t in [0.2, 0.9, 1.1, 1.4] {
        let diff = mean_diff(&frame(&ffmpeg, &export, 3.0 + t), &frame(&ffmpeg, &preview, t));
        assert!(diff < 4.0, "preview differs from the export at +{t}s: {diff}");
    }
    // The same loudness: the range's sound matches the export's at that point.
    let level = |path: &Path, start: f64| rms(&ffmpeg, path, start, 1.0);
    let (whole, part) = (level(&export, 3.2), level(&preview, 0.2));
    assert!((whole - part).abs() < 0.02 * whole.max(1e-6) + 1e-4, "export {whole} vs preview {part}");
    // A frame from a neighbouring moment differs clearly, so the check can fail.
    let off = mean_diff(&frame(&ffmpeg, &export, 3.5), &frame(&ffmpeg, &preview, 0.2));
    assert!(off > 8.0, "frames too similar to prove alignment: {off}");
}

#[test]
fn an_untouched_project_renders_its_clip() {
    let Some((ffmpeg, ffprobe)) = ffmpeg_tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let source = source_video(&ffmpeg, dir.path());
    let r = real_renderer(&ffmpeg, &ffprobe);
    let request = RenderRequest {
        video_path: source.clone(),
        output_path: dir.path().join("clip.mp4"),
        start_time_ms: 2_000,
        end_time_ms: 6_000,
        source_width: 320,
        source_height: 240,
        include_captions: false,
        title_text: Some("Clip".into()),
        layout_style: "fit".into(),
        pacing: "natural".into(),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let result = block_on(r.render_clip(&request, &cancel)).unwrap();
    let p = Project::from_render(&request, &result, "pop", &[]).unwrap();
    assert_eq!(p.duration_ms(), result.duration_ms);

    let export = dir.path().join("reexport.mp4");
    let job = ProjectRender { project: &p, source: &source, output_path: &export, range: None, scale_to: None, loudness_filter: None, media_dir: dir.path(), progress: None };
    block_on(r.render_project(&job, &cancel)).unwrap();
    for t in [0.1, 1.7, 3.5] {
        let diff = mean_diff(&frame(&ffmpeg, &result.output_path, t), &frame(&ffmpeg, &export, t));
        assert!(diff < 4.0, "re-export differs from the clip at {t}s: {diff}");
    }
}

#[test]
fn shots_serialize_into_project_files() {
    let p = project(vec![item("a", 1_000, 2_000)]);
    let back: Project = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
    assert_eq!(back.video, p.video);
    let ShotLayout { focus_path, .. } = &back.shots[0];
    assert_eq!(focus_path, &p.shots[0].focus_path);
}

#[test]
fn text_layers_join_the_caption_file_in_their_presets_look() {
    let mut p = project(vec![item("a", 1_000, 3_000)]);
    assert_eq!(p.with_text_layers(None), None);
    p.texts = vec![text_layer(500, 1_500)];
    p.validate(10_000).unwrap();
    // No captions: a document of its own, one centred line per text line, braces dropped.
    let alone = p.with_text_layers(None).unwrap();
    assert!(alone.contains("PlayResX: 180\nPlayResY: 320"), "{alone}");
    let face = alone.lines().find(|l| l.starts_with("Dialogue: 4,")).unwrap();
    assert!(face.starts_with("Dialogue: 4,0:00:00.50,0:00:01.50,"), "{face}");
    assert!(face.contains(r"\an5\pos(90,80)\fnPoppins Black\fs40"), "{face}");
    assert!(face.contains(r"}BIG NEWS\N{") && face.ends_with("}HERE"), "{face}");
    // With captions: appended to them.
    let with = p.with_text_layers(Some("[Events]\nDialogue: caption\n".into())).unwrap();
    assert!(with.starts_with("[Events]\nDialogue: caption\nDialogue: "), "{with}");

    p.texts[0].x = 1.5;
    assert!(p.validate(10_000).is_err());
}

#[test]
fn a_text_layer_shows_in_the_export_and_its_preview() {
    let Some((ffmpeg, ffprobe)) = ffmpeg_tools() else { return };
    if !has_libass(&ffmpeg) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = source_video(&ffmpeg, dir.path());
    let r = real_renderer(&ffmpeg, &ffprobe);
    let mut p = project(vec![item("a", 1_000, 4_000)]);
    p.captions = false;
    p.title = None;
    let cancel = CancellationToken::new();
    let render = |p: &Project, name: &str, range: Option<(i64, i64)>| {
        let out = dir.path().join(name);
        let job = ProjectRender { project: p, source: &source, output_path: &out, range, scale_to: None, loudness_filter: None, media_dir: dir.path(), progress: None };
        block_on(r.render_project(&job, &cancel)).unwrap();
        out
    };
    let plain = render(&p, "plain.mp4", None);
    p.texts = vec![text_layer(1_000, 2_000)];
    let texted = render(&p, "texted.mp4", None);
    let preview = render(&p, "preview.mp4", Some((1_000, 2_000)));

    let at = |a: &Path, ta: f64, b: &Path, tb: f64| mean_diff(&frame(&ffmpeg, a, ta), &frame(&ffmpeg, b, tb));
    assert!(at(&plain, 0.5, &texted, 0.5) < 1.0, "text shown before its start");
    assert!(at(&plain, 1.5, &texted, 1.5) > 2.0, "text missing while it should show");
    assert!(at(&plain, 2.5, &texted, 2.5) < 1.0, "text shown after its end");
    assert!(at(&texted, 1.5, &preview, 0.5) < 1.0, "preview text differs from the export");
}

fn sound(id: &str, file: Option<&str>, (src_in_ms, src_out_ms): (i64, i64), start_ms: i64, duck: bool) -> AudioItem {
    AudioItem { id: id.into(), name: String::new(), file: file.map(Into::into), src_in_ms, src_out_ms, start_ms, volume: 1.0, fade_in_ms: 0, fade_out_ms: 0, duck, voice: false, speed: 1.0 }
}

#[test]
fn sounds_mix_under_the_voice_with_fades() {
    let mut p = project(vec![item("a", 1_000, 3_000)]);
    p.video[0].fade_in_ms = 500;
    p.audio = vec![
        AudioItem { volume: 0.5, fade_out_ms: 1_000, ..sound("m", Some("audio_1.m4a"), (2_000, 6_000), 500, true) },
        sound("d", None, (5_000, 6_000), 1_500, false),
    ];
    p.validate(10_000).unwrap();
    let g = p.pieces_graph((1_000, 2_000), Path::new("s.mp4"), true, Path::new("/run"), "anull").unwrap();
    // Ducked music plays here, so the range renders from 0.6 s earlier (0.4 s) and cuts
    // that off: the music from its start at 0.5 s, the detached sound from 1.5 s.
    assert_eq!(g.audio_reads, [(PathBuf::from("/run/audio_1.m4a"), 2_000, 1_500), (PathBuf::from("s.mp4"), 5_000, 500)]);
    assert!(g.graph.contains("[1:a:0]") && g.graph.contains("[2:a:0]"), "{}", g.graph);
    assert!(g.graph.contains("volume='1*min(1,(t+0.400000)/0.500000)':eval=frame[pa0]"), "{}", g.graph);
    assert!(g.graph.contains("volume='0.5*min(1,max(0,(4.000000-t-0.000000)/1.000000))':eval=frame,adelay=100:all=1[au0]"), "{}", g.graph);
    assert!(g.graph.contains("adelay=1100:all=1[au1]"), "{}", g.graph);
    assert!(g.graph.contains("[vcat]trim=start_frame=18,setpts=PTS-STARTPTS,setpts=PTS+1.000000/TB[base]"), "{}", g.graph);
    // The main track and the detached sound are the voice the music ducks under.
    assert!(g.graph.contains("[acat][au1]amix=inputs=2") && g.graph.contains("[music][voice_key]sidechaincompress"), "{}", g.graph);
    assert!(g.graph.contains("apad=whole_len=76800,atrim=start_sample=28800:end_sample=76800,asetpts=PTS-STARTPTS"), "{}", g.graph);
    // Without ducking there is no lead.
    let mut plain = p.clone();
    plain.audio[0].duck = false;
    let g = plain.pieces_graph((1_000, 2_000), Path::new("s.mp4"), true, Path::new("/run"), "anull").unwrap();
    assert!(g.graph.contains("apad=whole_len=48000,atrim=end_sample=48000") && g.graph.contains("[vcat]setpts=PTS+"), "{}", g.graph);
    // A sound at 2x reads twice its timeline length and speeds up after the read.
    plain.audio[0].speed = 2.0;
    plain.validate(10_000).unwrap();
    let g = plain.pieces_graph((1_000, 2_000), Path::new("s.mp4"), true, Path::new("/run"), "anull").unwrap();
    assert_eq!(g.audio_reads[0], (PathBuf::from("/run/audio_1.m4a"), 3_000, 2_000));
    assert!(g.graph.contains("asetpts=PTS-STARTPTS,atempo=2,volume='0.5*min(1,max(0,(2.000000-t-0.500000)/1.000000))'"), "{}", g.graph);

    // A voiceover over a silent source is the voice the music ducks under.
    let mut vo = p.clone();
    vo.audio[1] = AudioItem { voice: true, ..sound("vo", Some("audio_2.m4a"), (0, 1_000), 1_500, true) };
    let g = vo.pieces_graph((1_000, 2_000), Path::new("s.mp4"), false, Path::new("/run"), "anull").unwrap();
    assert!(g.graph.contains("[au1]anull[voice]") && g.graph.contains("[music][voice_key]sidechaincompress"), "{}", g.graph);

    // No voice (a silent source): the music plays as it is.
    p.audio.truncate(1);
    let silent = p.pieces_graph((1_000, 2_000), Path::new("s.mp4"), false, Path::new("/run"), "anull").unwrap();
    assert!(silent.audio && !silent.graph.contains("sidechaincompress"), "{}", silent.graph);
    p.audio.clear();
    assert!(!p.pieces_graph((1_000, 2_000), Path::new("s.mp4"), false, Path::new("/run"), "anull").unwrap().audio);

    for bad in ["../x.m4a", "/abs.m4a", ".hidden", "a\\b"] {
        p.audio = vec![sound("m", Some(bad), (0, 1_000), 0, false)];
        assert!(p.validate(10_000).is_err(), "{bad}");
    }
}

#[test]
fn music_ducks_under_speech_and_previews_match() {
    let Some((ffmpeg, ffprobe)) = ffmpeg_tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let source = source_video(&ffmpeg, dir.path());
    let wav = dir.path().join("song.wav");
    let status = Command::new(&ffmpeg)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=1000:duration=8"])
        .arg(&wav)
        .status()
        .unwrap();
    assert!(status.success());
    let cancel = CancellationToken::new();
    let imported = dir.path().join("audio_1.m4a");
    let length = block_on(crate::render::project::import_audio(&ffmpeg, &ffprobe, &wav, &imported, &cancel)).unwrap();
    assert!((length - 8_000).abs() < 100, "{length}");
    let peaks = block_on(crate::render::project::waveform(&ffmpeg, &imported, &cancel)).unwrap();
    assert!((peaks.len() as i64 - 160).abs() <= 2 && peaks[10..150].iter().all(|&p| p > 50), "{peaks:?}");

    // Speech (the tone) for 3 s, then a muted piece; music under all of it.
    let r = real_renderer(&ffmpeg, &ffprobe);
    let mut p = project(vec![item("a", 1_000, 4_000), item("b", 4_000, 7_000)]);
    (p.captions, p.title) = (false, None);
    p.video[1].volume = 0.0;
    let render = |p: &Project, name: &str, range: Option<(i64, i64)>| {
        let out = dir.path().join(name);
        // No loudness levelling: compare raw levels.
        let job = ProjectRender { project: p, source: &source, output_path: &out, range, scale_to: None, loudness_filter: Some("anull".into()), media_dir: dir.path(), progress: None };
        block_on(r.render_project(&job, &cancel)).unwrap();
        out
    };
    p.audio = vec![sound("m", Some("audio_1.m4a"), (0, 6_000), 0, false)];
    let loud = render(&p, "loud.mp4", None);
    p.audio[0].duck = true;
    let ducked = render(&p, "ducked.mp4", None);
    let part = render(&p, "part.mp4", Some((1_000, 3_000)));

    let level = |path: &Path, at: f64| rms(&ffmpeg, path, at, 0.8);
    let (speech_loud, speech_ducked) = (level(&loud, 1.2), level(&ducked, 1.2));
    assert!(speech_ducked < speech_loud * 0.85, "music not ducked under speech: {speech_ducked} vs {speech_loud}");
    let (quiet_loud, quiet_ducked) = (level(&loud, 4.8), level(&ducked, 4.8));
    assert!((quiet_ducked - quiet_loud).abs() < quiet_loud * 0.05, "music ducked without speech: {quiet_ducked} vs {quiet_loud}");
    let (whole, range) = (level(&ducked, 1.5), level(&part, 0.5));
    assert!((whole - range).abs() < whole * 0.05, "preview sounds different: {range} vs {whole}");
}

#[test]
fn piece_speed_crop_and_transitions_shape_the_timeline() {
    // 2 s of source at 2x plays for 1 s; the next piece dips in from black.
    let mut p = project(vec![VideoItem { speed: 2.0, ..item("a", 1_000, 3_000) }, VideoItem { transition_ms: 600, ..item("b", 4_000, 6_000) }]);
    p.validate(10_000).unwrap();
    assert_eq!(p.duration_ms(), 3_000);
    // Words move with the speed: "two" (source 2.1 s) plays at 0.55 s.
    let (plan, words) = p.timeline();
    let two = words.iter().flat_map(|s| &s.words).find(|w| w.word == "two").unwrap();
    assert_eq!((two.start_time_ms, two.end_time_ms), (550, 950));
    assert_eq!(plan.shots.last().unwrap().end_ms, 3_000);
    // A range in the fast piece reads twice its length; the fades sit either side of the cut.
    let g = p.pieces_graph((500, 1_500), Path::new("s.mp4"), true, Path::new("."), "anull").unwrap();
    assert_eq!(g.reads, [(2_000, 1_000), (4_000, 500)]);
    assert!(g.graph.contains("setpts=(PTS-STARTPTS)/2,fps=30") && g.graph.contains("atempo=2,apad=whole_len=24000"), "{}", g.graph);
    assert!(g.graph.contains("fade=t=out:st=0.700000:d=0.300000"), "{}", g.graph);
    assert!(g.graph.contains("setpts=PTS-STARTPTS+0.000000/TB,fade=t=in:st=0:d=0.300000"), "{}", g.graph);

    // Crop: the canvas's shape, zoomed, kept inside the 320x240 source.
    assert_eq!(p.crop_rect(Crop { x: 0.5, y: 0.5, zoom: 1.0 }), (134, 240, 93, 0));
    assert_eq!(p.crop_rect(Crop { x: 1.0, y: 0.0, zoom: 2.0 }), (66, 120, 254, 0));
    p.video[0] = VideoItem { framing: FRAMING_CROP.into(), crop: Some(Crop { x: 0.5, y: 0.5, zoom: 2.0 }), ..item("a", 1_000, 3_000) };
    let g = p.pieces_graph((0, 1_000), Path::new("s.mp4"), false, Path::new("."), "anull").unwrap();
    assert!(g.graph.contains("[0:v]crop=66:120:127:60[p0_crop]") && g.graph.contains("[p0_crop]fps="), "{}", g.graph);

    for bad in [
        VideoItem { speed: 5.0, ..item("x", 0, 1_000) },
        VideoItem { transition_ms: 3_000, ..item("x", 0, 5_000) },
        VideoItem { framing: FRAMING_CROP.into(), ..item("x", 0, 1_000) },
    ] {
        let q = Project { video: vec![bad.clone()], ..p.clone() };
        assert!(q.validate(10_000).is_err(), "{bad:?}");
    }
    let q = Project { logos: vec![LogoLayer { path: "/etc/x.png".into(), position: "top-left".into(), size: 0.1, placed: None }], ..p.clone() };
    assert!(q.validate(10_000).is_err());
}

#[test]
fn edits_render_speed_transitions_crop_and_placed_overlays() {
    let Some((ffmpeg, ffprobe)) = ffmpeg_tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let source = source_video(&ffmpeg, dir.path());
    let made = Command::new(&ffmpeg)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=red:size=40x40", "-frames:v", "1"])
        .arg(dir.path().join("logo_0.png"))
        .status()
        .unwrap();
    assert!(made.success());
    let r = real_renderer(&ffmpeg, &ffprobe);
    let cancel = CancellationToken::new();
    let mut p = project(vec![
        VideoItem { speed: 2.0, ..item("a", 1_000, 3_000) },
        VideoItem { transition_ms: 600, framing: FRAMING_CROP.into(), crop: Some(Crop { x: 0.3, y: 0.4, zoom: 1.5 }), ..item("b", 4_000, 6_000) },
    ]);
    (p.captions, p.title) = (false, None);
    let render = |p: &Project, name: &str, range: Option<(i64, i64)>| {
        let out = dir.path().join(name);
        let job = ProjectRender { project: p, source: &source, output_path: &out, range, scale_to: None, loudness_filter: None, media_dir: dir.path(), progress: None };
        block_on(r.render_project(&job, &cancel)).unwrap();
        out
    };
    let plain = render(&p, "plain.mp4", None);
    p.logos = vec![LogoLayer {
        path: "logo_0.png".into(),
        position: "top-left".into(),
        size: 0.2,
        placed: Some(OverlayBox { x: 0.5, y: 0.5, scale: 1.0, start_ms: 0, end_ms: 800 }),
    }];
    let export = render(&p, "export.mp4", None);
    let part = render(&p, "part.mp4", Some((500, 1_500)));

    let seconds = |path: &Path| {
        let out = Command::new(&ffprobe).args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]).arg(path).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap()
    };
    assert!((seconds(&export) - 3.0).abs() < 0.1, "{}", seconds(&export));
    // The range matches the export across the fast piece, the cut and the dip.
    for t in [0.1, 0.45, 0.6, 0.9] {
        let diff = mean_diff(&frame(&ffmpeg, &export, 0.5 + t), &frame(&ffmpeg, &part, t));
        assert!(diff < 4.0, "preview differs from the export at +{t}s: {diff}");
    }
    // Black at the cut, normal away from it.
    let luma = |t: f64| frame(&ffmpeg, &export, t).iter().map(|&v| v as f64).sum::<f64>() / (90.0 * 160.0);
    assert!(luma(1.0) < 25.0 && luma(2.2) > 60.0, "dip: {} vs {}", luma(1.0), luma(2.2));
    // The placed logo shows only over its time.
    assert!(mean_diff(&frame(&ffmpeg, &plain, 0.3), &frame(&ffmpeg, &export, 0.3)) > 1.0, "logo missing");
    assert!(mean_diff(&frame(&ffmpeg, &plain, 2.5), &frame(&ffmpeg, &export, 2.5)) < 1.0, "logo shown after its end");
    // The editor's title preview: an image and its automatic centre.
    p.title = Some("Big news today".into());
    let previews = r.overlay_previews(&p, dir.path(), 500).unwrap();
    assert_eq!(previews.len(), 1);
    assert!(previews[0].path.is_file() && previews[0].width > 50 && previews[0].auto_x == 0.5 && previews[0].auto_y < 0.5, "{:?}", previews[0]);
    // A one-word title is drawn once (it used to read "Zoo Zoo").
    let width = |title: &str| {
        let q = Project { title: Some(title.into()), ..p.clone() };
        r.overlay_previews(&q, dir.path(), 0).unwrap()[0].width
    };
    assert!(width("Zoo") < width("Zoo Zoo") * 3 / 4, "{} vs {}", width("Zoo"), width("Zoo Zoo"));
}

#[test]
fn a_dissolve_overlaps_pieces_and_blends_them() {
    // b dissolves into a over 1 s: they overlap, so the edit is 1 s shorter.
    let mut p = project(vec![item("a", 1_000, 3_000), VideoItem { dissolve: true, transition_ms: 1_000, ..item("b", 4_000, 7_000) }]);
    p.validate(10_000).unwrap();
    assert_eq!(p.duration_ms(), 4_000);
    // Captions switch half way through the overlap (1.5 s): "two" (a, 1.1 s) plays and
    // ends at the switch; b's "three" (1.1 s) starts before it and is dropped; "four" plays.
    let (plan, words) = p.timeline();
    let at = |w: &str| words.iter().flat_map(|s| &s.words).find(|x| x.word == w).map(|x| (x.start_time_ms, x.end_time_ms));
    assert_eq!((at("two"), at("three"), at("four")), (Some((1_100, 1_500)), None, Some((2_100, 2_900))));
    assert!(plan.shots.windows(2).all(|w| w[0].end_ms <= w[1].start_ms), "{:?}", plan.shots);
    // A range inside the overlap reads both pieces and fades b in, timed from the overlap's start.
    let g = p.pieces_graph((1_250, 1_750), Path::new("s.mp4"), true, Path::new("."), "anull").unwrap();
    // Both pieces are read over the whole overlap (1-2 s), blended, then cut to the range.
    assert_eq!(g.reads, [(2_000, 1_000), (4_000, 1_000)]);
    assert!(g.graph.contains("xfade=transition=fade:duration=1.000000:offset=0,trim=start_frame=8:end_frame=23"), "{}", g.graph);
    assert!(g.graph.contains("atrim=start_sample=12000:end_sample=36000"), "{}", g.graph);
    let slide = Project { video: vec![p.video[0].clone(), VideoItem { effect: "slideleft".into(), ..p.video[1].clone() }], ..p.clone() };
    assert!(slide.pieces_graph((1_250, 1_750), Path::new("s.mp4"), true, Path::new("."), "anull").unwrap().graph.contains("transition=slideleft"));
    assert!(Project { video: vec![p.video[0].clone(), VideoItem { effect: "boom".into(), ..p.video[1].clone() }], ..p.clone() }.validate(10_000).is_err());
    // The whole edit: a alone, the overlap, b alone.
    let g = p.pieces_graph((0, 4_000), Path::new("s.mp4"), true, Path::new("."), "anull").unwrap();
    assert_eq!(g.reads, [(1_000, 1_000), (2_000, 1_000), (4_000, 1_000), (5_000, 2_000)]);
    assert!(g.graph.contains("concat=n=3:v=1:a=1"), "{}", g.graph);

    // Dissolves in and out of one piece must fit inside it (b is now 2 s).
    p.video[1].src_out_ms = 6_000;
    p.video.push(VideoItem { dissolve: true, transition_ms: 1_500, ..item("c", 7_000, 9_000) });
    assert!(p.validate(10_000).is_err());
    p.video[2].transition_ms = 1_000;
    p.validate(10_000).unwrap();
}

#[test]
fn a_dissolve_renders_the_same_in_the_preview() {
    let Some((ffmpeg, ffprobe)) = ffmpeg_tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let source = source_video(&ffmpeg, dir.path());
    let r = real_renderer(&ffmpeg, &ffprobe);
    let cancel = CancellationToken::new();
    let mut p = project(vec![item("a", 1_000, 3_000), VideoItem { dissolve: true, transition_ms: 1_000, ..item("b", 5_000, 8_000) }]);
    (p.captions, p.title) = (false, None);
    let render = |p: &Project, name: &str, range: Option<(i64, i64)>| {
        let out = dir.path().join(name);
        let job = ProjectRender { project: p, source: &source, output_path: &out, range, scale_to: None, loudness_filter: None, media_dir: dir.path(), progress: None };
        block_on(r.render_project(&job, &cancel)).unwrap();
        out
    };
    let export = render(&p, "export.mp4", None);
    let part = render(&p, "part.mp4", Some((1_300, 2_300)));
    // A wipe too: its preview matches mid-way.
    let wipe = Project { video: vec![p.video[0].clone(), VideoItem { effect: "wipeleft".into(), ..p.video[1].clone() }], ..p.clone() };
    let (wiped, wiped_part) = (render(&wipe, "wipe.mp4", None), render(&wipe, "wipe_part.mp4", Some((1_300, 2_300))));
    let diff = mean_diff(&frame(&ffmpeg, &wiped, 1.5), &frame(&ffmpeg, &wiped_part, 0.2));
    assert!(diff < 4.0, "wipe preview differs: {diff}");
    assert!(mean_diff(&frame(&ffmpeg, &wiped, 1.5), &frame(&ffmpeg, &export, 1.5)) > 3.0, "a wipe looks like a dissolve");
    for t in [0.05, 0.4, 0.8] {
        let diff = mean_diff(&frame(&ffmpeg, &export, 1.3 + t), &frame(&ffmpeg, &part, t));
        assert!(diff < 4.0, "preview differs from the export at +{t}s: {diff}");
    }
    // Half way through the dissolve, the frame is between a's and b's.
    let cut = project(vec![item("a", 1_000, 3_000), item("b", 5_000, 8_000)]);
    let cut = render(&Project { captions: false, title: None, ..cut }, "cut.mp4", None);
    let (mid, a, b) = (frame(&ffmpeg, &export, 1.5), frame(&ffmpeg, &cut, 1.5), frame(&ffmpeg, &cut, 2.5));
    let (from_a, from_b) = (mean_diff(&mid, &a), mean_diff(&mid, &b));
    assert!(from_a > 3.0 && from_b > 3.0 && mean_diff(&a, &b) > from_a.max(from_b), "{from_a} {from_b}");
}

#[test]
fn a_range_after_speech_keeps_the_exports_ducking() {
    let Some((ffmpeg, ffprobe)) = ffmpeg_tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let source = source_video(&ffmpeg, dir.path());
    let status = Command::new(&ffmpeg)
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=1000:duration=8", "-c:a", "aac"])
        .arg(dir.path().join("audio_1.m4a"))
        .status()
        .unwrap();
    assert!(status.success());
    let r = real_renderer(&ffmpeg, &ffprobe);
    let cancel = CancellationToken::new();
    // Speech until 2 s, then silence; ducked music throughout. The range starts as the
    // speech stops, while the export's music is still coming back up.
    let mut p = project(vec![item("a", 1_000, 3_000), VideoItem { volume: 0.0, ..item("b", 4_000, 7_000) }]);
    (p.captions, p.title) = (false, None);
    p.audio = vec![sound("m", Some("audio_1.m4a"), (0, 5_000), 0, true)];
    let render = |name: &str, range: Option<(i64, i64)>| {
        let out = dir.path().join(name);
        let job = ProjectRender { project: &p, source: &source, output_path: &out, range, scale_to: None, loudness_filter: Some("anull".into()), media_dir: dir.path(), progress: None };
        block_on(r.render_project(&job, &cancel)).unwrap();
        out
    };
    let export = render("export.mp4", None);
    let part = render("part.mp4", Some((2_000, 4_000)));
    let (whole, range) = (rms(&ffmpeg, &export, 2.0, 0.15), rms(&ffmpeg, &part, 0.0, 0.15));
    assert!((whole - range).abs() < whole * 0.1, "ducking restarts at the range: {range} vs {whole}");
}

fn media(file: &str, width: u32, height: u32, duration_ms: i64) -> MediaFile {
    MediaFile { file: file.into(), name: file.into(), width, height, duration_ms, image: false }
}

#[test]
fn added_videos_play_on_the_main_track_and_over_it() {
    let mut p = project(vec![item("a", 1_000, 3_000), VideoItem { media: Some("media_1.mp4".into()), framing: FRAMING_CROP.into(), crop: Some(Crop { x: 0.5, y: 0.5, zoom: 1.0 }), ..item("b", 0, 2_000) }]);
    p.media = vec![media("media_1.mp4", 640, 360, 5_000)];
    p.pip = vec![PipItem { id: "p0".into(), media: "media_1.mp4".into(), src_in_ms: 1_000, src_out_ms: 3_000, start_ms: 500, x: 0.7, y: 0.3, width: 0.4, volume: 0.5 }];
    p.validate(10_000).unwrap();
    // The added video's words aren't the source's: none of its time is captioned.
    let (_, words) = p.timeline();
    assert!(words.iter().flat_map(|s| &s.words).all(|w| w.start_time_ms < 2_000), "{words:?}");
    // A silent source among added videos: its piece gets silence so the track has sound.
    let g = p.pieces_graph((1_000, 3_000), Path::new("s.mp4"), false, Path::new("/run"), "anull").unwrap();
    assert_eq!(g.reads, [(2_000, 1_000), (0, 1_000)]);
    assert_eq!(g.read_paths, [None, Some(PathBuf::from("/run/media_1.mp4"))]);
    assert!(g.graph.contains("anullsrc=r=48000:cl=stereo,atrim=end_sample=48000") && g.graph.contains("concat=n=2:v=1:a=1"), "{}", g.graph);
    // Cropped with the added video's own size (640x360 -> a 9:16 window of 360 px height).
    assert!(g.graph.contains("[1:v]crop=202:360:219:0"), "{}", g.graph);
    // Picture-in-picture: read from where the range meets it, placed and timed on the timeline.
    assert_eq!(g.audio_reads, [(PathBuf::from("/run/media_1.mp4"), 1_500, 1_500)]);
    assert!(g.graph.contains("[2:v]scale=72:-2,setsar=1,setpts=PTS-STARTPTS+1.000000/TB[pv0]"), "{}", g.graph);
    assert!(g.graph.contains("[main0][pv0]overlay=x=126-w/2:y=96-h/2:eof_action=pass[base]"), "{}", g.graph);
    assert!(g.inputs(Path::new("s.mp4"))[1].0 == Path::new("/run/media_1.mp4"));

    let bad = |f: &dyn Fn(&mut Project)| {
        let mut q = p.clone();
        f(&mut q);
        q.validate(10_000).is_err()
    };
    assert!(bad(&|q| q.media.clear()), "a piece of an unknown video");
    assert!(bad(&|q| q.video[1].src_out_ms = 6_000), "past the end of the added video");
    assert!(bad(&|q| q.pip[0].width = 2.0));
    assert!(bad(&|q| q.media[0].file = "../x.mp4".into()));
}

#[test]
fn added_videos_and_images_render() {
    let Some((ffmpeg, ffprobe)) = ffmpeg_tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let source = source_video(&ffmpeg, dir.path());
    let r = real_renderer(&ffmpeg, &ffprobe);
    let cancel = CancellationToken::new();
    // A landscape video without sound, and a red picture.
    let clip = dir.path().join("broll.mov");
    let png = dir.path().join("red.png");
    for (args, out) in [
        (vec!["-f", "lavfi", "-i", "color=blue:size=320x180:rate=25:duration=3"], &clip),
        (vec!["-f", "lavfi", "-i", "color=red:size=200x100", "-frames:v", "1"], &png),
    ] {
        assert!(Command::new(&ffmpeg).args(["-v", "error", "-y"]).args(args).arg(out).status().unwrap().success());
    }
    let (w, h, len, image) = block_on(r.import_media(&clip, &dir.path().join("media_1.mp4"), "30", &cancel)).unwrap();
    assert!((w, h, image) == (320, 180, false) && (len - 3_000).abs() < 100, "{w}x{h} {len}");
    let (_, _, still, image) = block_on(r.import_media(&png, &dir.path().join("media_2.mp4"), "30", &cancel)).unwrap();
    assert!(image && (still - 60_000).abs() < 100, "{still}");
    assert!(block_on(r.import_media(&dir.path().join("x.txt"), &dir.path().join("m.mp4"), "30", &cancel)).is_err());

    // Source, then the picture filling the frame, with the blue video small over both.
    let mut p = project(vec![
        item("a", 1_000, 3_000),
        VideoItem { media: Some("media_2.mp4".into()), framing: FRAMING_CROP.into(), crop: Some(Crop { x: 0.5, y: 0.5, zoom: 1.0 }), ..item("b", 0, 2_000) },
    ]);
    (p.captions, p.title) = (false, None);
    p.title = Some("Title".into());
    p.media = vec![media("media_1.mp4", w, h, len), MediaFile { image: true, ..media("media_2.mp4", 200, 100, still) }];
    p.pip = vec![PipItem { id: "p0".into(), media: "media_1.mp4".into(), src_in_ms: 0, src_out_ms: 2_000, start_ms: 1_000, x: 0.5, y: 0.45, width: 0.5, volume: 1.0 }];
    p.validate(10_000).unwrap();
    let render = |name: &str, range: Option<(i64, i64)>| {
        let out = dir.path().join(name);
        let job = ProjectRender { project: &p, source: &source, output_path: &out, range, scale_to: None, loudness_filter: None, media_dir: dir.path(), progress: None };
        block_on(r.render_project(&job, &cancel)).unwrap();
        out
    };
    let export = render("export.mp4", None);
    let part = render("part.mp4", Some((1_500, 3_000)));
    let seconds = |path: &Path| {
        let out = Command::new(&ffprobe).args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"]).arg(path).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap()
    };
    // The picture-in-picture ends at 3 s; the clip goes on to 4 s.
    assert!((seconds(&export) - 4.0).abs() < 0.1, "{}", seconds(&export));
    // Colour at (x, y) of the 90x160 grey frame: the picture is red (grey ~76), the PiP blue (~29).
    let at = |f: &[u8], x: usize, y: usize| f[y * 90 + x] as i32;
    let late = frame(&ffmpeg, &export, 2.5);
    assert!((at(&late, 45, 130) - 76).abs() < 12, "picture: {}", at(&late, 45, 130));
    assert!((at(&late, 45, 72) - 29).abs() < 12, "pip: {}", at(&late, 45, 72));
    let early = frame(&ffmpeg, &export, 0.5);
    assert!((at(&early, 45, 72) - 29).abs() > 12, "pip shown before its start");
    for t in [0.2, 0.8, 1.3] {
        let diff = mean_diff(&frame(&ffmpeg, &export, 1.5 + t), &frame(&ffmpeg, &part, t));
        assert!(diff < 4.0, "preview differs from the export at +{t}s: {diff}");
    }
}

#[test]
fn keyframes_zoom_and_pan_the_crop() {
    let key = |t_ms: i64, x: f64, zoom: f64| CropKey { t_ms, x, y: 0.5, zoom };
    let mut p = project(vec![VideoItem { framing: FRAMING_CROP.into(), keyframes: vec![key(0, 0.5, 1.0), key(2_000, 0.3, 2.0)], ..item("a", 1_000, 3_000) }]);
    p.validate(10_000).unwrap();
    let g = p.pieces_graph((500, 2_000), Path::new("s.mp4"), false, Path::new("."), "anull").unwrap();
    assert!(g.graph.contains("scale=w='max(180,") && g.graph.contains(":eval=frame") && g.graph.contains("crop=w=180:h=320:x='clip("), "{}", g.graph);
    // The read starts half a second into the piece: its clock is offset to match.
    assert!(g.graph.contains("(0.500000+t/1)"), "{}", g.graph);
    p.video[0].keyframes.reverse();
    assert!(p.validate(10_000).is_err(), "keyframes out of order");
    p.video[0].keyframes = vec![key(0, 0.5, 1.0), key(5_000, 0.5, 1.0)];
    assert!(p.validate(10_000).is_err(), "a keyframe past the piece");

    let Some((ffmpeg, ffprobe)) = ffmpeg_tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let source = source_video(&ffmpeg, dir.path());
    let r = real_renderer(&ffmpeg, &ffprobe);
    let cancel = CancellationToken::new();
    (p.captions, p.title) = (false, None);
    let render = |p: &Project, name: &str, range: Option<(i64, i64)>| {
        let out = dir.path().join(name);
        let job = ProjectRender { project: p, source: &source, output_path: &out, range, scale_to: None, loudness_filter: None, media_dir: dir.path(), progress: None };
        block_on(r.render_project(&job, &cancel)).unwrap();
        out
    };
    p.video[0].keyframes = vec![key(0, 0.5, 1.0), key(2_000, 0.3, 2.0)];
    let moving = render(&p, "moving.mp4", None);
    let part = render(&p, "part.mp4", Some((600, 1_600)));
    for t in [0.1, 0.5, 0.9] {
        let diff = mean_diff(&frame(&ffmpeg, &moving, 0.6 + t), &frame(&ffmpeg, &part, t));
        assert!(diff < 4.0, "preview differs from the export at +{t}s: {diff}");
    }
    // It starts as the zoom-1 crop and ends as the zoom-2 one.
    let still = |x: f64, zoom: f64, name: &str| {
        let q = Project { video: vec![VideoItem { keyframes: vec![], crop: Some(Crop { x, y: 0.5, zoom }), ..p.video[0].clone() }], ..p.clone() };
        render(&q, name, None)
    };
    let (wide, close) = (still(0.5, 1.0, "wide.mp4"), still(0.3, 2.0, "close.mp4"));
    let start = mean_diff(&frame(&ffmpeg, &moving, 0.0), &frame(&ffmpeg, &wide, 0.0));
    let end = mean_diff(&frame(&ffmpeg, &moving, 1.95), &frame(&ffmpeg, &close, 1.95));
    let apart = mean_diff(&frame(&ffmpeg, &wide, 1.95), &frame(&ffmpeg, &close, 1.95));
    assert!(start < 6.0 && end < 6.0 && apart > start.max(end) * 2.0, "start {start} end {end} apart {apart}");
}

#[test]
fn colour_looks_change_the_picture() {
    let look = |brightness: f64, contrast: f64, saturation: f64, temperature: f64| Some(ColorAdjust { brightness, contrast, saturation, temperature });
    let mut p = project(vec![VideoItem { color: look(0.1, 1.2, 0.0, 5000.0), ..item("a", 1_000, 3_000) }]);
    p.validate(10_000).unwrap();
    let g = p.pieces_graph((0, 2_000), Path::new("s.mp4"), false, Path::new("."), "anull").unwrap();
    assert!(g.graph.contains("[p0_base]lutyuv=y='clip((val-128)*1.2000+128+25.5000,0,255)',hue=s=0.0000,colortemperature=temperature=5000[p0_v]"), "{}", g.graph);
    // As shot: no filters at all.
    p.video[0].color = look(0.0, 1.0, 1.0, 6500.0);
    assert!(p.pieces_graph((0, 2_000), Path::new("s.mp4"), false, Path::new("."), "anull").unwrap().graph.contains("concat=n=1:v=1:a=0[vcat]"));
    assert!(!p.pieces_graph((0, 2_000), Path::new("s.mp4"), false, Path::new("."), "anull").unwrap().graph.contains("[p0_v]"));
    p.video[0].color = look(0.0, 1.0, 5.0, 6500.0);
    assert!(p.validate(10_000).is_err());

    let Some((ffmpeg, ffprobe)) = ffmpeg_tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let source = source_video(&ffmpeg, dir.path());
    let r = real_renderer(&ffmpeg, &ffprobe);
    let cancel = CancellationToken::new();
    (p.captions, p.title) = (false, None);
    let render = |color: Option<ColorAdjust>, name: &str| {
        let q = Project { video: vec![VideoItem { color, ..p.video[0].clone() }], ..p.clone() };
        let out = dir.path().join(name);
        let job = ProjectRender { project: &q, source: &source, output_path: &out, range: None, scale_to: None, loudness_filter: None, media_dir: dir.path(), progress: None };
        block_on(r.render_project(&job, &cancel)).unwrap();
        out
    };
    // Mean |R-G| + |G-B| of a frame: 0 for black and white.
    let colourful = |path: &Path| {
        let out = Command::new(&ffmpeg).args(["-v", "error", "-ss", "1", "-i"]).arg(path).args(["-frames:v", "1", "-vf", "scale=90:160", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"]).output().unwrap();
        out.stdout.chunks_exact(3).map(|c| (c[0] as f64 - c[1] as f64).abs() + (c[1] as f64 - c[2] as f64).abs()).sum::<f64>() / (90.0 * 160.0)
    };
    let brightness = |path: &Path| frame(&ffmpeg, path, 1.0).iter().map(|&v| v as f64).sum::<f64>() / (90.0 * 160.0);
    let plain = render(None, "plain.mp4");
    let grey = render(look(0.0, 1.0, 0.0, 6500.0), "grey.mp4");
    let bright = render(look(0.2, 1.0, 1.0, 6500.0), "bright.mp4");
    assert!(colourful(&plain) > 40.0 && colourful(&grey) < 6.0, "{} vs {}", colourful(&plain), colourful(&grey));
    assert!(brightness(&bright) > brightness(&plain) + 25.0, "{} vs {}", brightness(&bright), brightness(&plain));
}

#[test]
fn text_layers_animate_in_and_out() {
    let events = |animation_in: &str, animation_out: &str| {
        let p = Project { texts: vec![TextLayer { animation_in: animation_in.into(), animation_out: animation_out.into(), ..text_layer(1_000, 3_000) }], ..project(vec![item("a", 1_000, 3_000)]) };
        p.validate(10_000).unwrap();
        let doc = p.with_text_layers(None).unwrap();
        doc.lines().filter(|l| l.starts_with("Dialogue:")).map(String::from).collect::<Vec<_>>()
    };
    let plain = events("none", "none");
    assert!(plain.iter().all(|l| l.contains("\\pos(90,80)") && !l.contains("\\fad")), "{plain:?}");
    let faded = events("fade", "fade");
    assert!(faded.iter().all(|l| l.contains("\\fad(300,250)")), "{faded:?}");
    let slid = events("slide", "pop");
    assert!(slid.iter().all(|l| l.contains("\\move(90,120,90,80,0,300)") && l.contains("\\t(1750,2000,\\fscx30\\fscy30)")), "{slid:?}");
    // Typewriter: "BIG NEWS / HERE" is 3 words, so 3 steps of 400 ms; hidden words are transparent.
    let typed = events("typewriter", "none");
    let face: Vec<_> = typed.iter().filter(|l| l.starts_with(&typed.last().unwrap()[..11])).collect();
    assert_eq!(face.len(), 3, "{typed:?}");
    assert!(face[0].contains("0:00:01.00,0:00:01.40") && face[0].contains("BIG {\\alpha&HFF&}NEWS"), "{}", face[0]);
    assert!(face[2].contains("0:00:01.80,0:00:03.00") && !face[2].contains("\\alpha&HFF&}"), "{}", face[2]);

    let bad = Project { texts: vec![TextLayer { animation_in: "spin".into(), ..text_layer(0, 1_000) }], ..project(vec![item("a", 1_000, 3_000)]) };
    assert!(bad.validate(10_000).is_err());

    let Some((ffmpeg, ffprobe)) = ffmpeg_tools() else { return };
    if !has_libass(&ffmpeg) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = source_video(&ffmpeg, dir.path());
    let r = real_renderer(&ffmpeg, &ffprobe);
    let cancel = CancellationToken::new();
    let render = |texts: Vec<TextLayer>, name: &str| {
        let mut p = project(vec![item("a", 1_000, 4_000)]);
        (p.captions, p.title, p.texts) = (false, None, texts);
        let out = dir.path().join(name);
        let job = ProjectRender { project: &p, source: &source, output_path: &out, range: None, scale_to: None, loudness_filter: None, media_dir: dir.path(), progress: None };
        block_on(r.render_project(&job, &cancel)).unwrap();
        out
    };
    let plain = render(vec![], "plain.mp4");
    let typed = render(vec![TextLayer { text: "one two three four".into(), size: 30, animation_in: "typewriter".into(), ..text_layer(0, 3_000) }], "typed.mp4");
    // Early on only the first word is typed: much less text than later.
    let early = mean_diff(&frame(&ffmpeg, &plain, 0.15), &frame(&ffmpeg, &typed, 0.15));
    let late = mean_diff(&frame(&ffmpeg, &plain, 2.0), &frame(&ffmpeg, &typed, 2.0));
    assert!(early > 0.1 && late > early * 2.0, "early {early} late {late}");
}

// ---- split framing and backdrop ---------------------------------------------------

fn face(x: f64) -> LBox {
    LBox { x, y: 0.3, w: 0.08, h: 0.16 }
}

/// Three people on camera 1-5 s, the middle one only half the time, and a face cut
/// by the left edge throughout.
fn panel_project(framing: &str) -> Project {
    let mut p = project(vec![VideoItem { framing: framing.into(), ..item("a", 1_000, 5_000) }]);
    p.face_samples = (4..20)
        .map(|k| {
            let mut faces = vec![face(0.1), face(0.8), LBox { x: 0.0, y: 0.3, w: 0.04, h: 0.16 }];
            if k % 2 == 0 {
                faces.push(face(0.45));
            }
            (k * 250, faces)
        })
        .collect();
    p
}

#[test]
fn split_framing_stacks_the_pieces_people() {
    let people = |p: &Project| p.timeline().0.shots.iter().map(|s| (s.layout.clone(), s.people.iter().map(|b| (b.cx() * 100.0).round()).collect::<Vec<_>>())).collect::<Vec<_>>();
    // Three panels: everyone whole, left to right; the edge-cut face is left out.
    assert_eq!(people(&panel_project(FRAMING_SPLIT3)), [(STACK.to_string(), vec![14.0, 49.0, 84.0])]);
    // Two panels: the two on screen longest.
    assert_eq!(people(&panel_project(FRAMING_SPLIT)), [(STACK.to_string(), vec![14.0, 84.0])]);
    // Each camera shot gets its own people: after a cut at 3 s, one person only.
    let mut cut = panel_project(FRAMING_SPLIT);
    cut.shots = vec![shot(1_000, 3_000, TALKING_HEAD), shot(3_000, 9_000, TALKING_HEAD)];
    for (t, faces) in &mut cut.face_samples {
        if *t >= 3_000 {
            *faces = vec![face(0.5)];
        }
    }
    assert_eq!(people(&cut), [(STACK.to_string(), vec![14.0, 84.0]), (SCREEN.to_string(), vec![])]);
    // Nobody to split: the whole frame.
    let mut empty = panel_project(FRAMING_SPLIT);
    empty.face_samples.clear();
    assert_eq!(people(&empty), [(SCREEN.to_string(), vec![])]);
}

#[test]
fn split_and_backdrop_are_validated() {
    assert!(panel_project(FRAMING_SPLIT3).validate(10_000).is_ok());
    let mut added = panel_project(FRAMING_SPLIT);
    added.media = vec![MediaFile { file: "media_1.mp4".into(), name: "clip.mp4".into(), duration_ms: 5_000, width: 320, height: 240, image: false }];
    added.video[0].media = Some("media_1.mp4".into());
    assert!(added.validate(10_000).is_err(), "an added video has no faces to split");
    let mut colour = panel_project(FRAMING_FIT);
    colour.backdrop = Some("#101820".into());
    assert!(colour.validate(10_000).is_ok());
    colour.backdrop = Some("#10182g".into());
    assert!(colour.validate(10_000).is_err());
}

#[test]
fn stacked_panels_fill_the_frame_and_captions_sit_on_a_seam() {
    let shot = ShotLayout { layout: STACK.into(), people: vec![face(0.8), face(0.1), face(0.45)], ..shot(0, 1_000, STACK) };
    let panels = stack_panels(&shot, 1920, 1080, 1080, 1920);
    assert_eq!(panels.iter().map(|p| p.1).collect::<Vec<_>>(), [(0, 640), (640, 640), (1280, 640)]);
    // Left to right, top to bottom, each crop within the half-way points to its neighbours.
    let (lo, hi) = ((0.14 + 0.49) / 2.0 * 1920.0, (0.49 + 0.84) / 2.0 * 1920.0);
    let middle = panels[1].0;
    assert!(middle.2 as f64 >= lo - 1.0 && (middle.2 + middle.0) as f64 <= hi + 1.0, "{middle:?}");
    assert_eq!(caption_anchor(&shot, 1920, 1080, 1080, 1920), (5, 1280));
}

#[test]
fn split_and_a_solid_backdrop_render() {
    let Some((ffmpeg, ffprobe)) = ffmpeg_tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let source = source_video(&ffmpeg, dir.path());
    let r = real_renderer(&ffmpeg, &ffprobe);
    let mut p = panel_project(FRAMING_SPLIT3);
    p.video.push(VideoItem { framing: FRAMING_FIT.into(), ..item("b", 5_000, 7_000) });
    (p.captions, p.title, p.backdrop) = (false, None, Some("#ff0000".into()));
    let graph = p.pieces_graph((0, 6_000), &source, false, dir.path(), "anull").unwrap().graph;
    assert!(graph.contains("vstack=inputs=3") && graph.contains("color=0xff0000"), "{graph}");
    let out = dir.path().join("out.mp4");
    let job = ProjectRender { project: &p, source: &source, output_path: &out, range: None, scale_to: None, loudness_filter: None, media_dir: dir.path(), progress: None };
    block_on(r.render_project(&job, &CancellationToken::new())).unwrap();
    // The whole-frame piece sits on solid red (grey ~76), top and bottom.
    let img = frame(&ffmpeg, &out, 5.0);
    let band = |rows: std::ops::Range<usize>| rows.flat_map(|y| img[y * 90..y * 90 + 90].to_vec()).map(|v| v as f64).collect::<Vec<_>>();
    for rows in [0..20, 140..160] {
        let v = band(rows);
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        assert!((mean - 76.0).abs() < 8.0 && v.iter().all(|x| (x - mean).abs() < 12.0), "not a solid red band: {mean}");
    }
}
