"""Local downloads must stay on the guarded Python network stack."""

import asyncio
from types import SimpleNamespace
from unittest.mock import patch

from clip_engine.services.video_downloader import VideoDownloaderService


def test_local_download_and_metadata_disable_native_handler_and_proxy():
    settings = SimpleNamespace(local_mode=True, get_proxy_list=lambda: ["http://proxy.invalid"])
    captured = []

    class FakeYoutubeDL:
        def __init__(self, options):
            captured.append(options)

        def __enter__(self):
            return self

        def __exit__(self, *_):
            return None

        def extract_info(self, *_args, **_kwargs):
            return {"title": "test", "duration": 5, "width": 320, "height": 240, "fps": 30}

    with patch("clip_engine.services.video_downloader.get_settings", return_value=settings), \
         patch("clip_engine.services.video_downloader.yt_dlp.YoutubeDL", FakeYoutubeDL):
        service = VideoDownloaderService()
        options = service._build_ytdlp_opts()
        assert options["nocheckcertificate"] is False
        assert options["proxy"] == ""
        asyncio.run(service._get_video_info("https://example.com/video"))

    assert captured[0]["nocheckcertificate"] is False
    assert captured[0]["proxy"] == ""


def test_disguised_playlist_cannot_read_an_unselected_video(tmp_path):
    """Reject a playlist before ffprobe follows its sibling-file reference."""
    import shutil
    import subprocess
    import pytest
    from clip_engine.services.video_downloader import VideoDownloadError

    if not shutil.which('ffmpeg') or not shutil.which('ffprobe'):
        pytest.skip('FFmpeg tools are required for the malicious-media regression')
    private = tmp_path / 'private.mp4'
    subprocess.run([
        'ffmpeg', '-v', 'error', '-f', 'lavfi', '-i', 'color=s=64x64:d=0.2',
        '-c:v', 'mpeg4', str(private),
    ], check=True, capture_output=True)
    playlist = tmp_path / 'disguised.mp4'
    playlist.write_text("ffconcat version 1.0\nfile 'private.mp4'\n")
    service = VideoDownloaderService.__new__(VideoDownloaderService)
    with pytest.raises(VideoDownloadError, match='metadata probe failed'):
        service._run_ffprobe_sync(str(playlist))
    assert service._run_ffprobe_sync(str(private))[0] == 0


def test_youtube_downloads_prefer_highest_quality_without_av1():
    from clip_engine.services import video_downloader as module

    with patch("clip_engine.services.video_downloader.get_settings", return_value=SimpleNamespace(local_mode=True)):
        options = VideoDownloaderService()._build_ytdlp_opts()

    assert options["format"] == module.YOUTUBE_FORMAT_SELECTORS[0]
    assert options["format_sort"] == ["hdr:SDR", "res:2160", "fps"]
    # No selector may pin a resolution or allow AV1, which FFmpeg cannot decode here.
    for selector in module.YOUTUBE_FORMAT_SELECTORS:
        assert "height" not in selector
        assert all("vcodec!^=av01" in part or "vcodec^=avc1" in part or part == "ba" or part.startswith("ba[")
                   for alternative in selector.split("/") for part in alternative.split("+"))


def test_disk_full_is_reported_through_wrapped_errors():
    import errno

    from clip_engine.error_policy import safe_processing_error
    from clip_engine.services.video_downloader import VideoDownloadError

    disk_full = "Not enough disk space to save clips"
    assert safe_processing_error(OSError(errno.ENOSPC, "No space left on device")) == disk_full
    assert safe_processing_error(
        VideoDownloadError("Failed to download video: [Errno 28] No space left on device")) == disk_full
    try:
        try:
            raise OSError(errno.ENOSPC, "write failed")
        except OSError:
            raise VideoDownloadError("Failed to download video")
    except VideoDownloadError as wrapped:
        assert safe_processing_error(wrapped) == disk_full
    assert safe_processing_error(VideoDownloadError("HTTP Error 403")) == "Video download failed"


def test_download_stops_before_filling_the_disk(tmp_path):
    import pytest

    from clip_engine.error_policy import is_disk_full
    from clip_engine.services import video_downloader as module
    from clip_engine.services.video_downloader import VideoDownloadError

    usage = SimpleNamespace(total=100 * 1000 ** 3, used=0, free=3 * 1000 ** 3)
    with patch.object(module.shutil, "disk_usage", return_value=usage):
        VideoDownloaderService._check_free_space(str(tmp_path), 1000 ** 3)
        with pytest.raises(VideoDownloadError) as error:
            VideoDownloaderService._check_free_space(str(tmp_path), int(2.5 * 1000 ** 3))
    assert is_disk_full(error.value)
