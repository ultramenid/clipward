#!/usr/bin/env bash
# Points vendor/whisper-rs-sys at a whisper.cpp release (the latest by default).
# whisper-rs-sys on crates.io lags upstream, so the workspace patches it with this
# copy; see vendor/whisper-rs-sys/CLIPWARD_PATCH.md. Bindings are generated at
# build time, so a version bump is just new sources plus a rebuild.
set -euo pipefail

tag="${1:-$(curl -fsSL https://api.github.com/repos/ggml-org/whisper.cpp/releases/latest \
  | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')}"
[[ -n "$tag" ]] || { echo "Could not find the latest whisper.cpp release" >&2; exit 1; }

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/vendor/whisper-rs-sys/whisper.cpp"
work="$(mktemp -d -t clipward-whisper)"
trap 'rm -rf "$work"' EXIT

curl -fsSL "https://github.com/ggml-org/whisper.cpp/archive/refs/tags/${tag}.tar.gz" | tar xz -C "$work"
src="$(echo "$work"/whisper.cpp-*)"

# The same subset whisper-rs-sys publishes: what CMake needs to build the library.
rm -rf "$dest"
mkdir -p "$dest"
cp -R "$src"/{CMakeLists.txt,LICENSE,cmake,ggml,include,src} "$dest"/
# Only the CPU, Metal and BLAS backends are built (engine Cargo.toml); CMake adds
# a backend's folder only when its GGML_<NAME> option is on, so drop the rest.
find "$dest/ggml/src" -mindepth 1 -maxdepth 1 -type d -name 'ggml-*' \
  ! -name ggml-cpu ! -name ggml-metal ! -name ggml-blas -exec rm -rf {} +
mkdir -p "$dest/bindings/javascript"
cp "$src/bindings/javascript/package-tmpl.json" "$dest/bindings/javascript/" 2>/dev/null || true

sed -n "s/^set(WHISPER_VERSION_\([A-Z]*\) \(.*\))/\1 \2/p" "$dest/CMakeLists.txt" | head -3
echo "Now: cargo build, then re-run the transcription bench before shipping."
