# Security review — 24 September 2026

Historical snapshot: this review predates the move of the clipping engine into `engine/` in the Clipward repository. References below to a separate legacy engine checkout and release pin describe the earlier design; see [Architecture](ARCHITECTURE.md) and [Releasing](RELEASING.md) for the current design.

## Verdict

Confirmed issues found during this review have been patched locally. This is **not a release approval or a guarantee that the project has no vulnerabilities**. Publish a reviewed clean source snapshot; do not make the existing development repository/history public by changing its visibility. The legacy engine revision is still `UNPINNED`, so the release workflow deliberately refuses to build a release until maintainers publish and pin the reviewed engine commit.

The checkout already contained extensive staged, unstaged and untracked work. Findings below describe the working tree reviewed here, not a published version or a claim about who introduced a change. No commits, pushes, releases, real social posts, credential rotations, or signing operations were performed.

## Scope and threat model

Reviewed the desktop Electron main/preload/renderer trust boundaries, IPC registration and validation, local-file protocol, file authorization/export, secure settings and migration, logging, process execution and cancellation, Python bridge, public-network validation, social connection callbacks, provider clients/uploads, account and post caches, automation scheduling/metadata, updater configuration, packaging and CI/release workflows, source export, dependency manifests, and the adjacent static Next.js website.

Also inspected the staged legacy engine's download/network/media paths and patched the corresponding source checkout of the legacy engine (a separate repository). Changes to its downloader, transcription, visual sampling, layout analysis and rendering must ship with Clipward. The ignored staged engine was patched as well for local smoke tests; it is not the source of truth. The complete legacy engine test suite ran, but this was not a comprehensive audit of its separately deployed API infrastructure.

Threats considered: malicious imported/downloaded media, hostile provider responses and redirects, compromised renderer calls, local files/symlinks, accidental credential publication, and release supply-chain mistakes. No production accounts or private media were used. No exploitation of remote services was attempted.

## Findings fixed

| ID | Severity / prerequisites | Finding and impact | Fix and evidence |
| --- | --- | --- | --- |
| BC-01 | High; user imports or downloads attacker-controlled media | Extension checks and a local-protocol allowlist did not stop FFmpeg from recognizing a playlist disguised as video and opening another local media file. Automation conversion and posting probes also lacked protocol restrictions. A harmless fixture reproduced conversion of a separate recording; pipeline processing could disclose unselected media to configured AI providers or include it in output. | Allow only supported video demuxers at desktop and legacy engine source-media entry points; block network protocols in automation conversion and posting probes. Real FFmpeg regression rejects disguised playlists, accepts ordinary video, and confirms no transcription request is made. Thumbnail regression also rejects the fixture. |
| BC-02 | Medium; an authenticated provider endpoint returns a redirect | Automation requests followed redirects. A 307/308 can replay private audio/transcripts; the custom `xi-api-key` header is not protected by the standard cross-origin Authorization-header stripping behavior. | Both automation provider calls use `redirect: 'error'`. Two local HTTP servers verify that redirected transcription/metadata calls fail and the destination receives no request. Production provider URLs remain fixed. |
| BC-03 | Low; malformed or compromised provider/storage response | Automation checked size only after buffering an entire response. Zernio API and upload confirmation bodies were also unbounded. This could exhaust main-process memory. | Shared streaming reader caps decoded bytes before buffering, cancels oversized bodies, and applies limits to automation, Zernio API and storage responses. A never-ending mocked response verifies cancellation after crossing the limit. Existing request deadlines still apply. |
| BC-04 | Low; attacker can create files in the app's user-data directory | Account/post caches wrote through predictable `.tmp` paths without exclusive creation. A pre-created symlink could overwrite another writable file with cache JSON. This is not a remote privilege escalation; it requires local filesystem access. | Random temporary filenames with exclusive `wx` creation and owner-only permissions. Regression verifies an existing predictable symlink's target stays unchanged. |
| BC-05 | Low; renderer abuse plus an application bundle/alias in the output library | The shell-opening guard checked the supplied directory name for `.app`/`.bundle`, so a symlink with an ordinary folder name could bypass it and launch the target bundle. | Canonicalize the path before both checking and opening it. The IPC regression verifies that an aliased application bundle is rejected before a shell launch. |

## Release hardening and readiness repairs

- Configured production Electron fuses to disable RunAsNode, Node options and inspector entry points, require ASAR loading, and enable embedded ASAR integrity validation. The packaged macOS verification script now checks the actual fuse bytes. These are defense-in-depth changes, not evidence of an independently demonstrated remote exploit. ASAR integrity does not cover the separately bundled Python engine and binaries.
- Corrected `mac.notarize` from an obsolete object to the boolean required by installed electron-builder 26.15.3. The protected release workflow already supplies `APPLE_TEAM_ID` and notarization credentials. Configuration validation now passes; real signing/notarization still needs release CI.
- Updated the exact public-export manifest for the newer jobs, automations, tests, and transcription smoke test. Added an optional legacy engine source-directory argument so relocated checkouts can be exported. The unused old `src/main/callback-server.ts` is explicitly excluded; the active Zernio callback implementation remains included. The exporter still rejects unexpected files and symlinks.

## Existing controls verified by inspection/tests

- Renderer sandboxing, context isolation, disabled Node integration, restrictive CSP, blocked navigation/webviews/popups, and denied permission requests.
- IPC sender/main-frame checks; API keys stay in the main process, with configured-status flags exposed to the renderer. Key storage refuses insecure fallback when an OS keychain is unavailable.
- Media paths use authorization and canonical containment; the local-file reader/upload/export paths open checked handles. Export destinations use exclusive creation. These controls do not make arbitrary native decoders an OS sandbox.
- Zernio browser links are constrained, API redirects are rejected, callback listeners bind loopback and check host/path, and connection flows use random callback paths. Upload DNS is checked at socket creation rather than only at initial URL validation.
- Python worker environment is restricted; progress/result parsing is bounded and sanitized. Public-network socket guards and engine download policies are present.
- Release actions are pinned, ordinary CI has read-only permissions, signing uses a protected environment, downloaded runtime/source archives have hashes, Python dependencies are hash locked, and release publication produces a draft.
- The website builds as static pages, with no application API routes or user-controlled HTML execution found in the reviewed source.

## Verification

- Desktop: TypeScript checks, ESLint, production build and full unit/integration suites pass; 158 Node tests plus 11 Python bridge tests.
- Electron: 21 end-to-end tests pass using isolated app data, generated clips and mocked providers.
- Legacy engine source: 239 tests pass, one skipped (see test output for environmental skip); generated-media regression included.
- Staged bundled engine: transcription extraction and smart-render smoke tests pass against the bundled FFmpeg.
- Website: ESLint and production build pass.
- Packaging: installed electron-builder configuration validator passes; modified shell scripts parse successfully. A newly signed/notarized package was not built or tested during this review.
- npm registry audits: zero reported vulnerabilities for both application lockfiles, including development dependencies.
- Python lock audit: 48 staged legacy engine dependencies checked with pip-audit 2.10.1; zero reported vulnerabilities. This does not audit the Python interpreter, FFmpeg, OpenCV's native code, or every bundled library.
- Gitleaks: current application/website source, available Clipward Git history, and the exported Clipward/legacy engine source snapshot returned zero findings. Pattern scanners cannot prove absence of secrets. Ignored local environments, caches, user data, build output and binaries must stay out of the public source snapshot.

## Remaining publication/release gates

1. Publish the reviewed legacy engine snapshot and replace the engine source pin file with its exact public commit SHA. Do not pin the old source before incorporating the media fixes. The current `UNPINNED` value remains intentionally unchanged.
2. Review ownership and redistribution rights for both projects, branding, fonts, model weights, and bundled runtimes, using the existing third-party notices. An MIT license file does not establish ownership or replace third-party licenses. This review does not provide legal sign-off.
3. Scan and validate the exact source snapshot and final release artifacts after any further changes. Enable private vulnerability reporting, secret scanning/push protection, branch/tag protections, and protected release-environment reviewers in GitHub. Remote repository settings were not inspected or changed.
4. Build signed and notarized macOS artifacts in release CI; check the new fuse assertions, signatures, artifact inventories and updater metadata. Test installation and an update on clean Apple silicon and Intel machines. Windows remains experimental and was not tested here.
5. Native media parsers still process untrusted bytes with the user's filesystem privileges. Format/protocol restrictions reduce exposure but are not process sandboxing or a proof against codec vulnerabilities. Maintain the runtime/native dependency update process; an OS-constrained media worker is a worthwhile future isolation project.
6. AI metadata grounding checks are heuristic. Treat automatic posting as permission to publish model-generated text; transcript prompt injection and inaccurate claims cannot be completely excluded by the current validators. Review-sensitive workflows should use manual metadata.

## References

- [Electron security recommendations](https://www.electronjs.org/docs/latest/tutorial/security)
- [Electron fuses](https://www.electronjs.org/docs/latest/tutorial/fuses)
- [FFmpeg protocol restrictions](https://www.ffmpeg.org/ffmpeg-protocols.html)

The tests and reports are evidence for the specific code and conditions reviewed, not an exhaustive proof of security or a substitute for independent penetration testing.
