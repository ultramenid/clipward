"""Golden fixtures from the Python planner for the Rust port's equivalence tests.

Run from anywhere with the pre-Rust engine checked out (engine/clip_engine) and
`pip install pydantic pydantic-settings httpx`:  python3 gen_fixtures.py
"""
import json
import os
import random
import sys
import types

ROOT = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), *[".."] * 5))
sys.path.insert(0, os.path.join(ROOT, "engine"))
# The planner imports yt-dlp through media_process; it is not needed here.
_ytdlp = types.ModuleType("yt_dlp")
_ytdlp_utils = types.ModuleType("yt_dlp.utils")


class _Popen:
    @classmethod
    def run(cls, *args, **kwargs):
        pass


_ytdlp_utils.Popen = _Popen
sys.modules.update({"yt_dlp": _ytdlp, "yt_dlp.utils": _ytdlp_utils})

_services = types.ModuleType("clip_engine.services")
_services.__path__ = [os.path.join(ROOT, "engine/clip_engine/services")]
sys.modules["clip_engine.services"] = _services
from clip_engine.config import Settings  # noqa: E402
from clip_engine.services.intelligence_planner import (  # noqa: E402
    IntelligencePlannerService, IntelligencePlanningError, campaign_brief_section, clip_plan_schema,
)
from clip_engine.services.transcription_service import TranscriptSegment, TranscriptWord  # noqa: E402

OUT = os.path.dirname(os.path.abspath(__file__))
os.makedirs(OUT, exist_ok=True)


def planner(**overrides):
    p = IntelligencePlannerService()
    p.settings = Settings(_env_file=None, openrouter_api_key="test", **overrides)
    return p


def sentences(n, words_per=12, word_ms=300, gap_ms=80, pause_ms=400, start=1000, speakers=False, events=False):
    segs, t = [], start
    for s in range(n):
        words = []
        for w in range(words_per):
            words.append(TranscriptWord(f"w{s}_{w}" + ("." if w == words_per - 1 else ""), t, t + word_ms))
            t += word_ms + gap_ms
        t += pause_ms
        segs.append(TranscriptSegment(
            words[0].start_time_ms, words[-1].end_time_ms, " ".join(x.word for x in words),
            (f"S{s % 3 + 1}" if speakers else None), words,
            audio_events=(["(laughter)", "(applause)"] if events and s % 7 == 3 else []),
        ))
    return segs


def irregular(n, seed):
    """Sentences with varying lengths, '?'/'!' endings, mid-sentence segment splits."""
    rng = random.Random(seed)
    segs, t = [], 500
    for s in range(n):
        words = []
        count = rng.randint(3, 20)
        for w in range(count):
            dur = rng.randint(120, 700)
            end_mark = rng.choice([".", "?", "!", "。"]) if w == count - 1 and rng.random() < 0.85 else ""
            if end_mark == "" and rng.random() < 0.05:
                end_mark = ","
            words.append(TranscriptWord(f"x{s}_{w}{end_mark}", t, t + dur))
            t += dur + rng.randint(0, 250)
        t += rng.randint(0, 2500)
        segs.append(TranscriptSegment(words[0].start_time_ms, words[-1].end_time_ms,
                                      " ".join(x.word for x in words), None, words))
    return segs


def segment_level(n):
    segs = []
    for i in range(n):
        text = f"Sentence number {i}" + ("." if i % 3 else "")
        segs.append(TranscriptSegment(i * 5000, (i + 1) * 5000 - 50, text))
    return segs


def seg_json(segs):
    return [{
        "start_time_ms": s.start_time_ms, "end_time_ms": s.end_time_ms, "text": s.text,
        "speaker_label": s.speaker_label,
        "words": [{"word": w.word, "start_time_ms": w.start_time_ms, "end_time_ms": w.end_time_ms} for w in s.words],
        "audio_events": list(getattr(s, "audio_events", []) or []),
    } for s in segs]


TRANSCRIPTS = {
    "regular": sentences(120),
    "long": sentences(200),
    "speakers": sentences(40, speakers=True, events=True),
    "irregular": irregular(150, 7),
    "segments": segment_level(60),
    "empty": [],
}

# ---------------------------------------------------------------- prompts
p = planner()
prompts = []
bounds = [(None, None, None), (15, 90, None), (15, 90, ["long"]), (None, None, ["short", "long"]),
          (None, None, ["xshort"]), (20, None, None), (None, None, ["bogus"]),
          (None, None, ["feature", "landscape_extended"]), (100, None, None)]
for count in (1, 5, 37):
    for mn, mx, ranges in bounds:
        prompts.append({"kind": "system", "clip_count": count, "min": mn, "max": mx, "ranges": ranges,
                        "text": p._build_system_prompt(count, mn, mx, ranges)})
        prompts.append({"kind": "visual", "clip_count": count, "min": mn, "max": mx, "ranges": ranges,
                        "text": p._build_visual_only_system_prompt(count, mn, mx, ranges)})
for count, mn, mx in [(2, 600, 900), (5, 300, 1800), (3, 900, 1800), (1, 359, 721)]:
    prompts.append({"kind": "longform", "clip_count": count, "min": mn, "max": mx, "ranges": None,
                    "text": p._build_longform_system_prompt(count, mn, mx)})
for req in ["  Mention Acme.\nNo swearing. {braces} #tag  ", "x", "Ünïcode — ok\n\n"]:
    prompts.append({"kind": "brief", "requirements": req, "text": campaign_brief_section(req)})
for name in ("speakers", "segments", "empty", "regular"):
    prompts.append({"kind": "transcript", "transcript": name,
                    "text": p._build_transcript_text(TRANSCRIPTS[name])})

frames = [{"base64": "QUJD", "mime_type": "image/jpeg", "timestamp_ms": t} for t in (10000, 20550, 31049)]
for name, frame_set, count, duration, longform in [
    ("speakers", [], 3, 0.0, False), ("speakers", frames, 7, 612.4, False), ("long", [], 2, 1234.5, True),
    ("empty", frames, 4, 60.0, False), ("regular", frames, 2, 0.0, True), ("empty", [], 3, 0.0, False),
]:
    tr = TRANSCRIPTS[name]
    msgs = p._build_vision_messages("SYS", p._build_transcript_text(tr), frame_set, count, tr, duration, longform)
    parts = []
    for part in msgs[1]["content"]:
        if part["type"] == "text":
            parts.append({"text": part["text"]})
        else:
            parts.append({"image": part["image_url"]["url"], "detail": part["image_url"]["detail"]})
    prompts.append({"kind": "user", "transcript": name, "frames": [f["timestamp_ms"] for f in frame_set],
                    "clip_count": count, "duration": duration, "longform": longform, "parts": parts})
prompts.append({"kind": "schema", "longform": False, "schema": clip_plan_schema(False)})
prompts.append({"kind": "schema", "longform": True, "schema": clip_plan_schema(True)})

# ---------------------------------------------------------------- clip counts
counts = []
for duration in (0, 30, 59.9, 300, 1800, 3600, 7200, 36000):
    for user_max in (None, 1, 3, 5, 50, 100, 250):
        for auto in (True, False):
            for wpm in (None, 0.0, 50.0, 128.0, 160.0, 200.0, 400.0):
                counts.append({"duration": duration, "user_max": user_max, "auto": auto, "wpm": wpm,
                               "expected": p.calculate_optimal_clip_count(duration, user_max, auto, wpm)})

# ---------------------------------------------------------------- parsing
rng = random.Random(1234)


def shown(ms):
    return round(ms / 1000, 1)


def pick_times(tr):
    """A plausible (start, end) in seconds for transcript tr."""
    if not tr:
        a = rng.uniform(0, 600)
        return round(a, 1), round(a + rng.uniform(1, 200), 1)
    i = rng.randrange(len(tr))
    j = min(len(tr) - 1, i + rng.randint(0, 40))
    start = tr[i].start_time_ms
    end = tr[j].end_time_ms
    mode = rng.random()
    if mode < 0.4:
        return shown(start), shown(end)
    if mode < 0.7:
        return round((start + rng.randint(-3000, 3000)) / 1000, 3), round((end + rng.randint(-4000, 4000)) / 1000, 3)
    if mode < 0.8:
        return start, end  # milliseconds
    return shown(start), shown(start) + rng.choice([3, 10, 14.9, 15, 29.99, 60, 90.5, 301, 700])


cases = []
bound_choices = [(None, None, None), (15, 90, None), (15, 60, None), (5, 600, None), (15, 20, None), (15, 15, None),
                 (None, None, ["short"]), (None, None, ["long"]), (None, None, ["xshort", "medium"]),
                 (None, None, ["xlong"]), (None, None, ["extended"])]
for n in range(700):
    name = rng.choice(["regular", "regular", "irregular", "irregular", "segments", "long", "empty"])
    tr = TRANSCRIPTS[name]
    mn, mx, ranges = rng.choice(bound_choices)
    total_s = (tr[-1].end_time_ms / 1000) if tr else 600
    start_limit = end_limit = None
    if rng.random() < 0.3:
        start_limit = round(rng.uniform(0, total_s * 0.6), rng.choice([0, 1, 3]))
    if rng.random() < 0.3:
        end_limit = round(rng.uniform(total_s * 0.4, total_s * 1.1), rng.choice([0, 1, 3]))
    video_duration = rng.choice([None, None, total_s, total_s * 0.8, 0.0])
    longform = name in ("long", "regular") and rng.random() < 0.3
    visual_times = sorted(round(rng.uniform(0, 600), 1) for _ in range(rng.randint(3, 20))) if not tr and rng.random() < 0.8 else []
    clips = []
    for _ in range(rng.randint(0, 6)):
        start, end = pick_times(tr)
        clip = {"summary": rng.choice(["Title", "  Big  Title ", None, "Ünï"]), "tags": ["a", 3, "b"],
                "emphasis": ["one", "two", 5, "three", "four", "five", "six"]}
        keys = rng.choice([("start_time", "end_time"), ("startTime", "endTime"), ("start", "end")])
        clip[keys[0]], clip[keys[1]] = start, end
        r = rng.random()
        if r < 0.6:
            clip["scores"] = {d: rng.choice([0, 3, 7.5, 10, 12, -1, "8"]) for d in
                              rng.sample(["hook", "standalone", "arc", "quotability", "ending"], rng.randint(1, 5))}
        elif r < 0.8:
            clip["virality_score"] = rng.choice([0.3, 1.7, "x", None])
        if clip.get("summary") is None:
            clip.pop("summary")
        if longform:
            s_ms, e_ms = start * 1000, end * 1000
            clip["skip"] = [{"start_time": round(rng.uniform(s_ms, e_ms) / 1000, 1),
                             "end_time": round(rng.uniform(s_ms, e_ms) / 1000 + rng.uniform(0, 90), 1)}
                            for _ in range(rng.randint(0, 4))] + rng.choice([[], ["bad"], [{"start_time": "x"}]])
            clip["chapters"] = [{"time": round(rng.uniform(s_ms - 10000, e_ms) / 1000, 1),
                                 "title": rng.choice(["Opening", "  Main   point ", "", "A" * 100])}
                                for _ in range(rng.randint(0, 8))]
            clip["description"] = rng.choice(["  About things.  ", "", "D" * 2000])
        clips.append(clip)
    body = {"insights": "x", "clips": clips}
    content_mode = rng.random()
    if content_mode < 0.8:
        content = json.dumps(body)
    elif content_mode < 0.9:
        content = "Here you go:\n```json\n" + json.dumps(body) + "\n```"
    elif content_mode < 0.93:
        content = json.dumps(clips)
    elif content_mode < 0.96:
        content = "not json at all"
    else:
        content = json.dumps({"clips": [{"start_time": "10", "end_time": 40}]})
    pl = planner()
    pl._current_min_duration = mn
    pl._current_max_duration = mx
    pl._current_duration_ranges = ranges
    pl._current_transcript = tr
    pl._start_time_seconds = start_limit
    pl._end_time_seconds = end_limit
    pl._current_video_duration = video_duration
    pl._current_longform = longform
    pl._current_visual_frame_times = visual_times
    pl._current_target_platform = "tiktok"
    response = {"choices": [{"message": {"content": content}, "finish_reason": "stop"}]}
    try:
        plan = pl._parse_clip_plan_response(response)
        expected = {"segments": [{
            "start": s.start_time_ms, "end": s.end_time_ms, "score": s.virality_score, "summary": s.summary,
            "tags": [t for t in s.tags if isinstance(t, str)], "emphasis": s.emphasis_words,
            "skips": [list(x) for x in s.skip_ranges_ms], "chapters": [list(x) for x in s.chapters],
            "description": s.description,
        } for s in plan.segments], "insights": plan.insights}
    except IntelligencePlanningError as e:
        expected = {"error": True, "retryable": e.retryable}
    cases.append({"transcript": name, "min": mn, "max": mx, "ranges": ranges, "start_limit": start_limit,
                  "end_limit": end_limit, "video_duration": video_duration, "longform": longform,
                  "visual_times": visual_times, "content": content, "expected": expected})

with open(os.path.join(OUT, "transcripts.json"), "w", encoding="utf-8") as f:
    json.dump({k: seg_json(v) for k, v in TRANSCRIPTS.items()}, f, separators=(",", ":"))
with open(os.path.join(OUT, "prompts.json"), "w", encoding="utf-8") as f:
    json.dump(prompts, f, separators=(",", ":"))
with open(os.path.join(OUT, "clip_counts.json"), "w", encoding="utf-8") as f:
    json.dump(counts, f, separators=(",", ":"))
with open(os.path.join(OUT, "parse_cases.json"), "w", encoding="utf-8") as f:
    json.dump(cases, f, separators=(",", ":"))
errors = sum(1 for c in cases if "error" in c["expected"])
kept = sum(len(c["expected"].get("segments", [])) for c in cases)
print(f"{len(prompts)} prompts, {len(counts)} counts, {len(cases)} parse cases ({errors} errors, {kept} clips kept)")
