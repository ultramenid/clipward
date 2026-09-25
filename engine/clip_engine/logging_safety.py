"""Keep source URLs, paths, and provider diagnostics out of application logs."""

import logging
import re

_URL = re.compile(r"(?:https?|s3)://\S+", re.IGNORECASE)
_PATH = re.compile(r"(?<![\w])(?:/[\w. -]+){2,}|[A-Za-z]:\\\S+")
_CREDENTIAL = re.compile(
    r"(?i)\b(?:authorization|api[_ -]?key|token|password|secret)\"?\s*[:=]\s*\"?(?:Bearer\s+|Basic\s+)?[^\s,;\"}]+"
)


def safe_log_text(value: object) -> str:
    text = str(value)
    text = _CREDENTIAL.sub("[redacted credential]", text)
    text = _URL.sub("[redacted URL]", text)
    text = _PATH.sub("[redacted path]", text)
    return text[:1000]


class SafeDiagnosticsFilter(logging.Filter):
    def filter(self, record: logging.LogRecord) -> bool:
        try:
            record.msg = safe_log_text(record.getMessage())
        except Exception:
            record.msg = "[diagnostic unavailable]"
        record.args = ()
        record.exc_info = None
        record.exc_text = None
        record.stack_info = None
        return True


def install_safe_logging() -> None:
    root = logging.getLogger()
    for handler in root.handlers:
        if not any(isinstance(item, SafeDiagnosticsFilter) for item in handler.filters):
            handler.addFilter(SafeDiagnosticsFilter())
