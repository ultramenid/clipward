"""Golden vectors for the Rust render port, produced by the Python engine.

Run from the repo root with the engine's Python environment (Pillow, pydantic-settings):

    PYTHONPATH=engine python crates/engine/src/render/tests/gen_golden.py

Writes golden.json.gz next to this file. The Rust tests (render/tests/golden.rs)
replay every case and require byte-identical FFmpeg arguments, filter graphs,
ASS and SRT text.
"""

import asyncio
import logging
import copy
import gzip
import dataclasses
import json
import os
import random
import shutil
import sys
import tempfile
from types import SimpleNamespace

from clip_engine.config import CaptionStyle, get_caption_preset, get_settings
from clip_engine.services import rendering_service as rs_module
from clip_engine.services.caption_generator import CaptionGeneratorService
from clip_engine.services.clip_editor import (
    TimeMap,
    WindowWord,
    compute_keep_intervals,
    reaction_intervals,
    remap_plan,
    remap_segments,
    subtract_intervals,
    window_words,
)
from clip_engine.services.layout_analyzer import Box, ClipLayoutPlan, LayoutType, ShotLayout
from clip_engine.services.layout_renderer import (
    CaptionPlacer,
    FaceZone,
    banner_y,
    build_layout_graph,
    caption_anchor,
    face_rects,
    face_zones,
    measured_loudness_filter,
    piecewise_expr,
    shot_chain,
    title_y,
)
from clip_engine.services.rendering_service import RenderingService, RenderRequest
from clip_engine.services.transcription_service import TranscriptSegment, TranscriptWord
from clip_engine.services.video_speed import scaled_duration_ms, speed_audio_filter, speed_video_filter

logging.disable(logging.CRITICAL)
rng = random.Random(20260925)
PRESETS = ["pop", "spotlight", "impact", "glow", "boxed", "sweep", "editorial", "hype", "punch", "neon",
           "headline", "paper", "subtle"]
LAYOUTS = [LayoutType.TALKING_HEAD, LayoutType.TWO_SHOT, LayoutType.SCREEN_CAM, LayoutType.SCREEN]


# ---------------------------------------------------------------- serializers

def box_j(b):
    return None if b is None else {"x": b.x, "y": b.y, "w": b.w, "h": b.h}


def shot_j(s):
    return {
        "start_ms": s.start_ms, "end_ms": s.end_ms, "layout": s.layout, "source": s.source,
        "detected_layout": s.detected_layout,
        "focus_path": [[t, cx, cy] for t, cx, cy in s.focus_path],
        "people": [box_j(p) for p in s.people],
        "screen_box": box_j(s.screen_box), "screen_focus": box_j(s.screen_focus),
        "cam_box": box_j(s.cam_box), "cam_face": box_j(s.cam_face),
    }


def plan_j(p):
    return {
        "shots": [shot_j(s) for s in p.shots], "source_width": p.source_width, "source_height": p.source_height,
        "vision_cost_usd": p.vision_cost_usd,
        "face_samples": [[t, [box_j(b) for b in faces]] for t, faces in p.face_samples],
    }


def word_j(w):
    return {"word": w.word, "start_time_ms": w.start_time_ms, "end_time_ms": w.end_time_ms}


def seg_j(s):
    return {
        "start_time_ms": s.start_time_ms, "end_time_ms": s.end_time_ms, "text": s.text,
        "speaker_label": s.speaker_label, "words": [word_j(w) for w in s.words],
        "audio_events": list(getattr(s, "audio_events", []) or []),
    }


def style_j(st):
    return {k: getattr(st, k) for k in CaptionStyle.__annotations__}


# ---------------------------------------------------------------- generators

def rbox(lo=0.02, hi=0.5):
    w, h = rng.uniform(lo, hi), rng.uniform(lo, hi)
    return Box(round(rng.uniform(0, 1 - w), 4), round(rng.uniform(0, 1 - h), 4), round(w, 4), round(h, 4))


def rshot(start, end, layout=None):
    layout = layout or rng.choice(LAYOUTS)
    shot = ShotLayout(start, end, layout, source=rng.choice(["heuristic", "vision"]))
    if rng.random() < 0.3:
        shot.detected_layout = rng.choice(LAYOUTS)
    if layout == LayoutType.TALKING_HEAD:
        n = rng.randint(0, 4)
        shot.focus_path = sorted((rng.randint(0, max(1, end - start)), round(rng.uniform(0.1, 0.9), 3),
                                  round(rng.uniform(0.1, 0.9), 3)) for _ in range(n))
        shot.people = [rbox(0.05, 0.3)]
    elif layout == LayoutType.TWO_SHOT:
        a, b = rbox(0.05, 0.25), rbox(0.05, 0.25)
        shot.people = sorted([a, b], key=lambda p: p.cx)
        if rng.random() < 0.15:
            shot.people = shot.people[:1]
    elif layout == LayoutType.SCREEN_CAM:
        if rng.random() < 0.85:
            shot.cam_box = rbox(0.06, 0.45)
            if rng.random() < 0.7:
                c = shot.cam_box
                shot.cam_face = Box(c.x + c.w * 0.3, c.y + c.h * 0.2, c.w * 0.4, c.h * 0.5)
        if rng.random() < 0.6:
            shot.screen_box = rbox(0.5, 1.0)
        if rng.random() < 0.5:
            shot.screen_focus = rbox(0.1, 0.6)
    return shot


def rplan(window_ms, src=None, faces=False):
    src_w, src_h = src or rng.choice([(1920, 1080), (1280, 720), (1080, 1920), (1440, 1080), (3840, 2160),
                                      (1920, 1088), (640, 480), (2560, 1080)])
    n = rng.randint(1, 4)
    cuts = sorted(rng.sample(range(300, max(301, window_ms - 300)), min(n - 1, max(0, window_ms // 1000))))
    bounds = [0, *cuts, window_ms]
    shots = [rshot(a, b) for a, b in zip(bounds, bounds[1:]) if b > a]
    samples = []
    if faces:
        for t in range(0, window_ms, 250):
            samples.append((t, [rbox(0.05, 0.4) for _ in range(rng.randint(0, 2))]))
    return ClipLayoutPlan(shots, src_w, src_h, vision_cost_usd=round(rng.uniform(0, 0.01), 4), face_samples=samples)


FILLERS = ["um", "uh,", "Umm...", "hmm", "er"]
VOCAB = ["we", "made", "$14,500.", "this", "is", "huge!", "really?", "look", "at", "that,", "business.",
         "okay", "{bad}", "so", "like", "incredible", "opportunities", "Next", "phrase", "here.", "café", "straße"]


def rwords(start=0, count=None, pause_p=0.2):
    count = count if count is not None else rng.randint(1, 30)
    t = start + rng.randint(0, 1500)
    words = []
    for _ in range(count):
        dur = rng.randint(80, 700)
        text = rng.choice(FILLERS) if rng.random() < 0.12 else rng.choice(VOCAB)
        words.append(TranscriptWord(text, t, t + dur))
        t += dur + (rng.randint(700, 4000) if rng.random() < pause_p else rng.randint(0, 150))
    return words


def rsegments(start=0, n=None, max_words=30):
    segs = []
    t = start
    for _ in range(n if n is not None else rng.randint(1, 4)):
        words = rwords(t, rng.randint(1, max_words))
        seg = TranscriptSegment(words[0].start_time_ms, words[-1].end_time_ms, " ".join(w.word for w in words),
                                words=words)
        if rng.random() < 0.2:
            seg.audio_events = ["(laughter)"]
        if rng.random() < 0.1:
            seg = TranscriptSegment(seg.start_time_ms, seg.end_time_ms, seg.text)  # segment-level
        segs.append(seg)
        t = words[-1].end_time_ms + rng.randint(0, 3000)
    return segs


def rkeeps(window_ms):
    if rng.random() < 0.3:
        return None
    points = sorted(set(rng.randint(0, window_ms) for _ in range(rng.randint(2, 12))))
    keeps = [(a, b) for a, b in zip(points[::2], points[1::2]) if b > a]
    return keeps or None


out = {}

# ---------------------------------------------------------------- video speed
out["speed"] = []
for speed in [1, 1.1, 1.25, 1.5, 1.75, 2, 1.3333333333, 1.05]:
    for dur in [0, 1, 5, 999, 11000, 123457]:
        out["speed"].append({
            "speed": speed, "duration_ms": dur, "scaled": scaled_duration_ms(dur, speed),
            "video": speed_video_filter(speed, "30000/1001"), "audio": speed_audio_filter(speed, dur),
        })

# ---------------------------------------------------------------- clip editor
out["keeps"] = []
for _ in range(200):
    window_ms = rng.randint(2000, 60000)
    words = [WindowWord(w.start_time_ms, min(w.end_time_ms, window_ms), w.word)
             for w in rwords(0, pause_p=rng.choice([0.1, 0.4])) if w.start_time_ms < window_ms]
    plan = None if rng.random() < 0.3 else rplan(window_ms)
    protected = []
    if rng.random() < 0.3:
        a = rng.randint(0, window_ms)
        protected = [(a, min(window_ms, a + rng.randint(100, 5000)))]
    longform = rng.random() < 0.3
    keeps = compute_keep_intervals(words, window_ms, plan, protected, longform=longform)
    skips = []
    for _ in range(rng.randint(0, 3)):
        a = rng.randint(-1000, window_ms)
        skips.append((a, a + rng.randint(0, 8000)))
    sub = subtract_intervals(keeps, skips)
    tm = TimeMap(sub, window_ms)
    probes = [rng.randint(-100, window_ms + 100) for _ in range(8)]
    out["keeps"].append({
        "words": [[w.start_ms, w.end_ms, w.text] for w in words], "window_ms": window_ms,
        "plan": plan_j(plan) if plan else None, "protected": protected, "longform": longform,
        "keeps": keeps, "skips": skips, "subtracted": sub,
        "removed_ms": tm.removed_ms, "cut_count": tm.cut_count, "output_ms": tm.output_ms,
        "probes": [[p, tm.to_output(p), tm.to_output_clamped(p), tm.pieces_within(p, p + 1500)] for p in probes],
    })

out["remap"] = []
for _ in range(60):
    window_start = rng.randint(0, 20000)
    segs = rsegments(window_start + rng.randint(-2000, 2000))
    window_ms = rng.randint(3000, 40000)
    keeps = rkeeps(window_ms) or [(0, window_ms)]
    tm = TimeMap(keeps, window_ms if rng.random() < 0.5 else None)
    plan = rplan(window_ms)
    out["remap"].append({
        "window_start_ms": window_start, "window_ms": window_ms, "segments": [seg_j(s) for s in segs],
        "keeps": keeps, "time_map_window": tm.window_ms,
        "remapped": [seg_j(s) for s in remap_segments(segs, window_start, tm)],
        "window_words": [[w.start_ms, w.end_ms, w.text] for w in window_words(segs, window_start, window_ms)],
        "reactions": reaction_intervals(segs, window_start, window_ms),
        "plan": plan_j(plan), "remapped_plan": plan_j(remap_plan(plan, tm)),
    })

# ---------------------------------------------------------------- layout renderer
out["chains"] = []
for _ in range(300):
    window_ms = rng.randint(1000, 20000)
    plan = rplan(window_ms)
    shot = rng.choice(plan.shots)
    out_w, out_h = rng.choice([(1080, 1920), (720, 1280), (1920, 1080), (180, 320), (2560, 1440)])
    landscape = out_w > out_h
    t_ms = rng.randint(shot.start_ms, shot.end_ms)
    faces = [rbox(0.03, 0.4) for _ in range(rng.randint(0, 3))]
    try:
        rects = face_rects(shot, faces, t_ms, plan.source_width, plan.source_height, out_w, out_h)
    except ZeroDivisionError:
        rects = "error"
    out["chains"].append({
        "shot": shot_j(shot), "src": [plan.source_width, plan.source_height], "out": [out_w, out_h],
        "landscape": landscape, "chain": shot_chain(3, shot, plan.source_width, plan.source_height, out_w, out_h,
                                                    landscape),
        "anchor": list(caption_anchor(shot, plan.source_width, plan.source_height, out_w, out_h)),
        "title_y": [title_y(shot, plan.source_width, plan.source_height, out_w, out_h, h) for h in (60, 125, 400)],
        "banner_y": banner_y(shot, plan.source_width, plan.source_height, out_w, out_h),
        "t_ms": t_ms, "faces": [box_j(f) for f in faces], "rects": rects,
    })

out["graphs"] = []
for _ in range(120):
    window_ms = rng.randint(500, 30000)
    plan = rplan(window_ms)
    keeps = rkeeps(window_ms)
    fps = rng.choice(["30", "24", "25", "30000/1001", "60", "60000/1001", "24000/1001"])
    out_w, out_h = rng.choice([(1080, 1920), (1920, 1080), (3840, 2160)])
    with_audio = rng.random() < 0.7
    loud = None
    if rng.random() < 0.3:
        loud = measured_loudness_filter({"input_i": str(rng.uniform(-40, -5)), "input_tp": str(rng.uniform(-10, 0)),
                                         "input_lra": str(rng.uniform(0, 20)),
                                         "input_thresh": str(rng.uniform(-50, -10)),
                                         "target_offset": str(rng.uniform(-2, 2))})
    speed = rng.choice([1, 1, 1.25, 1.5, 2, 1.1])
    try:
        graph = build_layout_graph(plan, out_w, out_h, keeps, with_audio, landscape=out_w > out_h, fps=fps,
                                   loudness_filter=loud, video_speed=speed)
    except ValueError as e:
        graph = "error: " + str(e)
    out["graphs"].append({
        "plan": plan_j(plan), "keeps": keeps, "fps": fps, "out": [out_w, out_h], "with_audio": with_audio,
        "loudness": loud, "speed": speed, "graph": graph,
    })

out["loudness"] = []
for measured in [
    {"input_i": "-23.54", "input_tp": "-5.2", "input_lra": "6.1", "input_thresh": "-34", "target_offset": "0.4"},
    {"input_i": "-inf", "input_tp": "-5.2", "input_lra": "6.1", "input_thresh": "-34", "target_offset": "0.4"},
    {"input_i": "-71", "input_tp": "-5.2", "input_lra": "6.1", "input_thresh": "-34", "target_offset": "0.4"},
    {"input_i": -20, "input_tp": -1, "input_lra": 6, "input_thresh": -30, "target_offset": 0},
    {"input_i": "-20.005", "input_tp": "nan", "input_lra": "6", "input_thresh": "-30", "target_offset": "0"},
    {"input_i": "-20.125", "input_tp": "-1.005", "input_lra": "6", "input_thresh": "-30"},
    {"input_i": "x", "input_tp": "-1", "input_lra": "6", "input_thresh": "-30", "target_offset": "0"},
]:
    out["loudness"].append({"measured": measured, "filter": measured_loudness_filter(measured)})

out["zones"] = []
for _ in range(60):
    window_ms = rng.randint(2000, 15000)
    plan = rplan(window_ms, faces=True)
    keeps = rkeeps(window_ms) or [(0, window_ms)]
    tm = TimeMap(keeps, window_ms)
    zones = face_zones(plan, tm, 1080, 1920)
    anchors = [(s.end_ms, *caption_anchor(s, plan.source_width, plan.source_height, 1080, 1920))
               for s in remap_plan(plan, tm).shots]
    anchors[-1] = (10 ** 9, anchors[-1][1], anchors[-1][2])
    placer = CaptionPlacer(anchors, zones, 1080, 1920)
    calls = []
    t = 0
    for _ in range(rng.randint(1, 8)):
        start = t + rng.randint(0, 1500)
        end = start + rng.randint(200, 3000)
        w, h = rng.randint(100, 1100), rng.randint(60, 400)
        calls.append([start, end, w, h, list(placer(start, end, w, h))])
        t = end
    out["zones"].append({
        "plan": plan_j(plan), "keeps": keeps, "window_ms": window_ms,
        "zones": [[z.start_ms, z.end_ms, [list(r) for r in z.rects]] for z in zones],
        "anchors": anchors, "calls": calls,
    })

out["piecewise"] = []
for _ in range(50):
    pts = [(round(rng.uniform(0, 20), 4), round(rng.uniform(-50, 3000), 3)) for _ in range(rng.randint(0, 5))]
    out["piecewise"].append({"points": pts, "expr": piecewise_expr(pts)})

# ---------------------------------------------------------------- captions
gen = CaptionGeneratorService()


def ass_text(segs, clip_start, clip_end, style, width, height, anchors, emphasis):
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "c.ass")
        result = asyncio.run(gen.generate_captions(
            transcript_segments=segs, clip_start_ms=clip_start, clip_end_ms=clip_end, output_path=path,
            caption_style=style, output_width=width, output_height=height, anchors=anchors,
            emphasis_words=emphasis,
        ))
        return open(path, encoding="utf-8").read() if result else None


out["captions"] = []
for i in range(91):
    preset = PRESETS[i % len(PRESETS)]
    style = get_caption_preset(preset)
    if rng.random() < 0.3:
        style = RenderingService._landscape_caption_style(style, rng.choice([1080, 1440, 2160]))
    if rng.random() < 0.15:
        style.future_words = rng.choice(["show", "dim", "hide"])
        style.color_transition = not style.color_transition
    if rng.random() < 0.1:
        style.word_by_word_highlight = False
    if rng.random() < 0.1:
        style.position = rng.choice(["top", "center", "bottom"])
        style.alignment = rng.choice(["left", "center", "right"])
    segs = rsegments(rng.randint(0, 5000), rng.randint(1, 2), max_words=10)
    clip_start = max(0, segs[0].start_time_ms - rng.randint(-500, 2000))
    clip_end = segs[-1].end_time_ms + rng.randint(-1000, 3000)
    width, height = rng.choice([(1080, 1920), (1920, 1080), (3840, 2160)])
    anchors = None
    if rng.random() < 0.6:
        n = rng.randint(1, 3)
        anchors = sorted((rng.randint(0, 20000), rng.choice([2, 5]), rng.randint(300, 1700)) for _ in range(n))
        anchors[-1] = (10 ** 9, anchors[-1][1], anchors[-1][2])
    emphasis = rng.choice([[], ["14,500"], ["huge", "business opportunities"], ["looks"], ["a"]])
    out["captions"].append({
        "style": style_j(style), "segments": [seg_j(s) for s in segs], "clip_start_ms": clip_start,
        "clip_end_ms": clip_end, "width": width, "height": height, "anchors": anchors, "emphasis": emphasis,
        "ass": ass_text(segs, clip_start, clip_end, style, width, height, anchors, emphasis),
    })

out["presets"] = {p: style_j(get_caption_preset(p)) for p in PRESETS}

out["srt"] = []
for _ in range(40):
    segs = rsegments(rng.randint(0, 5000), rng.randint(1, 6))
    clip_start = max(0, segs[0].start_time_ms - rng.randint(-500, 2000))
    clip_end = segs[-1].end_time_ms + rng.randint(-1000, 3000)
    with tempfile.TemporaryDirectory() as d:
        path = gen.generate_srt(segs, clip_start, clip_end, os.path.join(d, "c.srt"))
        text = open(path, encoding="utf-8").read() if path else None
    out["srt"].append({"segments": [seg_j(s) for s in segs], "clip_start_ms": clip_start, "clip_end_ms": clip_end,
                       "srt": text})

out["block_sizes"] = []
for preset in PRESETS:
    for words in (["We", "made", "it"], ["INCREDIBLE", "INTERNATIONAL", "OPPORTUNITIES"], ["a"],
                  ["Look", "at", "this.", "right", "now,", "okay?"], []):
        style = get_caption_preset(preset)
        out["block_sizes"].append({"preset": preset, "words": words, "width": 1080,
                                   "size": list(gen._block_size(words, style, 1080))})

# ---------------------------------------------------------------- rendering service helpers
out["overlays_compose"] = []
for _ in range(20):
    ovs = []
    for k in range(rng.randint(0, 3)):
        if rng.random() < 0.5:
            ovs.append((f"/tmp/o{k}.png", "(W-w)/2", str(rng.randint(0, 500))))
        else:
            ovs.append((f"/tmp/o{k}.png", "(W-w)/2", "40", "between(t,0.4,5.5)", "format=rgba"))
    vf = rng.choice(["null", speed_video_filter(1.5, "30")])
    graph, extra = RenderingService._compose_overlays("[0:v]null[base];[base]null[captioned]", ovs, vf)
    out["overlays_compose"].append({"overlays": [list(o) for o in ovs], "video_filter": vf, "graph": graph,
                                    "extra": extra})

out["escape"] = []
svc0 = RenderingService.__new__(RenderingService)
for p in ["/tmp/a b/c.ass", "/tmp/it's [x]/c.ass", "C:\\Users\\me\\c.ass", "/x/y:z.ass"]:
    out["escape"].append({"path": p, "escaped": svc0._escape_filter_path(p)})

out["codec_args"] = []
settings = get_settings()
for local, platform in [(False, "linux"), (True, "darwin")]:
    for w, h in [(1080, 1920), (1920, 1080), (2560, 1440), (3840, 2160), (1000, 900)]:
        for fps in ["30", "60", "30000/1001", "24", "60000/1001", "25"]:
            object.__setattr__(settings, "local_mode", local)
            real_platform = sys.platform
            sys.platform = platform
            try:
                args = RenderingService._video_codec_args(SimpleNamespace(settings=settings), w, h, fps)
            finally:
                sys.platform = real_platform
            out["codec_args"].append({"videotoolbox": local, "out": [w, h], "fps": fps, "args": args})
object.__setattr__(settings, "local_mode", False)

out["probe_fps"] = []
for stream in [{"avg_frame_rate": "30000/1001", "r_frame_rate": "30000/1001"}, {"avg_frame_rate": "0/0",
               "r_frame_rate": "25/1"}, {"avg_frame_rate": "120/1"}, {"avg_frame_rate": "5/1", "r_frame_rate": "24/1"},
               {"avg_frame_rate": "121/2"}, {"avg_frame_rate": "61/1"}, {"avg_frame_rate": "60/1"},
               {"avg_frame_rate": "30"}, {"avg_frame_rate": "abc/1"}, {}, {"avg_frame_rate": "5/1"},
               {"avg_frame_rate": "60000/2002"}]:
    body = json.dumps({"streams": [stream]}).encode()

    def fake(cmd, timeout=None, body=body):
        return SimpleNamespace(returncode=0, stdout=body, stderr=b"")

    rs_module.run_media, saved = fake, rs_module.run_media
    try:
        fps = asyncio.run(svc0._probe_fps("x.mp4"))
    finally:
        rs_module.run_media = saved
    out["probe_fps"].append({"stdout": body.decode(), "fps": fps})

# ---------------------------------------------------------------- full render flows (FFmpeg stubbed)
FONTS = os.path.abspath(os.path.join(os.path.dirname(rs_module.__file__), "..", "..", "assets", "fonts"))


def flow_case(idx):
    aspect = rng.choice(["9:16", "9:16", "16:9"])
    style = rng.choice(["auto", "fill", "fit"])
    pacing = rng.choice(["tight", "tight", "natural"])
    speed = rng.choice([1, 1, 1.5, 2, 1.25])
    longform = aspect == "16:9" and rng.random() < 0.5
    start = rng.randint(0, 30000)
    end = start + rng.randint(3000, 40000)
    segs = rsegments(max(0, start - 1000), rng.randint(1, 4)) if rng.random() < 0.9 else []
    src = rng.choice([(1920, 1080), (1280, 720), (1440, 1080), (1080, 1920)])
    probe_dims = rng.random() < 0.85
    has_audio = rng.random() < 0.85
    fps_body = rng.choice(["30000/1001", "25/1", "60/1", "24000/1001"])
    ffmpeg_failures = rng.choice([0, 0, 0, 1, 2])
    captions = rng.random() < 0.8
    title = rng.choice([None, "Faster clips", "One", "A very long title that has way too many words in it"])
    banner = rng.random() < 0.4
    skips = []
    if longform or rng.random() < 0.2:
        for _ in range(rng.randint(0, 2)):
            a = rng.randint(start, end)
            skips.append((a, a + rng.randint(500, 6000)))
    chapters = [(start + k * rng.randint(5000, 20000), f"Chapter {k}") for k in range(rng.randint(0, 4))]
    window = (max(0, start - 150), end - max(0, start - 150) + 150)
    plan = rplan(window[1], src=src) if rng.random() < 0.8 else None
    content = rplan(window[1], src=src) if rng.random() < 0.7 else None
    loud_ok = rng.random() < 0.7
    big_graph = rng.random() < 0.15
    if big_graph:
        # Many keeps: a long filter graph goes through a script file.
        segs = [TranscriptSegment(start, end, "x", words=[
            TranscriptWord("w", t, t + 200) for t in range(start, end - 400, 1100)])]
        pacing = "tight"
    preset = rng.choice(PRESETS)
    return {
        "aspect_ratio": aspect, "layout_style": style, "pacing": pacing, "video_speed": speed,
        "longform": longform, "start_time_ms": start, "end_time_ms": end, "segments": [seg_j(s) for s in segs],
        "source": list(src), "probe_dims": probe_dims, "has_audio": has_audio, "fps_body": fps_body,
        "ffmpeg_failures": ffmpeg_failures, "include_captions": captions, "title": title,
        "banner": banner, "skips": skips, "chapters": chapters, "plan": plan_j(plan) if plan else None,
        "content_plan": plan_j(content) if content else None, "loud_ok": loud_ok, "preset": preset,
        "emphasis": rng.choice([[], ["huge"], ["14,500"]]),
        "layout_vision": rng.random() < 0.5,
    }


def plan_from(j):
    if j is None:
        return None
    b = lambda d: None if d is None else Box(d["x"], d["y"], d["w"], d["h"])
    shots = [ShotLayout(s["start_ms"], s["end_ms"], s["layout"], source=s["source"],
                        detected_layout=s["detected_layout"],
                        focus_path=[tuple(p) for p in s["focus_path"]], people=[b(p) for p in s["people"]],
                        screen_box=b(s["screen_box"]), screen_focus=b(s["screen_focus"]), cam_box=b(s["cam_box"]),
                        cam_face=b(s["cam_face"])) for s in j["shots"]]
    return ClipLayoutPlan(shots, j["source_width"], j["source_height"], vision_cost_usd=j["vision_cost_usd"],
                          face_samples=[(t, [b(x) for x in faces]) for t, faces in j["face_samples"]])


def seg_from(j):
    return TranscriptSegment(j["start_time_ms"], j["end_time_ms"], j["text"], speaker_label=j["speaker_label"],
                             words=[TranscriptWord(w["word"], w["start_time_ms"], w["end_time_ms"]) for w in j["words"]],
                             audio_events=list(j["audio_events"]))


LOUDNORM_STDERR = (
    "[Parsed_loudnorm_2 @ 0x1] \n{\n\t\"input_i\" : \"-23.54\",\n\t\"input_tp\" : \"-5.20\",\n"
    "\t\"input_lra\" : \"6.10\",\n\t\"input_thresh\" : \"-34.00\",\n\t\"output_i\" : \"-14.1\",\n"
    "\t\"target_offset\" : \"0.40\"\n}\n"
)


def run_flow(case, tmp):
    svc = RenderingService.__new__(RenderingService)
    svc.settings = get_settings()
    svc.caption_generator = CaptionGeneratorService()
    svc._font_path = os.path.join(FONTS, "Montserrat-Black.ttf")
    svc._fonts_dir = FONTS
    log = []
    ffmpeg_calls = [0]

    def responder(cmd, timeout=None, **kw):
        cmd = list(cmd)
        if "-/filter_complex" in cmd:
            i = cmd.index("-/filter_complex")
            cmd[i + 1] = "SCRIPT:" + open(cmd[i + 1], encoding="utf-8").read()
        text = json.dumps(cmd).replace(tmp, "{DIR}").replace(FONTS, "{FONTS}")
        log.append(json.loads(text))
        prog = cmd[0]
        if prog == "ffprobe":
            if "stream=width,height" in cmd:
                return SimpleNamespace(returncode=0 if case["probe_dims"] else 1,
                                       stdout=(f"{case['source'][0]}x{case['source'][1]}\n".encode()
                                               if case["probe_dims"] else b""), stderr=b"")
            if "stream=index" in cmd:
                return SimpleNamespace(returncode=0, stdout=b"1\n" if case["has_audio"] else b"", stderr=b"")
            if "stream=avg_frame_rate,r_frame_rate" in cmd:
                body = json.dumps({"streams": [{"avg_frame_rate": case["fps_body"], "r_frame_rate": "0/0"}]})
                return SimpleNamespace(returncode=0, stdout=body.encode(), stderr=b"")
            raise AssertionError(cmd)
        if "print_format=json" in " ".join(cmd):
            return SimpleNamespace(returncode=0 if case["loud_ok"] else 1, stdout=b"",
                                   stderr=LOUDNORM_STDERR.encode() if case["loud_ok"] else b"err")
        ffmpeg_calls[0] += 1
        if ffmpeg_calls[0] <= case["ffmpeg_failures"]:
            return SimpleNamespace(returncode=1, stdout=b"", stderr=b"x" * 1200 + b"Cannot select channel layout")
        with open(cmd[-1], "wb") as f:
            f.write(b"\0" * 2048)
        return SimpleNamespace(returncode=0, stdout=b"", stderr=b"")

    async def validate(output_path, duration_ms, fps, with_audio):
        log.append(["VALIDATE", duration_ms, fps, with_audio])

    async def plan_layout(request, w, h, ws, wms):
        log.append(["PLAN", request.layout_style, w, h, ws, wms])
        if request.layout_style == "fit":
            return None
        return plan_from(case["plan"])

    class Analyzer:
        async def analyze(self, video, s, wms, w, h, style, vision=True):
            log.append(["CONTENT", style, vision, w, h, s, wms])
            return plan_from(case["content_plan"])

    svc.layout_analyzer = Analyzer()
    svc._plan_layout = plan_layout
    svc._validate_output_timing = validate
    saved = rs_module.run_media
    rs_module.run_media = responder
    style = get_caption_preset(case["preset"])
    request = RenderRequest(
        video_path=os.path.join(tmp, "src.mp4"), output_path=os.path.join(tmp, "out", "clip_00.mp4"),
        start_time_ms=case["start_time_ms"], end_time_ms=case["end_time_ms"],
        source_width=case["source"][0], source_height=case["source"][1],
        transcript_segments=[seg_from(s) for s in case["segments"]], include_captions=case["include_captions"],
        caption_style=style, title_text=case["title"], emphasis_words=case["emphasis"],
        banner_platform="youtube" if case["banner"] else None,
        banner_channel_url="example.com/@me" if case["banner"] else None,
        aspect_ratio=case["aspect_ratio"], layout_style=case["layout_style"], pacing=case["pacing"],
        video_speed=case["video_speed"], longform=case["longform"], skip_ranges_ms=[tuple(s) for s in case["skips"]],
        chapters=[tuple(c) for c in case["chapters"]],
    )
    try:
        result = asyncio.run(svc.render_clip(request))
        res = {
            "duration_ms": result.duration_ms, "removed_ms": result.removed_ms, "layout_type": result.layout_type,
            "layout_shots": result.layout_shots, "layout_cost_usd": result.layout_cost_usd,
            "render_fallback": result.render_fallback, "chapters": [list(c) for c in result.chapters],
            "subtitle": open(result.subtitle_path, encoding="utf-8").read() if result.subtitle_path else None,
            "output": [result.output_width, result.output_height],
        }
    except Exception as e:  # noqa: BLE001
        res = {"error": type(e).__name__ + ": " + str(e)[:80]}
    finally:
        rs_module.run_media = saved
    ass_path = os.path.join(tmp, "out", f"clip-{case['start_time_ms']}-{case['end_time_ms']}.ass")
    res["ass"] = open(ass_path, encoding="utf-8").read() if os.path.isfile(ass_path) else None
    return log, res


out["flows"] = []
for idx in range(56):
    case = flow_case(idx)
    tmp = tempfile.mkdtemp(prefix="golden-")
    try:
        log, res = run_flow(case, tmp)
    finally:
        shutil.rmtree(tmp)
    case["log"] = log
    case["result"] = res
    out["flows"].append(case)

path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "golden.json.gz")
with gzip.open(path, "wt", encoding="utf-8", compresslevel=9) as f:
    json.dump(out, f, ensure_ascii=False, separators=(",", ":"))
print("wrote", path, os.path.getsize(path), "bytes")
