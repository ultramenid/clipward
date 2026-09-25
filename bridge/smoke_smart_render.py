#!/usr/bin/env python3
"""Release smoke test for the bundled Clipward smart graph and FFmpeg audio."""

import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

from clip_engine.services.layout_analyzer import Box, ClipLayoutPlan, LayoutType, ShotLayout
from clip_engine.services.layout_renderer import build_layout_graph


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: smoke_smart_render.py <ffmpeg>", file=sys.stderr)
        return 2

    shot = ShotLayout(
        0,
        1000,
        LayoutType.SCREEN_CAM,
        source="vision",
        screen_box=Box(0, 0, 1, 1),
        cam_box=Box(0.73, 0.68, 0.25, 0.28),
        cam_face=Box(0.80, 0.72, 0.09, 0.12),
    )
    graph = build_layout_graph(ClipLayoutPlan([shot], 640, 360), 360, 640, with_audio=True)
    with tempfile.TemporaryDirectory(prefix="clipward-smart-smoke-") as work:
        title = Path(work) / "title.png"
        Image.new("RGBA", (64, 32), (255, 255, 255, 255)).save(title)
        command = [
            sys.argv[1], "-v", "error", "-f", "lavfi", "-i",
            "testsrc2=size=640x360:rate=30:duration=1[out0];sine=frequency=440:duration=1[out1]",
            "-loop", "1", "-i", str(title),
            "-filter_complex", f"{graph};[base][1:v]overlay=x=20:y=20:shortest=1[out]",
            "-map", "[out]", "-map", "[aout]", "-t", "1", "-f", "null", "-",
        ]
        result = subprocess.run(command, capture_output=True, text=True, timeout=60, check=False)
    if result.returncode:
        print(result.stderr[-1000:], file=sys.stderr)
        return 1
    print("Bundled smart render passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
