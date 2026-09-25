# Security follow-up — 24 September 2026

Historical snapshot: this follow-up predates the move of the clipping engine into `engine/` in the Clipward repository. References below to a separate legacy engine checkout and release pin describe the earlier design; see [Architecture](ARCHITECTURE.md) and [Releasing](RELEASING.md) for the current design.

This follows [the original review](SECURITY_REVIEW_2026-09-24.md). The five original findings remain fixed. Original review statements about actions not performed describe that first pass only.

## Additional changes completed

- Packaged applications now resolve Python, FFmpeg, ffprobe and yt-dlp only inside the application resources, even when a file is missing. A damaged installation fails instead of searching PATH or using a configured external Python. Packaged job preflight also rejects missing media tools before starting the worker. A regression test verifies this behavior.
- New automations default to manual captions. AI caption writing is an explicit opt-in with clear disclosure that generated text publishes without review. Existing saved settings are preserved. This limits accidental use; it does not claim to solve prompt injection or hallucinations.
- Renderer builds include full installed license texts for the seven packages actually bundled into JavaScript. The build fails if a package lacks a license text. Notices ship both with the renderer output and as `RENDERER-THIRD-PARTY-LICENSES.txt` in packaged resources.
- Source CI now scans current files and Git history with redacted Gitleaks output. Release CI also scans committed history. Weekly Dependabot configurations cover npm/Python and GitHub Actions. Remote GitHub settings still need to be applied to the chosen public repositories.
- CI explicitly installs FFmpeg so media-security tests run rather than skipping for a missing tool.
- Local resource staging now requires the exact pinned legacy engine commit before deleting/replacing resources. It stages committed files through `git archive`, excluding working-tree edits and ignored files. Release resources record `SOURCE_REVISION`; packaged verification requires it to match the reviewed pin. Regression tests cover missing and mismatched pins. Packaged Python smoke checks disable bytecode writes so they do not mutate sealed application resources.

## Verification

Desktop TypeScript and ESLint checks, the production build, 161 Node tests, 11 bridge tests, and all 21 isolated Electron end-to-end tests pass. The Node count includes two new resource-staging guards and the packaged-tool resolution regression. The renderer produces notices for all seven bundled packages. Packaging configuration passes the installed electron-builder schema validator. The clean exported desktop snapshot independently passed installation, checks, tests, build, audit and all 21 end-to-end tests; the exported legacy engine source passed 239 tests with one skip. Inventories cover 539 installed Node packages and 49 bundled Python distributions; all 49 Python distributions list notice files.

A fresh Apple silicon diagnostic package completed Developer ID signing and passed deep, strict signature verification. Bundled transcription and smart-render smoke tests passed; deep signature verification also passed afterward. Its actual Electron fuses were verified: RunAsNode, Node options and inspector switches are disabled; embedded ASAR integrity and ASAR-only loading are enabled. This diagnostic package was built with notarization explicitly disabled. It is not an approved release and is not proof of notarization, clean-machine installation, or update compatibility. Some follow-up source changes were made after it was staged.

## External facts and unresolved gates

- The separate legacy engine repository exists, is private, and contains old history. It must not simply be made public. The current account has administrative access. Branch protection and repository rulesets were absent when inspected. The proposed `clipward/clipward` repository was not accessible (GitHub returned 404); the account has administrative membership of the organization.
- Ownership/contributor permission for original code and artwork requires the maintainer's factual confirmation. Bundled third-party licensing remains separate. The renderer notice gap has been repaired. Two installed runtime packages (`lazy-val` and `@electron-internal/extract-zip`) declare MIT and BSD-2-Clause respectively but omit root license files in their installed distributions; retain/review authoritative upstream terms and bundled native notices before approving binary redistribution. No permission was invented on their behalf.
- A new engine repository destination is needed to preserve the private development history. The ownership and destination questions remain pending. No repository visibility, history, permissions, release or public source has been changed. `UNPINNED` remains in place until the reviewed engine commit is available at the agreed public destination.
- A Developer ID Application signing identity is available locally, but notarization credentials were not available in the process environment. A protected release environment with the required credentials/reviewers must be configured on the final repository. Do not paste credentials into chat.
- Clean Intel/Apple silicon installation and signed-update tests require the actual final artifacts and suitable machines/runners. Windows remains unsupported for distribution.
- Native decoder isolation is still a separate architectural task. The app is not claiming an OS sandbox around FFmpeg/Python, and cannot guarantee absence of codec vulnerabilities. Protocol/format restrictions, fixed packaged runtimes and signed resources reduce exposure but do not replace OS isolation.

No public publication or change to an existing private repository is needed to review these local changes. Do not treat this document as ownership sign-off or final binary-release approval.

## Concurrent migration boundary

After the checks completed, independent edits appeared in the working tree migrating the engine to `clip_engine` and transcription to OpenRouter. Those edits are not part of the tested snapshots. They were preserved, not overwritten. The checks in this report apply to the frozen `release-review-20260924` snapshots; do not apply their release-readiness claims to the changing working tree. The migration needs a fresh review after its scope and implementation stabilize.
