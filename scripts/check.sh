#!/usr/bin/env bash
# The checks GitHub no longer runs: build every target and run the tests. Run
# before pushing to master and before tagging a release. This covers the host OS
# only; Windows and Linux compile errors show up in the tag's build jobs.
#
#   scripts/check.sh
set -euo pipefail
cd "$(dirname "$0")/.."
# Use the bundled FFmpeg (with libass) when staged; Homebrew's lacks libass.
if [[ -x engine-bin/ffmpeg ]]; then
  export TEST_FFMPEG="$PWD/engine-bin/ffmpeg" TEST_FFPROBE="$PWD/engine-bin/ffprobe"
fi
cargo build --workspace --all-targets
cargo test --workspace
