#!/usr/bin/env bash
# Stage the media tools in engine-bin/ for a release build, pinned by SHA-256.
#
#   scripts/stage-tools.sh <macos|windows|linux> [output dir, default engine-bin]
#
# All: yt-dlp's onedir build (yt-dlp plus _internal/). The single-file build unpacks
# Python on every launch, which macOS scans each time (~7 s per call); onedir starts
# in ~0.3 s once the first launch is checked. Windows and Linux: BtbN's static LGPL FFmpeg 8.1.3
# build (libass included, for burned captions) with its licence and the matching
# FFmpeg source. macOS builds FFmpeg from source instead (build-ffmpeg-mac.sh).
set -euo pipefail

os="${1:?usage: stage-tools.sh <macos|windows|linux> [dir]}"
out="${2:-engine-bin}"

ytdlp_version=2026.10.26
# shortcut: BtbN deletes a daily autobuild after about two weeks, so this pin 404s then; re-pin
# to a newer autobuild (hashes in its checksums.sha256). Month-end autobuilds are kept far
# longer: prefer the newest of those when it is not older than this pin. Mirror the archives
# if it keeps biting.
btbn_tag=autobuild-2026-11-26-12-56
btbn_build=ffmpeg-n8.1.3-41-ga8029b6528
ffmpeg_commit=a8029b6528

case "$os" in
  macos)   ytdlp=(yt-dlp_macos 20d98fa608998eff87d1d35a6e9fb65e4b874639a6cc89addc19da3becde3ef1 yt-dlp) ;;
  linux)   ytdlp=(yt-dlp_linux f34053415136e55073dd64ba5326d161ecc23ce0e98c081d8752867b37809450 yt-dlp)
           ffmpeg=("$btbn_build-linux64-lgpl-8.1.tar.xz" 313b6e52c31a7ce5db69fabe8fa04f6ef4b83d8c9cb365a0c8ed2a0349016948) ;;
  windows) ytdlp=(yt-dlp fd51b212cf8380c7a52ee08c0d6be7f3c59b2f2c832eaf7b45acfae6eab57419 yt-dlp.exe)
           ffmpeg=("$btbn_build-win64-lgpl-8.1.zip" 5eb24166e7f7103dcc12fa6d773674df9491005e78e8069c131e65b3661343da) ;;
  *) echo "Unknown OS: $os" >&2; exit 2 ;;
esac

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

fetch() { # <url> <sha256> <file>
  curl --fail --show-error --location --retry 3 --silent "$1" -o "$3"
  echo "$2  $3" | sha256sum --check --quiet 2>/dev/null || echo "$2  $3" | shasum -a 256 --check --quiet
}

unzip_to() { # <zip> <dir>
  if [[ "$os" == windows ]]; then
    # Windows runner: 7-Zip is a native program, so hand it Windows paths.
    7z x -bso0 -y -o"$(cygpath -w "$2")" "$(cygpath -w "$1")"
  else
    unzip -q -o "$1" -d "$2"
  fi
}

# The release zips are named yt-dlp_macos.zip, yt-dlp_linux.zip and yt-dlp_win.zip.
zip_name="${ytdlp[0]}.zip"
[[ "$os" == windows ]] && zip_name=yt-dlp_win.zip
fetch "https://github.com/yt-dlp/yt-dlp/releases/download/$ytdlp_version/$zip_name" "${ytdlp[1]}" "$work/ytdlp.zip"
rm -rf "$out/_internal" "$out/${ytdlp[2]}"
unzip_to "$work/ytdlp.zip" "$out"
[[ "$os" == windows ]] || mv "$out/${ytdlp[0]}" "$out/${ytdlp[2]}"
if [[ "$os" == macos ]]; then
  # The zip stores Python.framework's symlinks as copies, which codesign rejects
  # ("bundle format is ambiguous"); put the links back.
  fw="$out/_internal/Python.framework"
  version="$(ls "$fw/Versions" | grep -v Current)"
  rm -rf "$fw/Versions/Current" "$fw/Python" "$fw/Resources" "$out/_internal/Python"
  ln -s "$version" "$fw/Versions/Current"
  ln -s Versions/Current/Python "$fw/Python"
  ln -s Versions/Current/Resources "$fw/Resources"
  ln -s "Python.framework/Versions/$version/Python" "$out/_internal/Python"
fi
chmod 755 "$out/${ytdlp[2]}"
"$out/${ytdlp[2]}" --version >/dev/null

[[ "$os" == macos ]] && exit 0

archive="$work/${ffmpeg[0]}"
fetch "https://github.com/BtbN/FFmpeg-Builds/releases/download/$btbn_tag/${ffmpeg[0]}" "${ffmpeg[1]}" "$archive"
if [[ "$archive" == *.zip ]]; then
  unzip_to "$archive" "$work"
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
