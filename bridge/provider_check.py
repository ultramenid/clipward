#!/usr/bin/env python3
"""Settings → AI provider → Test: one tiny structured request through the engine's gateway.

Reads the same environment as a clipping job and prints one JSON line.
"""

import asyncio
import json
import sys

import httpx


async def check() -> dict:
    from clip_engine.config import get_settings
    from clip_engine.services.openrouter import OpenRouterError, chat_completion, json_schema_format, message_text

    settings = get_settings()
    schema = {"type": "object", "properties": {"answer": {"type": "integer"}}, "required": ["answer"], "additionalProperties": False}
    payload = {
        "model": settings.ai_model or settings.planner_model,
        "messages": [{"role": "user", "content": "What is 2 + 3? Reply with JSON."}],
        "max_tokens": 2000,
        "response_format": json_schema_format("check", schema),
    }
    async with httpx.AsyncClient(
        base_url=settings.openrouter_base_url, timeout=httpx.Timeout(120.0, connect=15.0),
        headers={"Authorization": f"Bearer {settings.openrouter_api_key}"},
    ) as client:
        try:
            body, _ = await chat_completion(client, payload)
        except OpenRouterError as error:
            return {"ok": False, "message": str(error)}
    content, _ = message_text(body)
    try:
        answer = json.loads(content or "").get("answer")
    except (ValueError, AttributeError):
        answer = None
    if answer != 5:
        return {"ok": False, "message": "The provider answered, but not with valid structured JSON."}
    return {"ok": True, "message": str(body.get("model") or "")}


if __name__ == "__main__":
    try:
        result = asyncio.run(check())
    except Exception as error:  # Never print provider text that could hold secrets.
        result = {"ok": False, "message": f"Check failed ({type(error).__name__})."}
    print(json.dumps(result))
    sys.exit(0)
