# Contributing to Clipward

Thanks for helping improve Clipward. For a substantial change, open an issue first to discuss the user problem and proposed behavior.

## Set up

Follow the [development instructions](README.md#develop): install the Rust and Tauri prerequisites, then run `npm ci` and `npm run dev`. Keep provider keys in the app's Settings; tests do not need real keys.

## Make a change

- Keep the Tauri shell's authority (settings, keys, file access, jobs), the renderer UI and the engine's responsibilities separate. Validate data at every command, subprocess and saved-file boundary.
- Keep provider keys out of renderer state, logs, test fixtures, and issue reports. Use placeholders in examples.
- Update the README or architecture guide when setup, provider data flow, or supported behavior changes.
- Add a regression test for a bug or a new boundary. Avoid tests that only repeat implementation details.

Run `npm run typecheck`, `npm run lint`, `npm test` (which includes `cargo test --workspace`) and `cargo clippy --workspace --tests` before opening a pull request. Install FFmpeg so the engine's real-media tests run instead of skipping. Describe behavior, tests, and any user-visible screenshots or sample outputs in the PR.

Use an imperative, scoped commit message such as `fix(clips): validate saved run output`. By contributing, you agree to follow the [code of conduct](CODE_OF_CONDUCT.md).
