#!/usr/bin/env bash
# Stage the media tools in engine-bin/ for a release build, pinned by SHA-256.
#
#   scripts/stage-tools.sh <macos|windows|linux> [output dir, default engine-bin]
#
# All: the standalone yt-dlp. Windows and Linux: BtbN's static LGPL FFmpeg 8.1.3
# build (libass included, for burned captions) with its licence and the matching
# FFmpeg source. macOS builds FFmpeg from source instead (build-ffmpeg-mac.sh).
set -euo pipefail

os="${1:?usage: stage-tools.sh <macos|windows|linux> [dir]}"
out="${2:-engine-bin}"

ytdlp_version=2026.08.19
btbn_tag=autobuild-2026-10-08-13-05
btbn_build=ffmpeg-n8.1.3-14-g330caae0c1
ffmpeg_commit=330caae0c1

case "$os" in
  macos)   ytdlp=(yt-dlp_macos 0f192b7ec147ab6288885d6351d9ab67367640029b4377576ef46dd79cf7b202 yt-dlp) ;;
  linux)   ytdlp=(yt-dlp_linux 58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a yt-dlp)
           ffmpeg=("$btbn_build-linux64-lgpl-8.1.tar.xz" efc5f0db8b76a2b2dd0bc26feb5ffce38dc252af21fd60457ec6ece62797c930) ;;
  windows) ytdlp=(yt-dlp.exe 66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a yt-dlp.exe)
           ffmpeg=("$btbn_build-win64-lgpl-8.1.zip" 293bc44ded938a6900060b7d622dc330675fdec0f028f8b2ea6735c64be02b12) ;;
  *) echo "Unknown OS: $os" >&2; exit 2 ;;
esac

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

fetch() { # <url> <sha256> <file>
  curl --fail --show-error --location --retry 3 --silent "$1" -o "$3"
  echo "$2  $3" | sha256sum --check --quiet 2>/dev/null || echo "$2  $3" | shasum -a 256 --check --quiet
}

fetch "https://github.com/yt-dlp/yt-dlp/releases/download/$ytdlp_version/${ytdlp[0]}" "${ytdlp[1]}" "$out/${ytdlp[2]}"
chmod 755 "$out/${ytdlp[2]}"

[[ "$os" == macos ]] && exit 0

archive="$work/${ffmpeg[0]}"
fetch "https://github.com/BtbN/FFmpeg-Builds/releases/download/$btbn_tag/${ffmpeg[0]}" "${ffmpeg[1]}" "$archive"
if [[ "$archive" == *.zip ]]; then
  # Windows runner: 7-Zip is a native program, so hand it Windows paths.
  7z x -bso0 -o"$(cygpath -w "$work")" "$(cygpath -w "$archive")"
else
  tar xJf "$archive" -C "$work"
fi
build="$work/${ffmpeg[0]%.zip}"
build="${build%.tar.xz}"
exe=""
[[ "$os" == windows ]] && exe=.exe
cp "$build/bin/ffmpeg$exe" "$build/bin/ffprobe$exe" "$out/"
chmod 755 "$out/ffmpeg$exe" "$out/ffprobe$exe"
cp "$build/LICENSE.txt" "$out/FFMPEG-LICENSE"

# LGPL: ship the exact FFmpeg source the build came from.
curl --fail --show-error --location --retry 3 --silent \
  "https://github.com/FFmpeg/FFmpeg/archive/$ffmpeg_commit.tar.gz" -o "$out/FFMPEG-SOURCE.tar.gz"
cat > "$out/FFMPEG-SOURCE.txt" <<EOF
FFmpeg 8.1.3 (release/8.1 at $ffmpeg_commit), built by BtbN/FFmpeg-Builds
release $btbn_tag: https://github.com/BtbN/FFmpeg-Builds/releases/tag/$btbn_tag
The FFmpeg source is included as FFMPEG-SOURCE.tar.gz; the build scripts and the
sources of the libraries linked in (libass, FreeType, FriBidi, HarfBuzz, ...) are at
https://github.com/BtbN/FFmpeg-Builds. LGPL variant: GPL and nonfree parts disabled.
EOF

configuration="$("$out/ffmpeg$exe" -hide_banner -version | sed -n '2,4p')"
for forbidden in --enable-gpl --enable-nonfree; do
  [[ "$configuration" != *"$forbidden"* ]] || { echo "Forbidden $forbidden" >&2; exit 1; }
done
for filter in ass crop overlay pad setsar gblur lutyuv vstack concat; do
  "$out/ffmpeg$exe" -hide_banner -filters 2>/dev/null | grep -E "[[:space:]]${filter}[[:space:]]" >/dev/null || {
    echo "Missing FFmpeg filter: $filter" >&2; exit 1;
  }
done
echo "Staged $os tools in $out"
