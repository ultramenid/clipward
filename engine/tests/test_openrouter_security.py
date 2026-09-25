"""Exercise real httpx streaming with offline, adversarial provider replies."""
import asyncio
import gzip
import json

import httpx
import pytest

from clip_engine.services import openrouter


class Body(httpx.AsyncByteStream):
    def __init__(self, chunks):
        self.chunks = chunks
        self.closed = False
        self.reads = 0

    async def __aiter__(self):
        for chunk in self.chunks:
            self.reads += 1
            yield chunk

    async def aclose(self):
        self.closed = True


def request(body, status=200, headers=None):
    async def run():
        async def handle(req):
            assert req.headers["accept-encoding"] == "identity"
            return httpx.Response(status, stream=body, headers=headers)
        async with httpx.AsyncClient(base_url="https://example.invalid",
                                     transport=httpx.MockTransport(handle)) as client:
            return await openrouter.chat_completion(client, {"model": "test"})
    return asyncio.run(run())


def test_valid_chunked_reply_and_usage():
    raw = json.dumps({"choices": [], "usage": {"cost": 0, "prompt_tokens": 12}}).encode()
    body = Body([raw[:7], raw[7:]])
    result, usage = request(body)
    assert result["choices"] == []
    assert usage["cost"] == 0 and usage["prompt_tokens"] == 12
    assert body.closed


@pytest.mark.parametrize("status", [200, 400, 500])
@pytest.mark.parametrize("headers", [{}, {"content-length": "1"}])
def test_oversized_body_stops_reading_and_closes(monkeypatch, status, headers):
    monkeypatch.setattr(openrouter, "MAX_CHAT_RESPONSE_BYTES", 1024)
    body = Body([b"x" * 600, b"x" * 600, b"never read"])
    with pytest.raises(openrouter.OpenRouterError, match="size limit"):
        request(body, status, headers)
    assert body.closed and body.reads == 2


def test_compressed_response_is_rejected_before_decompression():
    body = Body([gzip.compress(b"x" * (4 * 1024 * 1024))])
    with pytest.raises(openrouter.OpenRouterError, match="encoding"):
        request(body, headers={"content-encoding": "gzip"})
    assert body.closed and body.reads == 0


@pytest.mark.parametrize("raw", [b"not-json private-token", b"[]", b"null"])
def test_invalid_json_and_shape_are_sanitized(raw):
    with pytest.raises(openrouter.OpenRouterError) as error:
        request(Body([raw]))
    assert "private-token" not in str(error.value)


def test_provider_error_and_redirect_are_not_followed():
    for status in [302, 429, 500]:
        with pytest.raises(openrouter.OpenRouterError) as error:
            request(Body([b"private-provider-detail"]), status,
                    {"location": "https://attacker.invalid"})
        assert "private-provider-detail" not in str(error.value)
        assert error.value.retryable == (status in {429, 500})
