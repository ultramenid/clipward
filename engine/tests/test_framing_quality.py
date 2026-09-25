"""Offline golden cases for automatic vertical framing quality.

The inputs are face boxes rather than decoded video frames, so failures point
to planning or crop geometry without depending on a detector or paid vision API.
"""

from dataclasses import dataclass
import re

import pytest

from clip_engine.services.layout_analyzer import (
    Box,
    FrameInfo,
    LayoutType,
    ShotLayout,
    classify_shot,
    merge_vision_result,
    smooth_focus_path,
    track_faces,
)
from clip_engine.services.layout_renderer import (
    MAX_UPSCALE,
    cam_crop,
    fill_crop,
    panel_fit,
    person_crop,
    screen_crop,
    stacked_panel_heights,
)


OUT_W, OUT_H = 1080, 1920
DURATION_MS = 3000


@dataclass(frozen=True)
class GoldenCase:
    name: str
    width: int
    height: int
    faces: tuple[Box, ...]
    expected_layout: str
    vision_result: dict | None = None


GOLDEN_CASES = (
    GoldenCase("talking_head", 1920, 1080, (Box(0.43, 0.20, 0.14, 0.30),), LayoutType.TALKING_HEAD),
    GoldenCase(
        "two_shot", 1920, 1080,
        (Box(0.20, 0.25, 0.12, 0.25), Box(0.68, 0.25, 0.12, 0.25)),
        LayoutType.TWO_SHOT,
    ),
    GoldenCase("corner_cam", 1920, 1080, (Box(0.84, 0.80, 0.05, 0.09),), LayoutType.SCREEN_CAM),
    GoldenCase(
        "bottom_center_cam", 1920, 1080, (Box(0.47, 0.78, 0.06, 0.09),), LayoutType.SCREEN_CAM,
        # A centered overlay is ambiguous from face position alone. This is a
        # deterministic stand-in for the vision model's full overlay rectangle.
        {
            "layout": "screen_cam",
            "cam_box": [700, 300, 980, 700],
            "screen_box": [0, 0, 1000, 1000],
            "screen_focus": [],
            "people": [],
        },
    ),
    GoldenCase("already_vertical", 1080, 1920, (Box(0.43, 0.20, 0.14, 0.24),), LayoutType.TALKING_HEAD),
)


def plan_case(case: GoldenCase) -> ShotLayout:
    """Exercise face tracking, classification, vision merge, and focus path."""
    # The histogram is unused by this single-shot fixture, but FrameInfo's
    # shape matches the analyzer's decoded frame contract.
    frames = [FrameInfo(t_ms=t, faces=list(case.faces), hist=None) for t in range(0, DURATION_MS, 250)]
    shot, main_track = classify_shot(track_faces(frames), len(frames), case.width, case.height)
    if main_track is not None:
        crop_w_frac = min(1.0, (case.height * 9 / 16) / case.width)
        shot.focus_path = smooth_focus_path(main_track.samples, DURATION_MS, crop_w_frac)
    if case.vision_result is not None:
        shot = merge_vision_result(shot, case.vision_result, case.width, case.height)
    shot.start_ms, shot.end_ms = 0, DURATION_MS
    return shot


def face_visible_fraction(face: Box, rect: tuple[int, int, int, int], src_w: int, src_h: int) -> float:
    """Fraction of a known face box retained by a source-pixel crop."""
    w, h, x, y = rect
    fx0, fy0 = face.x * src_w, face.y * src_h
    fx1, fy1 = (face.x + face.w) * src_w, (face.y + face.h) * src_h
    overlap_w = max(0.0, min(fx1, x + w) - max(fx0, x))
    overlap_h = max(0.0, min(fy1, y + h) - max(fy0, y))
    return overlap_w * overlap_h / ((fx1 - fx0) * (fy1 - fy0))


def static_crop_coordinate(expression: str) -> int:
    """Read the first value of a constant focus path's FFmpeg expression."""
    if expression == "0":
        return 0
    match = re.search(r"if\(lt\(t,[0-9.]+\),(-?[0-9.]+)\+", expression)
    assert match is not None, expression
    value = float(match.group(1))
    assert expression.endswith(f"{value:.1f})"), expression
    return int(value)


@pytest.mark.parametrize("case", GOLDEN_CASES, ids=lambda case: case.name)
def test_golden_auto_framing(case: GoldenCase) -> None:
    shot = plan_case(case)
    assert shot.layout == case.expected_layout

    if shot.layout == LayoutType.TALKING_HEAD:
        crop_w, crop_h, x_expr, y_expr = fill_crop(shot, case.width, case.height, OUT_W, OUT_H)
        # These fixtures hold still, yielding a numeric crop position.
        rect = (crop_w, crop_h, static_crop_coordinate(x_expr), static_crop_coordinate(y_expr))
        assert face_visible_fraction(case.faces[0], rect, case.width, case.height) >= 0.98
        assert max(OUT_W / crop_w, OUT_H / crop_h) <= MAX_UPSCALE
        if case.name == "already_vertical":
            assert rect == (case.width, case.height, 0, 0)

    elif shot.layout == LayoutType.TWO_SHOT:
        assert len(shot.people) == 2
        assert shot.people[0].cx < shot.people[1].cx
        middle = (shot.people[0].cx + shot.people[1].cx) * case.width / 2
        for face, panel_h, bounds in zip(
            shot.people, (OUT_H // 2, OUT_H // 2), ((0, middle), (middle, case.width))
        ):
            rect = person_crop(face, case.width, case.height, OUT_W, panel_h, bounds)
            assert face_visible_fraction(face, rect, case.width, case.height) >= 0.95
            assert max(OUT_W / rect[0], panel_h / rect[1]) <= MAX_UPSCALE * 1.02

    elif shot.layout == LayoutType.SCREEN_CAM:
        assert shot.cam_box is not None
        assert shot.cam_box.contains(case.faces[0].cx, case.faces[0].cy)
        top_h, bottom_h = stacked_panel_heights(shot, case.height, OUT_H)
        cam_rect = cam_crop(shot.cam_box, shot.cam_face, case.width, case.height, OUT_W, bottom_h)
        assert face_visible_fraction(case.faces[0], cam_rect, case.width, case.height) >= 0.95
        fitted = panel_fit(cam_rect, OUT_W, bottom_h)
        shown_w, shown_h = fitted or (OUT_W, bottom_h)
        assert max(shown_w / cam_rect[0], shown_h / cam_rect[1]) <= MAX_UPSCALE * 1.02

        screen_rect = screen_crop(
            shot.screen_box, shot.screen_focus, case.width, case.height, OUT_W, top_h, shot.cam_box
        )
        assert max(OUT_W / screen_rect[0], top_h / screen_rect[1]) <= MAX_UPSCALE * 1.02


def test_golden_set_fallback_rate() -> None:
    layouts = [plan_case(case).layout for case in GOLDEN_CASES]
    assert layouts.count(LayoutType.SCREEN) == 0


def test_no_face_input_uses_safe_screen_fallback() -> None:
    case = GoldenCase("no_faces", 1920, 1080, (), LayoutType.SCREEN)
    assert plan_case(case).layout == LayoutType.SCREEN
