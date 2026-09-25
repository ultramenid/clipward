# Releasing Clipward

Only publish from a reviewed source snapshot. Keep pre-Clipward history and the parent development workspace private.

1. Confirm ownership and redistribution rights for Clipward source, fonts, logos, model assets and bundled tools. Complete `THIRD_PARTY_NOTICES.md` and review the dependency inventories (`package-lock.json`, `Cargo.lock`).
2. From a clean checkout, run `npm ci`, `npm run typecheck`, `npm run lint`, `npm test` (renderer, release-script and `cargo test --workspace`), `npm audit --audit-level=low` and `cargo audit`. Install FFmpeg first so the engine's real-media tests run instead of skipping. Secret-scan the tree without printing secret values.
3. Stage the media tools in `engine-bin/`: an LGPL FFmpeg and ffprobe with libass (`scripts/build-ffmpeg-mac.sh`, then `scripts/stage-ffmpeg-libs-mac.sh` to bundle the caption libraries and their notices) and yt-dlp. Review `engine-bin/BUNDLED_LIBRARIES.txt` and `engine-bin/THIRD_PARTY_LICENSES/`.
4. Build on each target Mac with `npm run build` (`tauri build`). Signing and notarization need an Apple Developer ID; set `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID` as the [Tauri macOS signing guide](https://tauri.app/distribute/sign/macos/) describes. Bundles land in `target/release/bundle/`.
5. Create a protected `vX.Y.Z` tag that matches `package.json`, `src-tauri/tauri.conf.json` and the Cargo manifests. Do not move or delete release tags.
6. On clean macOS machines, install and launch each architecture, run a local clip and a provider clip, check Library and export, and confirm the glass window and appearance settings. Publish only after these checks pass.

Auto-update is not wired yet: `tauri-plugin-updater` needs an update signing key and an update endpoint. Windows distribution is blocked until a signed installer and a verified update path exist. Do not publish Windows artifacts.
