#!/usr/bin/env bash
# Regenerates the Clipward app icon: build/icon.png, build/icon.icns,
# build/icon.ico and resources/clipward-icon-small.svg.
# Source: resources/clipward-icon.svg (vector, from build-logo.py). macOS only (iconutil).
#
#   bash scripts/icon/build-icons.sh
#
# Rendered from the vector master by Chromium (Electron), then resized.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

"$ROOT/node_modules/.bin/electron" "$ROOT/scripts/icon/render.js" "$WORK"

# The in-app 48px icon: a 128px raster keeps it sharp on Retina displays.
printf '<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128" role="img" aria-label="Clipward">\n  <title>Clipward</title>\n  <image width="128" height="128" href="data:image/png;base64,%s"/>\n</svg>\n' "$(base64 < "$WORK/icon-128.png" | tr -d '\n')" > "$ROOT/resources/clipward-icon-small.svg"

resize() {
  local size=$1 out=$2
  cp "$WORK/icon-$size.png" "$out"
}

resize 1024 "$ROOT/build/icon.png"

ICONSET="$WORK/Clipward.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  resize "$size" "$ICONSET/icon_${size}x${size}.png"
  resize $((size * 2)) "$ICONSET/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$ICONSET" -o "$ROOT/build/icon.icns"

# Windows .ico with PNG-compressed entries (supported since Vista).
for size in 16 24 32 48 64 128 256; do
  resize "$size" "$WORK/ico-$size.png"
done
WORK="$WORK" OUT="$ROOT/build/icon.ico" node -e '
const fs = require("fs")
const sizes = [16, 24, 32, 48, 64, 128, 256]
const images = sizes.map((s) => fs.readFileSync(`${process.env.WORK}/ico-${s}.png`))
const header = Buffer.alloc(6)
header.writeUInt16LE(0, 0)
header.writeUInt16LE(1, 2)
header.writeUInt16LE(sizes.length, 4)
let offset = 6 + 16 * sizes.length
const entries = sizes.map((s, i) => {
  const e = Buffer.alloc(16)
  e.writeUInt8(s >= 256 ? 0 : s, 0)
  e.writeUInt8(s >= 256 ? 0 : s, 1)
  e.writeUInt16LE(1, 4)
  e.writeUInt16LE(32, 6)
  e.writeUInt32LE(images[i].length, 8)
  e.writeUInt32LE(offset, 12)
  offset += images[i].length
  return e
})
fs.writeFileSync(process.env.OUT, Buffer.concat([header, ...entries, ...images]))
'

echo "Icons written to build/ and resources/"
