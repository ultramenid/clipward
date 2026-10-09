# Third-party notices

Clipward's [MIT license](LICENSE) covers Clipward-owned source. It does not change the licenses of bundled components.

| Component | Source and terms | Shipped notice |
| --- | --- | --- |
| Clipward clipping engine assets | Clipward-owned engine source is MIT; bundled fonts and YuNet retain their own licenses | `crates/engine/THIRD_PARTY_NOTICES.md` and license files under `crates/engine/assets/` |
| FFmpeg and ffprobe 8.1.3 | [FFmpeg source](https://ffmpeg.org/download.html), LGPL 2.1 or later build with GPL, nonfree and version 3 components disabled | `engine-bin/FFMPEG-LICENSE`, `engine-bin/FFMPEG-SOURCE.txt`, `engine-bin/FFMPEG-SOURCE.tar.xz`; build options are in `scripts/build-ffmpeg-mac.sh` and the binary's `-version` output |
| libass and its caption-shaping libraries | [libass source](https://github.com/libass/libass) and the upstream projects identified in the bundle; each retains its own license | `engine-bin/BUNDLED_LIBRARIES.txt` records exact installed versions and `engine-bin/THIRD_PARTY_LICENSES/` contains the notices. When Homebrew omits GLib's license, staging copies the LGPL 2.1 text from `scripts/licenses/`. |
| yt-dlp | [yt-dlp](https://github.com/yt-dlp/yt-dlp) standalone binary, Unlicense for its own code, with separate terms for bundled dependencies | License files staged beside `engine-bin/yt-dlp` |
| Geist fonts | [Geist](https://github.com/vercel/geist-font), SIL Open Font License 1.1 | `Geist-OFL.txt` in the packaged app resources |
| MonoCode UI styles | [MonoCode](https://github.com/hardbeat920/monocode), MIT license, Copyright (c) 2026 Nick. Clipward's design tokens and surface recipes (`crates/app/src/theme.rs`), control recipes (`crates/app/src/ui.rs`: sidebar rows, title bar, settings rows, toggles, segmented controls, menus, dialogs) and z-index scale are adapted from MonoCode's `src/styles/index.css`, `src/shared/ui/`, `src/shared/lib/layers.ts` and `src/app/shell/`. The native window glass (`crates/app/src/glass.rs`: transparent window, WindowServer blur radius with its 1% backing view) is adapted from MonoCode's `src-tauri/src/macos.rs` and `src-tauri/src/window.rs`. Settings → Appearance (theme, accent presets and colour picker, hue/saturation tint, dark-mode lightness, sidebar opacity, blur radius, main pane glass, interface scale, collapsed rail, reset) and its ranges, defaults and tests are ported from MonoCode's `src/features/settings/model/appearance.ts`, `uiScale.ts` and their tests, `src/features/settings/ui/SettingsView.tsx`, `src/shared/ui/ColorPickerPopover.tsx`, `src/shared/lib/colorUtils.ts` and `src/shared/hooks/useColorScheme.ts`, into `crates/core/src/appearance.rs`, `crates/core/src/color.rs` and `crates/app/src/pages/settings/`. No MonoCode name, logo or provider marks are used. | The MIT notice at the end of this file |
| GPUI (`gpui-kit`), whisper.cpp (`whisper-rs`), tract and other Rust crates | Versions in `Cargo.lock`; each crate retains its own license | The generated Rust dependency inventory |

The release packages the Rust clipping engine and assets committed in this repository, plus the FFmpeg and yt-dlp builds staged in `engine-bin/`. The final package still needs an asset and dependency license review, a signed/notarized install check, and a review of the generated inventories before publication.

## MonoCode

Portions of the app's styles and UI component recipes are adapted from MonoCode (https://github.com/hardbeat920/monocode) under the following license:

```
MIT License

Copyright (c) 2026 Nick

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
