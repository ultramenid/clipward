# Transcription and AI providers

## Offline transcription (Whisper)

Transcription runs on this computer with [whisper.cpp](https://github.com/ggerganov/whisper.cpp), through the `pywhispercpp` bindings (prebuilt for macOS arm64, Windows and Linux; Intel Macs build from source). Audio never leaves the machine.

The ggml model is downloaded once from `huggingface.co/ggerganov/whisper.cpp` in **Settings → Transcription** or the first-run card on Create. Jobs cannot start until the selected model is on disk. Downloads go to a `.part` file in the app data folder (`whisper-models/`) and are renamed only after the byte count matches, so an interrupted download is never mistaken for a model. The choices are Base, Small, Large v3 Turbo (compact, the default), Medium and Large v3 Turbo; see `src/shared/whisper-models.ts`.

The engine runs whisper.cpp with token timestamps and one word per segment (`max_len=1`, `split_on_word`), drops non-speech markers such as `[BLANK_AUDIO]`, shifts word times onto the source timeline, and groups words into sentences (a pause over two seconds also ends a segment). Custom vocabulary from Settings goes in as the initial prompt. There are no speaker labels. A decoder failure fails the job with `transcription.whisper_failed`, and a missing model fails with `transcription.whisper_model_missing`. Neither falls back to visual-only planning; only a video with no audio track does that.

## Clip planning (AI provider)

**Settings → AI provider** chooses who reads the transcript and picks the clips:

- **A logged-in coding CLI**, billed to that CLI's plan: Claude Code (`claude`), Codex (`codex`), Antigravity (`agy`), Grok (`grok`), Cursor (`cursor-agent`), OpenCode (`opencode`) or Gemini CLI (`gemini`). The app finds them on `PATH` and in the usual per-user install folders. Each runs non-interactively in an empty temporary folder, with tools off or read-only where the CLI allows, and the clip-plan JSON schema is enforced by the CLI's own schema flag when it has one. CLIs are text only, so layout vision is off and silent videos cannot be planned. Runs report $0 because the CLI bills the user's plan.
- **Your own API key** for OpenRouter, Anthropic or OpenAI. Only the selected provider's key reaches the engine. An `ANTHROPIC_API_KEY` is never passed to a CLI, so Claude Code keeps using the logged-in plan.
- **Local AI**: your own OpenAI-compatible server, such as Ollama, LM Studio, the llama.cpp server or vLLM. See below.

### Local AI (Ollama, LM Studio, llama.cpp, vLLM)

Clip planning can run fully offline on your own hardware. With Ollama:

```sh
ollama pull qwen2.5:14b    # any chat model works; bigger models plan better
```

Keep Ollama running, choose **Local AI** in **Settings → AI provider** (or the first-run card), keep the default URL `http://localhost:11434/v1`, and pick the model. **Detect models** lists what the server offers (`GET {url}/models`, plus Ollama's `/api/tags` when the URL is on port 11434); you can also type a name. LM Studio's server is usually `http://localhost:1234/v1`; llama.cpp's `llama-server` and vLLM serve the same OpenAI-compatible API under `/v1`.

- **Settings.** The server URL (`localBaseUrl`, http or https; loopback, LAN and `.local` hosts are fine; no credentials, query or fragment) and the model (`localModel`, required) are plain settings. **Model accepts images** (`localImages`, off by default) sends video frames to vision models such as `qwen2.5vl` or `gemma3`: it enables visual-only planning of silent videos and the layout vision check. An optional key, for servers started with one, is stored in the keychain as `localApiKey` and only passed to the engine when Local AI is selected. The provider counts as ready once a URL and model are set; **Test connection** sends one tiny structured request.
- **Traffic.** Planning and layout-vision requests go only to that server (`POST {url}/chat/completions`), with any system proxy bypassed for loopback and LAN hosts. Nothing else is contacted, and runs report $0.
- **Structured output.** The planner sends the same prompts as for hosted providers. The clip-plan schema goes as a JSON-schema `response_format`; when the server refuses that, the engine retries with `{"type": "json_object"}` and then with the schema written into the system prompt, and pulls the JSON out of the reply (reasoning blocks and code fences are skipped). The format a server accepts is remembered for the session.
- **Speed and size.** Local models are slow, so a request may take up to 20 minutes (10 s to connect). Output caps above 8192 tokens are left to the server. If the transcript does not fit the model's context, the server's error is shown as "The local model's context window is too small": pick a model with a larger context, raise the server's context length, or set a shorter start and end time. Ollama shortens prompts to its context length without an error, so for long videos raise it (for example `OLLAMA_CONTEXT_LENGTH=32768 ollama serve`); the engine logs a warning when the server read far fewer prompt tokens than it was sent.
- **Errors.** An unreachable server, a model the server doesn't have, a context overflow, a model that doesn't fit in memory and a timeout each map to a fixed message and hint in `pipeline::describe_failure`.

**Test connection** sends one tiny structured-output request through the same engine gateway a job uses (`clip_engine::llm`).

Every planner and layout-vision request goes through `clip_engine::llm` (`crates/engine/src/llm.rs`), which routes it to the selected provider. The **campaign brief** from the Create wizard is added to the planner prompt as hard requirements. Clips that break a rule are left out, so a strict brief can return fewer clips.

Tests: `crates/engine/src/transcribe.rs` (word timing, grouping, failures), `crates/engine/src/planner/tests.rs` (prompts, brief and parsing), `crates/engine/src/llm.rs` (provider requests against a local mock server, including Local AI's schema fallback, unreachable server, missing model and context overflow), and `src-tauri/src/settings.rs`, `ai_providers.rs` and `validation.rs` (provider key isolation, Local AI settings, job config).
