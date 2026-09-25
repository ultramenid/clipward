"""Local jobs discard downloaded and intermediate media on every exit path."""

import asyncio

import pytest

from clip_engine.services import ai_clipping_pipeline as pipeline_module
from clip_engine.services.ai_clipping_pipeline import AIClippingPipeline, ClippingJobRequest, JobStatus


@pytest.mark.parametrize("failure", [RuntimeError("test failure"), asyncio.CancelledError()])
def test_local_work_directory_is_removed_after_failure_or_cancellation(monkeypatch, tmp_path, failure):
    settings = pipeline_module.get_settings()
    monkeypatch.setattr(settings, "local_mode", True)
    monkeypatch.setattr(settings, "local_output_dir", str(tmp_path / "out"))
    monkeypatch.setattr(settings.__class__, "temp_directory", property(lambda self: str(tmp_path / "work")))

    pipeline = AIClippingPipeline()

    async def fail_after_download_starts(url, output_dir):
        (tmp_path / "work" / "job1" / "downloaded-source.mp4").write_bytes(b"test media")
        raise failure

    monkeypatch.setattr(pipeline.video_downloader, "download_video", fail_after_download_starts)
    request = ClippingJobRequest(video_url="https://example.com/video", job_id="job1")

    if isinstance(failure, asyncio.CancelledError):
        with pytest.raises(asyncio.CancelledError):
            asyncio.run(pipeline.process_video(request))
    else:
        result = asyncio.run(pipeline.process_video(request))
        assert result.status == JobStatus.FAILED

    assert not (tmp_path / "work" / "job1").exists()
