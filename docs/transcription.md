# Transcription and AI providers

## Offline transcription (Whisper)

Transcription runs on this computer with [whisper.cpp](https://github.com/ggerganov/whisper.cpp), through the `pywhispercpp` bindings (prebuilt for macOS arm64, Windows and Linux; Intel Macs build from source). Audio never leaves the machine.

The ggml model is downloaded once from `huggingface.co/ggerganov/whisper.cpp` in **Settings → Transcription** or the first-run card on Create. Jobs cannot start until the selected model is on disk. Downloads go to a `.part` file in the app data folder (`whisper-models/`) and are renamed only after the byte count matches, so an interrupted download is never mistaken for a model. The choices are Base, Small, Large v3 Turbo (compact, the default), Medium and Large v3 Turbo; see `src/shared/whisper-models.ts`.

The engine runs whisper.cpp with token timestamps and one word per segment (`max_len=1`, `split_on_word`), drops non-speech markers such as `[BLANK_AUDIO]`, shifts word times onto the source timeline, and groups words into sentences (a pause over two seconds also ends a segment). Custom vocabulary from Settings goes in as the initial prompt. There are no speaker labels. A decoder failure fails the job with `transcription.whisper_failed`, and a missing model fails with `transcription.whisper_model_missing`. Neither falls back to visual-only planning; only a video with no audio track does that.

## Clip planning (AI provider)

**Settings → AI provider** chooses who reads the transcript and picks the clips:

- **A logged-in coding CLI**, billed to that CLI's plan: Claude Code (`claude`), Codex (`codex`), Antigravity (`agy`), Grok (`grok`), Cursor (`cursor-agent`), OpenCode (`opencode`) or Gemini CLI (`gemini`). The app finds them on `PATH` and in the usual per-user install folders. Each runs non-interactively in an empty temporary folder, with tools off or read-only where the CLI allows, and the clip-plan JSON schema is enforced by the CLI's own schema flag when it has one. CLIs are text only, so layout vision is off and silent videos cannot be planned. Runs report $0 because the CLI bills the user's plan.
- **Your own API key** for OpenRouter, Anthropic or OpenAI. Only the selected provider's key reaches the engine. An `ANTHROPIC_API_KEY` is never passed to a CLI, so Claude Code keeps using the logged-in plan.

**Test connection** sends one tiny structured-output request through the same engine gateway a job uses (`bridge/provider_check.py`).

Every planner and layout-vision request still goes through `chat_completion` in `engine/clip_engine/services/openrouter.py`, and `llm_gateway.py` routes it to the selected provider. The **campaign brief** from the Create wizard is added to the planner prompt as hard requirements. Clips that break a rule are left out, so a strict brief can return fewer clips.

Tests: `engine/tests/test_transcription.py` (word timing, grouping, failures), `bridge/test_bridge_runner.py` (provider, Whisper and brief validation) and `tests/main/security.test.cjs` (provider key isolation, job config).
