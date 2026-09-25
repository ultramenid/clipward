# Clipward logo and app icon

The emblem is a play symbol cut in two, with the front piece pushed forward: a clip, moving forward. The rear piece is white (or ink on light surfaces) and the front piece uses the app accent gradient, `#5C8FFF` to `#38CCFF`. The app icon puts the emblem on a charcoal rounded tile.

## Source and exports

- **Source:** `scripts/icon/build-logo.py`. All geometry (triangle, cut angle, gap, forward shift) lives in its constants. It writes vector SVGs:
  - `resources/clipward-icon.svg`: app icon master, 1024 × 1024
  - `resources/clipward-mark.svg`: the emblem alone, for dark surfaces
  - `resources/clipward-logo.svg` and `resources/clipward-logo-light.svg`: emblem plus the Geist SemiBold wordmark, as outlines
- **App assets:** the Tauri bundle icons in `src-tauri/icons/` (PNG sizes, `icon.icns`, `icon.ico`) are generated from the vector master by `npm run icons` (`tauri icon`). `resources/clipward-icon-small.svg` (the 128px in-app icon) is checked in; regenerate it by exporting the master at 128px if the geometry changes.
- **Preview:** open `scripts/icon/icon.html`.

## Rebuild

```bash
pip install fonttools brotli uharfbuzz
python3 scripts/icon/build-logo.py
npm run icons   # tauri icon: writes src-tauri/icons/
```

Change the constants in `build-logo.py`, not the generated files.
