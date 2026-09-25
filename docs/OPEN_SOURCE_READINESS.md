# Open-source readiness

Clipward keeps its desktop clipping engine, model, fonts, and locked Python dependencies in `engine/`. A user needs only this source repository, the documented runtimes, and their own provider key. The former separate engine repository is not part of the development or release path.

From the Clipward checkout, run `bash scripts/export-public-draft.sh /path/to/new-draft-directory` with a new path outside this checkout. The script checks an exact [file manifest](../scripts/public-draft-manifest.txt), rejects unreviewed files and symlinks in the selected source directories, and creates one `clipward/` draft without the former development history, local configuration, or build output. Add new source files to the manifest only after reviewing them. Review and secret-scan the **exact exported bytes** before any public push; the manifest checks file paths, not their contents.

## Publication gates

| Order | Gate | Completion evidence |
| --- | --- | --- |
| 1 | Confirm ownership and redistribution rights for source, branding, fonts, model, and bundled runtimes. | Maintainer sign-off and completed `THIRD_PARTY_NOTICES.md`. |
| 2 | Validate the exact exported draft before any public push. | Clipward typecheck, lint, tests, build, Electron tests, in-repo engine pytest, Python dependency audit, npm audit, and a redacted secret scan pass from the draft. |
| 3 | Publish the reviewed Clipward draft. Keep older development history private and enable repository secret scanning and push protection where available. | The first public commit contains only the scanned draft and CI passes without another source repository. |
| 4 | Build, sign, and notarize both macOS architectures in release CI. | Dependency inventories, archive checksums, updater metadata, signatures, and packaged engine checks pass. |
| 5 | Test installation and update on clean Apple silicon and Intel machines with a local file and controlled provider run. | Create, Library, export, cancellation, and update behavior are recorded for each architecture. |

Windows source builds remain experimental. A Windows release requires its own runtime staging, process cancellation, packaging, and update tests.

## Maintenance plan after the public snapshot

| Priority | Area | Next change | Acceptance check |
| --- | --- | --- | --- |
| 1 | `src/main/pipeline-runner.ts` | Split tool discovery, process lifecycle, and JSON-lines protocol parsing into focused modules. | Cancellation and malformed-message tests still pass. |
| 2 | Settings and IPC | Keep saved provider keys confined to main-process secure storage. | Renderer APIs expose configured status, never stored key values. |
| 3 | Python worker and `engine/` | Document and version the result schema shared within this repository. | Older valid runs load; malformed or partial outputs fail safely. |
| 4 | Frontend | Split large Create, Settings, and posting dialogs into focused components. | Keyboard flow, error recovery, and responsive checks pass. |
| 5 | Supply chain | Refresh pinned runtimes and dependency inventories. | Every release records exact source revision, archive hashes, and bundled notices. |

The [release guide](RELEASING.md) contains verification steps. Historical reviews describe the former separate engine layout; [the September 2026 security review](SECURITY_REVIEW_2026-09-24.md) remains a record of that work.
