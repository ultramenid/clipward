# Clipward logo and app icon

The emblem is a clip (a wide rounded block) cut on a slant, its two halves slid apart. It is one colour: white (or ink on light surfaces). The app icon puts the emblem on a charcoal rounded tile.

## Source and exports

- **Source:** `scripts/icon/build-logo.py`. All geometry (the clip box, cut angle, gap, slide) lives in its constants. It writes vector SVGs:
  - `resources/clipward-icon.svg`: app icon master, 1024 × 1024
  - `resources/clipward-mark.svg`: the emblem alone, for dark surfaces
  - `resources/clipward-logo.svg` and `resources/clipward-logo-light.svg`: emblem plus the Geist SemiBold wordmark, as outlines
- **App assets:** `crates/app/icons/icon.icns` (the bundle icon) and `icon.png` (the Dock icon under `cargo run`) are rasterized from the vector master; see Rebuild. `resources/clipward-icon-small.svg` (the 128px in-app icon) is checked in; regenerate it by exporting the master at 128px if the geometry changes.
- **Preview:** open `scripts/icon/icon.html`.

## Rebuild

```bash
pip install fonttools brotli uharfbuzz
python3 scripts/icon/build-logo.py
```

Then export `resources/clipward-icon.svg` to a 1024px PNG (`icon_1024.png`, any SVG tool) and rebuild the app icons:

```bash
mkdir icon.iconset
for s in 16 32 128 256 512; do
  sips -z $s $s icon_1024.png --out icon.iconset/icon_${s}x${s}.png
  sips -z $((s*2)) $((s*2)) icon_1024.png --out icon.iconset/icon_${s}x${s}@2x.png
done
iconutil -c icns icon.iconset -o crates/app/icons/icon.icns
sips -z 512 512 icon_1024.png --out crates/app/icons/icon.png
```

Change the constants in `build-logo.py`, not the generated files.
