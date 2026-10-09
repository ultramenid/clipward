# Porting the desktop shell from Tauri + React to GPUI

Branch `gpui-rewrite`. The Tauri shell, React UI and npm toolchain are removed; the
old sources are on `master` (`git show master:src/renderer/...`).

## Layout

| Crate | Role |
| --- | --- |
| `crates/engine` | Clipping engine (unchanged). |
| `crates/core` | App logic: settings, job queue, library, editor previews, path checks. `Core` methods in `api.rs` are the old Tauri commands minus their dialogs. |
| `crates/app` | GPUI shell (binary `clipward`). |

It uses the Tauri shell's folders (`AppPaths::resolve`, bundle id `com.clipward.desktop`),
so settings, models and the library carry over.

## Decisions

- **GPUI through `gpui-kit` 0.7.1** from crates.io (GPUI plus a component kit: inputs,
  selects, sliders, dialogs, tooltips, the Lucide icon set in `gpui-kit-assets`).
- **Video:** macOS plays through AVFoundation (`AVPlayer` + `AVPlayerItemVideoOutput`),
  painting each `CVPixelBuffer` with GPUI's `surface` element: hardware decode, no
  copies, audio in sync for free. Engine-rendered preview chunks and the joined preview
  are plain mp4 files, so the editor's hybrid player maps onto it.
- **Voiceover:** recorded natively (`recorder.rs`, `AVAudioRecorder`, mono AAC .m4a),
  then `Core::project_import_recording`. No echo cancellation (that needs
  `AVAudioEngine` voice processing).
- **Events:** `jobs:update` becomes the `Core::start` callback; whisper and export
  progress are per-call callbacks.
- Pure TS logic (`timeline.ts`, `overlays.ts`, `manual-clips.ts`, `color.ts`,
  `appearance.ts`, `utils.ts`, `job-output.ts`, `video-source.ts`) is ported to Rust
  with its 64 Node test cases ported alongside, as the parity oracle.
- **Redraw cost:** a GPUI frame re-renders every view on the path from the window
  root to whatever changed; only `.cached(style)` views are skipped. So anything that
  ticks (the player, the playhead line, the clock, job timers, the arcade) is its own
  small view, and big static parts (the editor's tracks and side panel, the Jobs
  lists) are cached. A cached view is laid out by the style its parent passes, not by
  its content: give it an explicit size (`Timeline::height`, the 288px inspector).
  Measured with the editor probe (debug build, 40 s clip): 3.1 ms per playback frame;
  25 ms when every view is forced to redraw.
- **Interface scale:** gpui-kit's root sets the window's rem to its theme's
  `font_size` on every frame, so the scale goes through `Tokens::rem` →
  `theme.font_size` (`Tokens::install`); a bare `window.set_rem_size` is overwritten.

## Parity checklist

Status: `[x]` done and checked (tests, or a snapshot of the real thing), `[~]` built
and compiling but not yet tried by hand (drags, clicks, typing, real devices),
`[ ]` todo.

### Foundation
- [x] `crates/core` extracted; Tauri shell runs on it
- [x] GPUI builds on this machine (Command Line Tools only)
- [x] Window: overlay title bar, traffic lights, min 720×520, 1280×800
- [~] Window glass / blur radius, opaque fallback
- [x] Theme tokens from `globals.css` (dark/light/system, hue, saturation, lightness, accent)
- [x] Fonts: Geist / Geist Mono bundled; caption preview fonts
- [x] Icons (gpui-kit's Lucide set; names differ a little, e.g. `trash` for Trash2)
- [~] UI scale (rem fixed to apply; non-100% not yet looked at), menu, ⌘0/⌘=/⌘- zoom
- [~] Single instance, quit waits for running jobs
- [x] UI kit (ui.rs + gpui-kit components)

### Shell
- [x] Sidebar (220px / 52px rail, <1024px rail), nav + shortcuts ⌘1/2/3/, and ⌘\
- [x] Active jobs card (shared stage labels), setup status footer, title bar breadcrumb
- [~] Boot: settings loading / error + Retry; tool check on start

### Create (JobForm wizard)
- [x] All steps, source picker, trim, tiles, clips, caption picker, branding, review
      (snapshots of each step; submitting a real job not tried from the GPUI app)

### Jobs
- [x] Active list, previous list, filters, search, 10 s polling, confirm dialog
- [x] JobProgress, JobFailure, JobCancelled (snapshots with fake jobs:
      `CLIPWARD_SNAPSHOT_JOB=list|running|queued|failed|cancelled`)
- [~] Arcade (3 games): drawn; not played by hand. Pauses when the window is inactive.

### Library
- [x] Run cards, search, skeletons, empty states
- [~] ClipList + ClipCard (hover video, selection, bulk export, sort), RunStats

### Editor
- [x] Load, read-only callout, warm-up overlay with Skip, header (save state, undo,
      redo, export), narrow-window panel toggle
- [x] Player: exact chunks + joined preview, instant source preview, overlays, text
      layers; plays in real time (probe: 15.5 s of edit in 16.6 s)
- [~] Player drags: text, overlays, PiP, crop pan, corner resize
- [x] Timeline drawn: ruler, lanes, caption words, filmstrip, waveform, playhead,
      auto-fit zoom
- [~] Timeline input: seek, hover scrub, ⌘wheel/pinch zoom, every drag and trim,
      reorder, word editing
- [x] Side panel drawn: brief, piece inspector, overlays list, captions panel
- [~] Side panel controls, text/overlay/PiP/sound inspectors (no such items in the
      test clip)
- [~] Autosave, undo/redo (100 steps, batched drags), export with progress
- [~] Media / audio / logo import (native pickers), voiceover (denied-permission path
      and the encoder verified; a real take needs mic access)
- [~] Keyboard: Space, ⌘Z/⇧⌘Z, S, T, Delete, ←/→ (Shift) — skipped while a text
      field has focus

### Settings
- [x] Every section drawn (snapshots); [~] interactions (downloads, key tests)

### Release
- [x] `.app` bundle: `scripts/bundle-macos.sh` (Info.plist with mic usage, icon,
      `engine-bin/`, assets, audio-input entitlement); a `--debug` bundle boots and
      finds every tool
- [~] Signing/notarization (script + docs/RELEASING.md; needs a release machine)
- [ ] `engine-bin/yt-dlp` is a Homebrew Python wrapper:
      stage a self-contained one
- [x] Remove Tauri, React, npm toolchain

### Checking the editor without a window

`cargo build -p clipward_app --features snapshot`, then
`CLIPWARD_SNAPSHOT=/tmp/snap CLIPWARD_SNAPSHOT_EDITOR=<run folder> ./target/debug/clipward-app`
writes `/tmp/snap/editor.png` (hidden window, nothing saved). Options in
`snapshot.rs::editor_probe`: `CLIPWARD_SNAPSHOT_CLIP`, `_SELECT=piece:0|text:<id>|…`,
`_AT=<ms>`, `_WIDTH=<px>`, `_SECS`, `_WARMUP=1`, `_PLAY=1|full` (prints playback
progress and frame times).

## Porting a page (conventions)

- Source of truth is the React file on `master`; port behaviour, copy and states exactly
  (empty, loading, error, disabled reasons, tooltips, keyboard shortcuts).
- Sizes in CSS pixels go through `ui::u(px)` (rems), so Interface scale works.
  Type scale: `.text_body()`, `.text_caption()`… (`ui::TypeScale`). Colours only
  from `cx.tokens()` (`theme::Tokens`): `t.ink`, `t.ink_secondary`, `t.tint(0.05)`
  for `bg-content/5`, `t.stroke`, `t.selection`, `t.accent`, status hues.
- Primitives live in `ui.rs` (`Btn`, `panel`, `panel_header`, `page_header`,
  `badge`, `icon_tile`, `callout`, `setting_row`, `segmented`, `switch`,
  `checkbox`, `progress_bar`, `progress_ring`, `status_dot`, `skeleton`,
  `empty_state`, `icon("lucide-name")`). Text fields, selects, sliders and dialogs
  use gpui-kit (`gpui_kit::component::{input, select, slider, dialog}`), styled
  with tokens.
- Backend calls: `state::core(cx)` for sync methods; async ones through
  `state::run(cx, async move { core.x().await })` awaited inside `cx.spawn`.
  Dialogs and opening files: `shell.rs`. Shared state: `models::{Settings, Jobs}`.
  Page switches: `workspace::go(Page::Jobs, window, cx)`.
- Pure logic goes in `crates/core` with unit tests; views stay thin.
- Check visually: `cargo build -p clipward_app --no-default-features --features
  "page-x snapshot"` then `CLIPWARD_SNAPSHOT=/tmp/snap ./target/debug/clipward-app`
  writes `/tmp/snap/<page>.png`.
