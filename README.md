<p align="center">
  <img src="resources/clipward-logo-light.svg#gh-light-mode-only" alt="Clipward" height="56" />
  <img src="resources/clipward-logo.svg#gh-dark-mode-only" alt="Clipward" height="56" />
</p>

<h3 align="center">Clips for content-reward campaigns, made on your own computer.</h3>

<p align="center">
  Paste a YouTube, Twitch or other video link, or upload a file, add the campaign's clip requirements,
  and Clipward finds the moments that fit the brief, cuts them to 9:16 or 16:9, and burns in word-by-word captions.
  Transcription runs offline with Whisper; clip picking uses the AI CLI you're already logged in to, or your own API key.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License" /></a>
  <a href="https://github.com/ultramenid/clipward/releases"><img src="https://img.shields.io/github/v/release/ultramenid/clipward?label=download" alt="Latest release" /></a>
  <a href="https://www.clipward.ai/discord"><img src="https://img.shields.io/badge/Discord-builders-5865F2?logo=discord&logoColor=white" alt="Discord" /></a>
</p>

---

## Why Clipward?

- **Built for clippers.** Paste the campaign brief (brand to mention, topics, tone, banned content) and every clip must follow it. Clips that break a rule are left out, not padded in.
- **Offline transcription.** Whisper (whisper.cpp) runs on your computer. Download a model once; your audio never leaves the machine.
- **Use the AI you already pay for.** Clipward detects Claude Code, Codex, Antigravity, Grok, Cursor, OpenCode and Gemini CLI and plans clips with your logged-in plan. You can also use your own OpenRouter, Anthropic or OpenAI key.
- **Or run it fully offline.** Point Clipward at your own model server (Ollama, LM Studio, llama.cpp server, vLLM or any OpenAI-compatible server) and clip planning never leaves your computer or network. Nothing is billed.
- **No Clipward account or backend.** Your videos and keys never pass through a Clipward server.
- **Captions that look native.** Nine styles (Viral, Hormozi, Bold, Clean, Minimal, Fire, Glow, Neon, Karaoke), each with a live preview before you render.
- **MIT licensed.** Fork it, change it, ship it.

## How it works

```
 Source video ──▶ Download ──▶ Transcribe ──▶ Find moments ──▶ Render
 (file or link)    yt-dlp      Whisper          your AI CLI      FFmpeg
                               (offline,        or API key,      crop, captions,
                               word timings)    follows brief    one file per clip
```

Create is a short wizard: **Video** (link or file, optional trim) → **Brief** (campaign requirements) → **Format** → **Clips** (lengths and count) → **Captions** → **Review**. The brief stays filled in for the next video, since you usually cut several videos for one campaign.

Already know the moments? In the **Clips** step, choose **I'll choose the moments** and type each clip's start and end (seconds, `m:ss` or `h:mm:ss`) with an optional title: up to 50 clips of 3 seconds to 15 minutes, overlaps allowed. For a local file, a preview player can set a clip's start or end from its current position. Clipward skips the brief and the AI planner, transcribes only the span your clips cover, and frames, captions and renders each one at exactly the times you typed. Manual mode needs no AI provider, only the Whisper model.

Every run gets its own folder. The **Library** shows completed clips with virality scores, timecodes and tags. **Jobs** shows what is running or queued (up to two runs at once; more wait in a queue) and every earlier run. See [transcription and AI providers](docs/transcription.md).

In **Create → Format → Video speed**, choose **1×** (normal), **1.1×**, **1.25×**, **1.5×**, **1.75×**, or **2×** for every clip in the job. Exports preserve voice pitch and keep captions synchronized. Speed works with **Cut dead air** and appears in Review and the saved results. Clip lengths and source trim times refer to the original footage: a 60-second clip at 1.5× exports in about 40 seconds, before any dead-air cuts. The choice stays selected when you clip another video in the same session. Existing exports stay as they are; generate a new job to change their speed.

## Download

Get the latest version from [Releases](https://github.com/ultramenid/clipward/releases): a DMG for Apple silicon Macs, a zip for Windows (x64) and a tarball for Linux (x64). Each bundles FFmpeg and yt-dlp. The builds aren't code-signed yet, so macOS and Windows warn on first launch; the release page has the steps to open them. After that, Clipward updates itself: it checks when it opens, and **Settings → Help & about → Update and restart** installs the new version.

On first launch, the setup card on Create asks for two things:

1. **The AI that picks the clips.** Choose a detected CLI you're logged in to (for example `claude` or `codex`), pick OpenRouter, Anthropic or OpenAI and paste a key, or choose **Local AI** and enter your server's URL and model (for Ollama: `ollama pull qwen2.5:14b`, keep Ollama open, and keep the default URL `http://localhost:11434/v1`). **Test connection** sends one tiny request to confirm it works.
2. **A Whisper model.** Large v3 Turbo (compact, 574 MB) is recommended; Base and Small are faster, Medium and Large v3 Turbo are more accurate. Change it later in **Settings → Transcription**.

Keys are stored in your operating system's keychain, and only the selected provider's key is passed to the clipping engine. A local server needs no key; if yours asks for one, it is kept in the keychain too.

### What leaves your computer

For a link, the app downloads the source using your network connection. Audio never leaves your computer: Whisper transcribes it locally. The transcript text and your campaign brief go to the AI provider you chose, through its CLI or its API. With an API-key provider and Smart framing's AI vision check on, a few video frames also go to that provider. CLI providers are text only. With **Local AI**, planning requests go only to the server URL you entered (usually on this computer or your local network) and nowhere else; frames are sent to it only if you turn on **Model accepts images**. With a local video file and Local AI on this computer, transcription, planning and rendering need no internet connection after the one-time downloads. The one-time Whisper model download comes from Hugging Face. Provider accounts, charges, retention and data policies are governed by those services.

Downloads and intermediate media are held in a private `work/` directory under Clipward’s per-user application data folder. Clipward removes job work on completion, failure, and cancellation, and clears stale work when it next starts after a forced shutdown. A local video you selected stays where it was. Rendered clips, the transcript, plan and `result.json` remain in a run folder under your chosen **Output folder** (by default, `~/Clipward`). Delete the run folder to remove those local outputs. Settings, downloaded Whisper models and diagnostic logs live in the per-user application data and logs folders.

Only download or clip material you have permission to use, and follow each campaign's rules. Remote sites may limit downloads or change their access rules.

### Clip a Twitch VOD

Paste a public, completed Twitch video link such as `https://www.twitch.tv/videos/1234567890` into Create, then choose your clip settings and generate. Clipward downloads the saved video and uses the same transcription, AI moment selection and rendering flow as other sources. Links on `twitch.tv`, `www.twitch.tv`, `m.twitch.tv` and `go.twitch.tv` are accepted and normalized to the canonical video URL.

Live channels, Twitch clips, collections, subscriber-only videos and deleted or expired VODs are not supported. No Twitch login or cookies are used. The original source must be at most six hours and 20 GB. Clipward downloads the full source before applying the optional start and end times; a link's timestamp or tracking parameters are ignored. For a longer source, trim a downloaded file before adding it. Downloads also stop after four hours or when less than 1 GB of free space would remain.

## Develop

Clipward is a Rust app: a [GPUI](https://www.gpui.rs) desktop shell over a clipping engine.

**Prerequisites:**
- A stable Rust toolchain (`rustup`).
- A C/C++ toolchain and CMake, which whisper.cpp needs to build. On macOS the Xcode Command Line Tools are enough.
- FFmpeg with the libass-backed `ass` filter for captions.
- yt-dlp for link sources.

Clipward uses FFmpeg and yt-dlp from `engine-bin/` when that folder exists, and otherwise from your `PATH`. Provider keys are only needed for live jobs, not for tests.

```bash
git clone https://github.com/ultramenid/clipward
cd clipward
cargo run -p clipward_app
```

**Settings → System check** shows the FFmpeg, FFprobe, yt-dlp, smart framing and engine checks. The engine's fonts and face detection model are in `crates/engine/assets/` and are bundled with the app.

To check that FFmpeg has the caption filter: `ffmpeg -hide_banner -filters | grep -E '[[:space:]]ass[[:space:]]'`. On macOS, `brew install ffmpeg-full` includes libass; on Arch, `sudo pacman -S ffmpeg yt-dlp`.

`scripts/bundle-macos.sh` builds `target/bundle/Clipward.app` (`--dmg` for a disk image). Distributable macOS builds also need signing credentials; see [the release guide](docs/RELEASING.md).

### First run and troubleshooting

1. In the setup card, pick an AI provider (a logged-in CLI, an API key or Local AI) and download a Whisper model. If a CLI isn't detected, run it once in a terminal to sign in, then choose **Detect again**.
2. Run **Settings → System check**. Captions need an FFmpeg build with libass (`brew install ffmpeg-full` on macOS, or stage one in `engine-bin/`).
3. Paste a video link or choose a file, paste the campaign brief, pick clip lengths, framing and caption style, then generate. Smart framing follows faces and arranges screen shares with facecams shot by shot. Completed runs appear in Library and in your output folder.
4. If a link fails, check it in a signed-out browser, or download the video yourself and select the local file. If a run fails, start with the in-app error and System check. Logs intentionally leave out raw provider responses and private source details.

| Command | What it does |
| --- | --- |
| `cargo run -p clipward_app` | Run the app |
| `scripts/check.sh` | Build every target and run the tests (GitHub only builds releases). Engine tests that need FFmpeg skip when it isn't installed |
| `cargo clippy --workspace --tests` | Lint |
| `scripts/bundle-macos.sh` | Build and bundle `Clipward.app` (see [the release guide](docs/RELEASING.md)) |

### Project layout

```
crates/app/       GPUI desktop app: window, menu, pages (Create, Library, Jobs, Settings), editor
crates/core/      App logic: settings, job queue, library, Whisper downloads, path checks
crates/engine/    Rust clipping engine: download, transcribe, plan, layout, render, pipeline
crates/engine/assets/  Caption fonts and the YuNet face detection model
scripts/          Release helpers and icon/logo generators
```

The visual system (tokens, components and rules) is documented in [DESIGN.md](DESIGN.md).
The desktop trust boundaries and the engine pipeline are described in [Architecture](docs/ARCHITECTURE.md).
The [open-source readiness checklist](docs/OPEN_SOURCE_READINESS.md) tracks the remaining release gates and maintenance priorities.

## Contributing

Issues and pull requests are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) for setup, review expectations and checks. Security reports should follow [SECURITY.md](SECURITY.md), not a public issue. The community guidelines are in [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## License

[MIT](LICENSE) © Clipward
