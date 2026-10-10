# Releasing Clipward

Only publish from a reviewed source snapshot. Keep pre-Clipward history and the parent development workspace private.

The desktop app (`crates/app`) is packaged by `scripts/bundle-macos.sh` as `Clipward.app`, bundle id `com.clipward.desktop`. Builds from the old Tauri shell used the same id and resource layout, so settings, library and models carry over.

## Build

1. Confirm ownership and redistribution rights for Clipward source, fonts, logos, model assets and bundled tools. Complete `THIRD_PARTY_NOTICES.md` and review the dependency inventory (`Cargo.lock`).
2. From a clean checkout, run `cargo test --workspace`, `cargo clippy --workspace --tests` and `cargo audit`. Install FFmpeg first so the engine's real-media tests run instead of skipping. Secret-scan the tree without printing secret values.
3. Stage the media tools in `engine-bin/`: an LGPL FFmpeg and ffprobe with libass and yt-dlp. On macOS, `scripts/build-ffmpeg-mac.sh` (it runs `scripts/stage-ffmpeg-libs-mac.sh` to bundle the caption libraries and their notices), then `scripts/stage-tools.sh macos` for yt-dlp. On Windows and Linux, `scripts/stage-tools.sh windows|linux` downloads the pinned BtbN static LGPL FFmpeg 8.1.3 (libass built in), its licence, the matching FFmpeg source and yt-dlp, all checked by SHA-256. Review `engine-bin/BUNDLED_LIBRARIES.txt`, `engine-bin/THIRD_PARTY_LICENSES/` and `engine-bin/FFMPEG-SOURCE.txt`.

4. Build on each target Mac with `scripts/bundle-macos.sh --dmg`. It runs `cargo build --release -p clipward_app` and writes `target/bundle/Clipward.app` and `target/bundle/Clipward_<version>_<arch>.dmg`:
   - `Contents/MacOS/clipward`: the `clipward-app` binary.
   - `Contents/Info.plist`: from `crates/app/macos/Info.plist`, with the version from `crates/app/Cargo.toml` and `LSMinimumSystemVersion` from the binary's deployment target (11.0 on Apple silicon, never below 10.13).
   - `Contents/Resources/icon.icns`, `engine-bin/`, `assets/fonts/`, `assets/models/`. The app treats any binary inside `Contents/MacOS` as packaged and reads tools and assets from `Contents/Resources` (`bundle_resources()` in `crates/app/src/main.rs`, `AppPaths`).
5. Signing:
   - `APPLE_SIGNING_IDENTITY`: a Developer ID Application identity in the keychain. The script signs every Mach-O in `Contents/Resources` (ffmpeg, ffprobe, their dylibs), then the app, with the hardened runtime, a secure timestamp and `crates/app/macos/entitlements.plist` (`com.apple.security.device.audio-input`, which voiceover recording needs). Unset, the bundle gets an ad-hoc signature and runs only on the Mac that built it.
   - Notarization: `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password) and `APPLE_TEAM_ID`, or an App Store Connect key via `APPLE_API_KEY`, `APPLE_API_ISSUER` and `APPLE_API_KEY_PATH`. With an identity and either set, the app is submitted with `notarytool`, then stapled; with `--dmg` the signed DMG is notarized and stapled too. A rejected submission prints the notary log and fails the script.
   - On CI, import the certificate into a keychain before running the script.
6. Check the result: `codesign --verify --strict --deep target/bundle/Clipward.app`, `spctl -a -vv target/bundle/Clipward.app` (expects `source=Notarized Developer ID`) and `xcrun stapler validate target/bundle/Clipward.app`.

Windows and Linux: `cargo build --release -p clipward_app`, then `scripts/bundle.sh windows|linux` writes `target/bundle/Clipward_<version>_<os>_<arch>.zip` (Windows) or `.tar.gz` (Linux): `Clipward/clipward[.exe]` with `resources/engine-bin/` and `resources/assets/` beside it, which the app treats as packaged. Linux build packages: `scripts/linux-deps.sh`. The editor's preview plays through the bundled ffmpeg there (`crates/app/src/video_ffmpeg.rs`; `--features portable-video` runs it on macOS for testing). Voiceover recording is macOS-only for now.

For local checks without a release build, `scripts/bundle-macos.sh --debug` bundles `target/debug/clipward-app` (`--no-build` packages the binary already built; `--features snapshot` plus `CLIPWARD_SNAPSHOT=<dir>` renders every page off screen and quits). The log (`~/Library/Logs/com.clipward.desktop/clipward.log`) should show `app.boot` with `"isPackaged":true` and a `system.checkTools` line with every tool `true`.

## CI

`.github/workflows/ci.yml` is the only workflow. It builds and tests the workspace on macOS, Windows and Linux for every pull request and push to `master`, except a commit whose message starts with `Release ` (the release commit; its tag run tests it). Pushing a `vX.Y.Z` tag runs the tests, then stages the tools, builds the macOS DMG (arm64), the Windows zip and the Linux tarball (x64), and attaches them to a **draft** GitHub release. macOS signing and notarization use these repository secrets when set: `APPLE_CERTIFICATE` (base64 .p12), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, and `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8` (the .p8 contents). Without them the DMG is ad-hoc signed.

## Publishing

7. Create a protected `vX.Y.Z` tag that matches the Cargo manifests (including `crates/app/Cargo.toml`). Do not move or delete release tags.
8. On clean macOS machines, install and launch each architecture, run a local clip and a provider clip, check Library and export, and confirm the glass window and appearance settings. Also record a voiceover in the editor and confirm macOS asks for microphone access. Publish only after these checks pass.

On clean Windows and Linux machines, unpack the archive, launch `clipward`, and run the same checks (local clip, provider clip, Library, export, editor playback with sound). The Windows build is unsigned, so SmartScreen warns on first launch; add Authenticode signing before a wide release.

Auto-update: the app reads `releases/latest` from GitHub (published releases only, so drafts are never offered), downloads its platform's package, checks it against `SHA256SUMS.txt` (the release workflow writes it; a release without it is not offered), and swaps it in (`crates/core/src/updater.rs`). Keep the package names from `scripts/bundle*.sh`: the updater finds its package by the name's ending (`_aarch64.dmg`, `_windows_x64.zip`, `_linux_x64.tar.gz`), and the archives must keep the `Clipward.app` / `Clipward/` top level. Packages are still unsigned; the updater doesn't need signing because a file the app downloads itself is not quarantined.
