import logging

from clip_engine.logging_safety import SafeDiagnosticsFilter


def test_log_filter_removes_urls_paths_and_credentials():
    record = logging.LogRecord(
        name="test", level=logging.ERROR, pathname=__file__, lineno=1,
        msg="Download https://example.com/video?token=value from /Users/person/private/file.mp4 Authorization: Bearer secret-value",
        args=(), exc_info=None,
    )
    assert SafeDiagnosticsFilter().filter(record)
    text = record.getMessage()
    assert "example.com" not in text
    assert "person" not in text
    assert "secret-value" not in text


def test_log_filter_removes_json_credentials():
    record = logging.LogRecord(
        name="test", level=logging.ERROR, pathname=__file__, lineno=1,
        msg='Provider rejected {"api_key":"secret-value"}', args=(), exc_info=None,
    )
    assert SafeDiagnosticsFilter().filter(record)
    assert "secret-value" not in record.getMessage()
