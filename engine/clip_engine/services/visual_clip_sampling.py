"""Bounded frame sampling for visual-only clip planning."""

import asyncio
import logging
import math
import os
import subprocess
from pathlib import Path

from PIL import Image, ImageChops, ImageStat

from clip_engine.services.intelligence_planner import VisionFrame
from clip_engine.services.media_process import media_child_env

logger = logging.getLogger(__name__)

MAX_PLANNING_FRAMES = 24
MAX_SUPPORT_GAP_SECONDS = 45
FRAME_WIDTH = 512
FRAME_TIMEOUT_SECONDS = 20


def has_visual_change(frames: list[VisionFrame]) -> bool:
    """Reject a still image or static slide before paying for visual planning."""
    if len(frames) < 3:
        return False
    previous = None
    try:
        for frame in frames:
            with Image.open(frame.file_path) as image:
                current = image.convert("L").resize((64, 36))
            if previous is not None and frame.timestamp_ms - previous_time <= MAX_SUPPORT_GAP_SECONDS * 1000:
                difference = ImageStat.Stat(ImageChops.difference(previous, current)).mean[0]
                if difference >= 3.0:
                    return True
            previous = current
            previous_time = frame.timestamp_ms
    except (OSError, ValueError):
        return False
    return False


def _sample_one(video_path: str, output_path: Path, timestamp: float) -> bool:
    command = [
        "ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y",
        "-ss", f"{timestamp:.3f}", "-protocol_whitelist", "file,pipe,fd", "-format_whitelist", "mov,matroska,webm,avi,flv,mpegts",
        "-i", video_path, "-map", "0:v:0", "-frames:v", "1",
        "-vf", f"scale={FRAME_WIDTH}:-2", "-q:v", "7", str(output_path),
    ]
    try:
        result = subprocess.run(
            command, capture_output=True, timeout=FRAME_TIMEOUT_SECONDS,
            env=media_child_env(), check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return result.returncode == 0 and output_path.is_file() and output_path.stat().st_size > 1024


async def sample_visual_planning_frames(
    video_path: str,
    duration_seconds: float,
    work_dir: str,
    start_time_seconds: float | None = None,
    end_time_seconds: float | None = None,
) -> list[VisionFrame]:
    """Sample up to 24 uniformly spaced source frames, within the chosen range."""
    if not math.isfinite(duration_seconds) or duration_seconds <= 0:
        return []
    start = max(0.0, start_time_seconds or 0.0)
    end = min(duration_seconds, end_time_seconds if end_time_seconds is not None else duration_seconds)
    if not math.isfinite(start) or not math.isfinite(end) or end <= start:
        return []

    # At least three samples are needed for the model to see a visual sequence.
    count = min(MAX_PLANNING_FRAMES, max(3, math.ceil((end - start) / 15)))
    frame_dir = Path(work_dir) / "visual_planning_frames"
    frame_dir.mkdir(parents=True, exist_ok=True)
    span = end - start
    if span / count <= MAX_SUPPORT_GAP_SECONDS:
        timestamps = [start + (index + 0.5) * span / count for index in range(count)]
    else:
        # Uniform samples on long sources are farther apart than one useful
        # short clip. Eight bounded three-frame windows give the planner local
        # evidence at representative points without increasing API media cost.
        windows = count // 3
        timestamps = [
            start + (window + 0.5) * span / windows + offset
            for window in range(windows)
            for offset in (-15.0, 0.0, 15.0)
        ]
    semaphore = asyncio.Semaphore(4)

    async def sample(index: int, timestamp: float) -> VisionFrame | None:
        output = frame_dir / f"frame_{index:02d}.jpg"
        async with semaphore:
            success = await asyncio.to_thread(_sample_one, video_path, output, timestamp)
        if not success:
            logger.warning("Visual planning frame unavailable at %.1fs", timestamp)
            return None
        return VisionFrame(timestamp_ms=round(timestamp * 1000), file_path=str(output), width=FRAME_WIDTH, height=0)

    frames = await asyncio.gather(*(sample(index, timestamp) for index, timestamp in enumerate(timestamps)))
    return [frame for frame in frames if frame is not None]
