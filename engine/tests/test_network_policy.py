import socket
from unittest.mock import patch

from clip_engine.network_policy import public_source_url


def _answer(address):
    return [(socket.AF_INET, socket.SOCK_STREAM, 6, "", (address, 443))]


def test_public_media_url_requires_global_resolved_destination():
    with patch("clip_engine.network_policy.socket.getaddrinfo", return_value=_answer("8.8.8.8")):
        assert public_source_url("https://video.example.com/movie.mp4")
        assert not public_source_url("file:///etc/passwd")
        assert not public_source_url("https://user:pass@video.example.com/movie.mp4")
        assert not public_source_url("https://video.example.com:8080/movie.mp4")
    with patch("clip_engine.network_policy.socket.getaddrinfo", return_value=_answer("127.0.0.1")):
        assert not public_source_url("https://video.example.com/movie.mp4")
    assert not public_source_url("http://localhost/private")


def test_youtube_detection_uses_hostname_not_text_in_path():
    from types import SimpleNamespace
    from unittest.mock import patch
    from clip_engine.services.video_downloader import VideoDownloaderService

    settings = SimpleNamespace(local_mode=False, get_proxy_list=lambda: [])
    with patch("clip_engine.services.video_downloader.get_settings", return_value=settings):
        downloader = VideoDownloaderService()
        assert downloader.detect_source_type("https://www.youtube.com/watch?v=test") == "youtube"
        assert downloader.detect_source_type("https://evil.example/youtube.com/watch") == "direct_url"
