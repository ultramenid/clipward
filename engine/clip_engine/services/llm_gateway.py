"""
Routes the engine's OpenAI-style chat payloads to the AI provider chosen in
Settings: a direct API key (Anthropic, OpenAI) or a logged-in coding CLI
(Claude Code, Codex, Antigravity, Grok, Cursor, OpenCode, Gemini).

Callers build one OpenRouter-shaped payload and read one OpenAI-shaped body
back, so the planner and layout vision don't know which provider answered.
CLIs are text only: image parts are dropped and the bridge turns vision off.
"""

import asyncio
import json
import logging
import os
import sys
import tempfile
from typing import Any, Optional

import httpx

from clip_engine.config import get_settings

logger = logging.getLogger(__name__)

# Planning a multi-hour transcript through an agent CLI can take many minutes.
CLI_TIMEOUT_SECONDS = 20 * 60
MAX_CLI_OUTPUT_BYTES = 8 * 1024 * 1024

DEFAULT_MODELS = {"anthropic": "claude-opus-5", "openai": "gpt-6-sol"}

# Keys the provider APIs don't accept; OpenRouter-only routing options.
_OPENROUTER_ONLY = ("plugins", "provider", "models", "reasoning")


class ProviderError(Exception):
    """Raised as OpenRouterError by callers; kept separate to avoid an import cycle."""


def provider_supports_images(provider: str) -> bool:
    return not provider.startswith("cli:")


async def complete(payload: dict[str, Any]) -> tuple[dict, dict]:
    """Send a chat payload to the configured non-OpenRouter provider."""
    from clip_engine.services.openrouter import OpenRouterError, post_chat_completion

    settings = get_settings()
    provider = settings.ai_provider
    if provider == "openai":
        if not settings.openai_api_key:
            raise OpenRouterError("OpenAI API key is missing (invalid api key)")
        body = {k: v for k, v in payload.items() if k not in _OPENROUTER_ONLY}
        body["model"] = settings.ai_model or DEFAULT_MODELS["openai"]
        if "max_tokens" in body:
            body["max_completion_tokens"] = body.pop("max_tokens")
        async with httpx.AsyncClient(
            base_url="https://api.openai.com/v1",
            timeout=httpx.Timeout(600.0, connect=30.0),
            headers={"Authorization": f"Bearer {settings.openai_api_key}"},
        ) as client:
            return await post_chat_completion(client, body)
    if provider == "anthropic":
        return await _anthropic(payload, settings)
    if provider.startswith("cli:"):
        return await _cli(provider[4:], payload, settings)
    raise OpenRouterError(f"Unknown AI provider: {provider}")


# ---------------------------------------------------------------------------
# Anthropic (BYOK)
# ---------------------------------------------------------------------------

def _anthropic_content(content: Any) -> list[dict]:
    if isinstance(content, str):
        return [{"type": "text", "text": content}]
    blocks = []
    for part in content:
        if part.get("type") == "text":
            blocks.append({"type": "text", "text": part["text"]})
        elif part.get("type") == "image_url":
            url = part["image_url"]["url"]
            header, _, data = url.partition(",")
            media_type = header.removeprefix("data:").split(";")[0] or "image/jpeg"
            blocks.append({"type": "image", "source": {"type": "base64", "media_type": media_type, "data": data}})
    return blocks


async def _anthropic(payload: dict[str, Any], settings) -> tuple[dict, dict]:
    import anthropic
    from clip_engine.services.openrouter import OpenRouterError

    if not settings.anthropic_api_key:
        raise OpenRouterError("Anthropic API key is missing (invalid api key)")
    system = "\n\n".join(m["content"] for m in payload["messages"] if m["role"] == "system" and isinstance(m["content"], str))
    messages = [
        {"role": m["role"], "content": _anthropic_content(m["content"])}
        for m in payload["messages"] if m["role"] != "system"
    ]
    request: dict[str, Any] = {
        "model": settings.ai_model or DEFAULT_MODELS["anthropic"],
        "max_tokens": max(16000, payload.get("max_tokens") or 0),
        "messages": messages,
    }
    if system:
        request["system"] = system
    schema = ((payload.get("response_format") or {}).get("json_schema") or {}).get("schema")
    if schema:
        request["output_config"] = {"format": {"type": "json_schema", "schema": schema}}

    client = anthropic.AsyncAnthropic(api_key=settings.anthropic_api_key, max_retries=2)
    try:
        # Streaming: long transcripts and 32k output would hit HTTP timeouts otherwise.
        async with client.messages.stream(**request) as stream:
            message = await stream.get_final_message()
    except anthropic.AuthenticationError:
        raise OpenRouterError("Anthropic rejected the API key (invalid api key)") from None
    except anthropic.RateLimitError:
        raise OpenRouterError("Anthropic rate limit reached", retryable=True) from None
    except anthropic.APIStatusError as e:
        raise OpenRouterError(f"Anthropic API error ({e.status_code})", retryable=e.status_code >= 500) from None
    except anthropic.APIConnectionError:
        raise OpenRouterError("Anthropic connection failed", retryable=True) from None
    finally:
        await client.close()

    if message.stop_reason == "refusal":
        raise OpenRouterError("The model declined this request")
    text = "".join(block.text for block in message.content if block.type == "text")
    finish = "length" if message.stop_reason == "max_tokens" else "stop"
    usage = {
        "prompt_tokens": message.usage.input_tokens,
        "completion_tokens": message.usage.output_tokens,
        "total_tokens": message.usage.input_tokens + message.usage.output_tokens,
        "cost": None,
    }
    return _body(message.model, text, finish), usage


# ---------------------------------------------------------------------------
# Logged-in CLIs
# ---------------------------------------------------------------------------

# id -> label. Detection and paths live in the desktop app (src/main/ai-providers.ts).
CLI_PROVIDERS = {
    "claude": "Claude Code",
    "codex": "Codex",
    "agy": "Antigravity",
    "grok": "Grok",
    "cursor-agent": "Cursor",
    "opencode": "OpenCode",
    "gemini": "Gemini CLI",
}


def _prompt_text(payload: dict[str, Any]) -> str:
    parts = []
    for message in payload["messages"]:
        content = message["content"]
        if isinstance(content, str):
            parts.append(content)
        else:
            parts.extend(p["text"] for p in content if p.get("type") == "text")
    return "\n\n".join(parts)


def _cli_command(cli_id: str, path: str, prompt: str, schema: Optional[dict], model: str, work: str) -> tuple[list[str], Optional[str]]:
    """Command line and stdin for one non-interactive, tool-free call."""
    schema_json = json.dumps(schema) if schema else None
    if cli_id == "claude":
        cmd = [path, "--print", "--output-format", "json", "--tools", "", "--no-session-persistence"]
        if schema_json:
            cmd += ["--json-schema", schema_json]
        if model:
            cmd += ["--model", model]
        return cmd, prompt
    if cli_id == "codex":
        cmd = [path, "exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only", "-o", os.path.join(work, "last.txt")]
        if schema_json:
            schema_path = os.path.join(work, "schema.json")
            with open(schema_path, "w", encoding="utf-8") as f:
                f.write(schema_json)
            cmd += ["--output-schema", schema_path]
        if model:
            cmd += ["-m", model]
        return cmd + ["-"], prompt
    if cli_id == "agy":
        # ponytail: agy reads the prompt only from argv; Windows caps a command
        # line at ~32k chars, so long transcripts need macOS/Linux there.
        cmd = [path, f"-p={prompt}", "--output-format", "json"]
        if schema_json:
            cmd += ["--json-schema", schema_json]
        if model:
            cmd += ["--model", model]
        return cmd, None
    if cli_id == "grok":
        prompt_path = os.path.join(work, "prompt.txt")
        with open(prompt_path, "w", encoding="utf-8") as f:
            f.write(prompt)
        cmd = [path, "--prompt-file", prompt_path, "--output-format", "json", "--no-subagents", "--disable-web-search"]
        if schema_json:
            cmd += ["--json-schema", schema_json]
        if model:
            cmd += ["-m", model]
        return cmd, None
    # The remaining CLIs have no schema flag; the schema goes in the prompt.
    if schema_json:
        prompt += f"\n\nRespond with a single JSON object that matches this JSON Schema, and nothing else:\n{schema_json}"
    if cli_id == "cursor-agent":
        return [path, "-p", "--output-format", "json", "--mode", "ask", "--trust", "--model", model or "auto"], prompt
    if cli_id == "opencode":
        return [path, "run", "--format", "json", *(["-m", model] if model else [])], prompt
    if cli_id == "gemini":
        return [path, "-p", "", "--output-format", "json", *(["-m", model] if model else [])], prompt
    raise ValueError(f"Unsupported CLI: {cli_id}")


def _parse_cli_output(cli_id: str, stdout: str, work: str) -> str:
    """The CLI's final answer text."""
    if cli_id == "codex":
        with open(os.path.join(work, "last.txt"), encoding="utf-8") as f:
            return f.read()
    if cli_id == "opencode":
        text = []
        for line in stdout.splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if event.get("type") == "text":
                text.append((event.get("part") or {}).get("text", ""))
        return "".join(text)
    data = json.loads(stdout)
    if cli_id == "claude" and data.get("is_error"):
        raise ProviderError(str(data.get("result") or "Claude Code returned an error"))
    structured = data.get("structured_output") or data.get("structuredOutput")
    if structured is not None:
        return json.dumps(structured)
    return data.get("result") or data.get("response") or data.get("text") or ""


async def _cli(cli_id: str, payload: dict[str, Any], settings) -> tuple[dict, dict]:
    from clip_engine.services.openrouter import OpenRouterError

    label = CLI_PROVIDERS.get(cli_id, cli_id)
    path = settings.ai_cli_path
    if cli_id not in CLI_PROVIDERS or not path or not os.path.isfile(path):
        raise OpenRouterError(f"{label} CLI was not found. Check Settings → AI provider.")
    schema = ((payload.get("response_format") or {}).get("json_schema") or {}).get("schema")
    prompt = _prompt_text(payload)
    with tempfile.TemporaryDirectory(prefix="clipward-cli-") as work:
        cmd, stdin = _cli_command(cli_id, path, prompt, schema, settings.ai_model, work)
        logger.info("Calling %s CLI for planning (%d prompt chars)", label, len(prompt))
        try:
            proc = await asyncio.create_subprocess_exec(
                *cmd, cwd=work,
                stdin=asyncio.subprocess.PIPE if stdin is not None else asyncio.subprocess.DEVNULL,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
                # Keep Windows from flashing a console for each call.
                creationflags=0x08000000 if sys.platform == "win32" else 0,
            )
        except OSError:
            raise OpenRouterError(f"{label} CLI could not be started") from None
        try:
            out, err = await asyncio.wait_for(
                proc.communicate(stdin.encode("utf-8") if stdin is not None else None),
                timeout=CLI_TIMEOUT_SECONDS,
            )
        except asyncio.TimeoutError:
            proc.kill()
            raise OpenRouterError(f"{label} CLI timed out", retryable=True) from None
        if len(out) > MAX_CLI_OUTPUT_BYTES:
            raise OpenRouterError(f"{label} CLI output exceeds the size limit")
        if proc.returncode != 0:
            # stderr can hold account details; log only its size.
            logger.warning("%s CLI exited with %s (%d stderr bytes)", label, proc.returncode, len(err))
            lowered = (out + err).decode("utf-8", "replace").lower()
            if any(marker in lowered for marker in ("not logged in", "login", "unauthorized", "authenticate")):
                raise OpenRouterError(f"{label} CLI is not logged in (unauthorized)")
            raise OpenRouterError(f"{label} CLI failed (exit {proc.returncode})", retryable=True)
        try:
            text = _parse_cli_output(cli_id, out.decode("utf-8", "replace"), work)
        except (ValueError, OSError, ProviderError) as e:
            raise OpenRouterError(f"{label} CLI returned an unusable response: {type(e).__name__}", retryable=True) from None
    # CLIs bill the user's own plan; their reported costs are list-price estimates.
    usage = {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0, "cost": 0.0}
    return _body(f"{cli_id} cli" + (f" ({settings.ai_model})" if settings.ai_model else ""), text, "stop"), usage


def _body(model: str, text: str, finish_reason: str) -> dict:
    return {"model": model, "choices": [{"message": {"content": text}, "finish_reason": finish_reason}]}
