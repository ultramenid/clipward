"""Minimal environment for local media tools processing untrusted input."""

import os
import subprocess
import threading
import time
from contextlib import contextmanager
from contextvars import ContextVar
from typing import Iterator

from yt_dlp.utils import Popen as YtDlpPopen


MEDIA_INPUT_OPTIONS = [
    "-protocol_whitelist", "file,pipe,fd",
    "-format_whitelist", "mov,matroska,webm,avi,flv,mpegts",
]
PROBE_TIMEOUT_SECONDS = 30
MEDIA_TIMEOUT_SECONDS = 2 * 60 * 60
MAX_TOOL_OUTPUT_BYTES = 1024 * 1024
MAX_VIDEO_EDGE = 16384
MAX_VIDEO_PIXELS = 64 * 1024 * 1024
MAX_VIDEO_ASPECT = 32


class MediaProcessError(subprocess.SubprocessError):
    """A media tool failed or exceeded its resource budget."""


def validate_video_dimensions(width: int, height: int) -> None:
    if (width <= 0 or height <= 0 or max(width, height) > MAX_VIDEO_EDGE
            or width * height > MAX_VIDEO_PIXELS
            or max(width, height) > min(width, height) * MAX_VIDEO_ASPECT):
        raise ValueError("Video dimensions exceed supported limits")


@contextmanager
def media_process(cmd: list[str], *, timeout: float = MEDIA_TIMEOUT_SECONDS,
                  max_stderr: int = MAX_TOOL_OUTPUT_BYTES):
    """Stream stdout while bounding stderr and the lifetime of a native tool.

    Keep the inherited process group: desktop cancellation must still terminate
    the bridge and its native tools together. On a stage failure, kill/reap the
    directly launched FFmpeg/FFprobe process before returning.
    """
    if timeout <= 0 or max_stderr < 0:
        raise ValueError("Invalid media process budget")
    process = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, env=media_child_env())
    timed_out = threading.Event()
    oversized = threading.Event()
    stderr = bytearray()

    def kill() -> None:
        if process.poll() is None:
            try:
                process.kill()
            except ProcessLookupError:
                pass

    def expire() -> None:
        timed_out.set()
        kill()

    def drain_stderr() -> None:
        while chunk := process.stderr.read(65536):
            if len(chunk) > max_stderr - len(stderr):
                oversized.set()
                kill()
                return
            stderr.extend(chunk)

    timer = threading.Timer(timeout, expire)
    timer.daemon = True
    reader = threading.Thread(target=drain_stderr, daemon=True)
    timer.start()
    reader.start()
    try:
        yield process, stderr
        process.wait()
        reader.join()
        if timed_out.is_set():
            raise MediaProcessError("Media processing exceeded its time limit")
        if oversized.is_set():
            raise MediaProcessError("Media diagnostics exceeded the size limit")
    finally:
        kill()
        process.wait()
        reader.join()
        timer.cancel()
        process.stdout.close()
        process.stderr.close()


def run_media(cmd: list[str], *, timeout: float = MEDIA_TIMEOUT_SECONDS,
              max_output: int = MAX_TOOL_OUTPUT_BYTES, check: bool = False) -> subprocess.CompletedProcess:
    """Capture small tool output without unbounded communicate() buffers."""
    with media_process(cmd, timeout=timeout, max_stderr=max_output) as (process, stderr):
        stdout = process.stdout.read(max_output + 1)
        if len(stdout) > max_output:
            raise MediaProcessError("Media output exceeded the size limit")
    result = subprocess.CompletedProcess(cmd, process.returncode, stdout, bytes(stderr))
    if check and result.returncode:
        raise MediaProcessError("Media processing failed")
    return result


_ALLOWED_ENV = {
    "PATH", "HOME", "TMPDIR", "TEMP", "TMP", "SYSTEMROOT", "WINDIR",
    "USERPROFILE", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "FONTCONFIG_PATH",
    "FONTCONFIG_FILE", "LANG", "LC_ALL", "LC_CTYPE", "TZ",
}


def media_child_env() -> dict[str, str]:
    """Keep the runtime paths media binaries need without passing provider keys."""
    return {key: value for key, value in os.environ.items() if key.upper() in _ALLOWED_ENV}


_in_media_scope: ContextVar[bool] = ContextVar("in_media_scope", default=False)
_media_deadline: ContextVar[float | None] = ContextVar("media_deadline", default=None)
_original_ytdlp_init = YtDlpPopen.__init__
_original_ytdlp_run = YtDlpPopen.run.__func__


def _guarded_ytdlp_init(self, args, *remaining, env=None, **kwargs):
    if _in_media_scope.get():
        source = media_child_env() if env is None else env
        env = {key: value for key, value in source.items() if key.upper() in _ALLOWED_ENV}
    _original_ytdlp_init(self, args, *remaining, env=env, **kwargs)


def _guarded_ytdlp_run(cls, *args, timeout=None, **kwargs):
    deadline = _media_deadline.get()
    if deadline is not None:
        remaining = max(0.001, deadline - time.monotonic())
        timeout = min(timeout, remaining) if timeout is not None else remaining
    return _original_ytdlp_run(cls, *args, timeout=timeout, **kwargs)


@contextmanager
def guarded_ytdlp_children(deadline: float | None = None) -> Iterator[None]:
    """Limit the environment inherited by yt-dlp's FFmpeg and JS children."""
    if YtDlpPopen.__init__ is not _guarded_ytdlp_init:
        YtDlpPopen.__init__ = _guarded_ytdlp_init
    if YtDlpPopen.run.__func__ is not _guarded_ytdlp_run:
        YtDlpPopen.run = classmethod(_guarded_ytdlp_run)
    token = _in_media_scope.set(True)
    deadline_token = _media_deadline.set(deadline)
    try:
        yield
    finally:
        _media_deadline.reset(deadline_token)
        _in_media_scope.reset(token)
