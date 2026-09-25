"""
Shared OpenRouter chat-completions call used by the clip planner and the
layout vision step. Normalizes errors into retryable / fatal and extracts
billed usage.
"""

import json
import logging
from typing import Any, Optional

import httpx

logger = logging.getLogger(__name__)

# HTTP statuses worth retrying (rate limits, provider outages, timeouts).
MAX_CHAT_RESPONSE_BYTES = 2 * 1024 * 1024

RETRYABLE_STATUS_CODES = {408, 429, 500, 502, 503, 504}


class OpenRouterError(Exception):
    """An OpenRouter request failed. `retryable` marks transient failures."""

    def __init__(self, message: str, retryable: bool = False):
        super().__init__(message)
        self.retryable = retryable


def json_schema_format(name: str, schema: dict[str, Any]) -> dict[str, Any]:
    """`response_format` for strict JSON-schema structured output."""
    return {
        "type": "json_schema",
        "json_schema": {"name": name, "strict": True, "schema": schema},
    }


def apply_reasoning(payload: dict[str, Any], effort: str, temperature: float = 0.2) -> None:
    """Set reasoning effort, or a temperature when reasoning is off.

    Reasoning models ignore or reject temperature, so it is only sent with
    effort "none".
    """
    if effort == "none":
        payload["temperature"] = temperature
    else:
        payload["reasoning"] = {"effort": effort, "exclude": True}


def message_text(body: dict[str, Any]) -> tuple[Optional[str], Optional[str]]:
    """Return (content, finish_reason) of the first choice."""
    choice = (body.get("choices") or [{}])[0]
    content = (choice.get("message") or {}).get("content")
    if isinstance(content, list):
        content = "".join(p.get("text", "") for p in content if isinstance(p, dict))
    return content, choice.get("finish_reason")


async def chat_completion(
    client: httpx.AsyncClient,
    payload: dict[str, Any],
) -> tuple[dict, dict]:
    """POST /chat/completions.

    Returns:
        (response_json, usage) where usage has prompt_tokens,
        completion_tokens, total_tokens and `cost` in USD (None if OpenRouter
        did not report it).

    Raises:
        OpenRouterError: `retryable=True` for rate limits, provider outages
        and network failures.
    """
    from clip_engine.config import get_settings
    if get_settings().ai_provider != "openrouter":
        from clip_engine.services.llm_gateway import complete
        return await complete(payload)
    return await post_chat_completion(client, payload)


async def post_chat_completion(client: httpx.AsyncClient, payload: dict[str, Any]) -> tuple[dict, dict]:
    """POST an OpenAI-compatible /chat/completions request to `client`'s base URL."""
    model = payload.get("model", "")
    try:
        async with client.stream(
            "POST", "/chat/completions", json=payload,
            headers={"Accept-Encoding": "identity"}, follow_redirects=False,
        ) as response:
            # Do not hand attacker-controlled compressed bodies to an unbounded
            # decompressor. The request explicitly negotiates an identity body.
            if response.headers.get("content-encoding", "identity").lower() != "identity":
                raise OpenRouterError("OpenRouter returned an unsupported response encoding")
            content = bytearray()
            async for chunk in response.aiter_raw():
                if len(chunk) > MAX_CHAT_RESPONSE_BYTES - len(content):
                    raise OpenRouterError("OpenRouter response exceeds the size limit")
                content.extend(chunk)
            status = response.status_code
    except (httpx.TimeoutException, httpx.TransportError):
        raise OpenRouterError("OpenRouter request failed", retryable=True) from None

    if status == 402:
        raise OpenRouterError(
            "OpenRouter account is out of credits. Add credits at openrouter.ai/credits."
        )
    if status != 200:
        raise OpenRouterError(
            f"OpenRouter API error ({status})",
            retryable=status in RETRYABLE_STATUS_CODES,
        )
    try:
        body = json.loads(content)
    except (ValueError, UnicodeError, RecursionError):
        raise OpenRouterError("OpenRouter returned invalid JSON") from None
    if not isinstance(body, dict):
        raise OpenRouterError("OpenRouter returned an invalid response")

    # OpenRouter can return 200 with an upstream provider error in the body.
    if body.get("error"):
        raise OpenRouterError("OpenRouter provider error", retryable=True)

    usage = body.get("usage") or {}
    cost = usage.get("cost")
    usage_data = {
        "prompt_tokens": usage.get("prompt_tokens", 0),
        "completion_tokens": usage.get("completion_tokens", 0),
        "total_tokens": usage.get("total_tokens", 0),
        "cost": float(cost) if cost is not None else None,
    }
    reasoning_tokens = (usage.get("completion_tokens_details") or {}).get("reasoning_tokens", 0)

    logger.info(
        f"OpenRouter usage ({body.get('model', model)}): "
        f"{usage_data['prompt_tokens']} prompt, "
        f"{usage_data['completion_tokens']} completion "
        f"({reasoning_tokens} reasoning), cost=${usage_data['cost'] if cost is not None else 'n/a'}"
    )
    return body, usage_data
