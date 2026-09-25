# Clipward Design System: Workbench

Clipward is a desktop tool people keep open for hours, so it looks like one: a neutral grey base, dense controls, hairline borders and one blue accent. Surfaces are flat. Colour comes from the footage, the one primary action, a chosen option and status. The visual language (palette, tint strengths, radii, control sizes, motion timings, overlay title bar) is adapted from the [MonoCode](https://github.com/hardbeat920/monocode) desktop app (MIT, see `THIRD_PARTY_NOTICES.md`). Clipward keeps its own name, logo and brand artwork.

Tokens live in `src/renderer/globals.css` (neutrals as HSL components, accent and status as RGB channels, redefined per colour scheme, plus the surface classes) and are mapped to Tailwind names in `tailwind.config.ts`. Reusable pieces live in `src/renderer/components/ui/`. Use them before writing new class strings.

---

## Principles

1. **One base, tinted by content.** Every fill, border and hover is the content colour at a low strength over the base (`bg-content/5`, `border-content/10`, `bg-selection`). Don't write raw greys, `white/…` or `black/…` for UI chrome, because they break in light mode. Black is allowed only on video (media wells, `media-chip`).
2. **Hairlines, not shadows.** Cards and rows have a 1px border and a faint fill. Only floating things (menus, tooltips, dialogs) get a shadow and a blur.
3. **Dense by default.** 13px body text, 28px controls, 32px rows. Nothing bigger than it needs to be.
4. **Selection is a brighter fill.** Active nav rows, tabs and segments use `bg-selection`. Accent is for the primary button, a chosen option's edge, live progress, focus rings and badges that count.
5. **Only status earns a hue.** Red means something broke, amber means setup is incomplete or a result needs a look, green means ready, found or saved.
6. **Say what happened.** Errors show the engine's message, in red, followed by the suggested fix (`job.errorHint`) and the failure code, never just "Something went wrong".

---

## Colour

Dark is the default. Settings → Appearance → Theme picks System, Dark or Light; the app resolves it to `html.theme-light` or `html.theme-dark` (without either class, the OS decides). The neutral rows below are the defaults: the Appearance hue and saturation tint every one of them, and dark-mode lightness moves the dark base (see **Appearance settings**).

| Token (Tailwind) | Dark | Light | Use |
| --- | --- | --- | --- |
| `canvas` (`--bg`) | `#171717` | `#F7F7F7` | Window base, page background |
| `sidebar` | `#141414` | `#F7F7F7` | Sidebar column (base mixed 10% toward black) |
| `elevated` | `#1F1F1F` | `#FFFFFF` | Solid popover fallback (light mode, reduced transparency) |
| `content` / `ink` | `#EBEBEB` | `#2E2E2E` | Text, and the source of every tint |
| `ink-muted` · `ink-subtle` · `ink-faint` | 163 · 125 · 94 grey | 102 · 128 · 166 grey | Solid greys kept for older non-text uses. Text uses `text-ink-secondary` / `text-ink-tertiary` (see Typography). |
| `stroke` | content 7% | content 7% | Structural hairlines: `border-stroke`, `divide-stroke`, `bg-stroke` |
| `selection` · `-subtle` · `-strong` · `-hover` | content 10 · 8 · 12 · 15% | 6 · 5 · 7 · 10% | Selected and raised fills |
| `accent` | `hsl(211 92% 62%)` | `#2986EE` | Primary button, chosen edges, progress, focus ring |
| `accent-fg` | `#7DB8FA` | `#0A66CC` | Accent used as text on the base |
| `success` · `warning` · `danger` | green-400 · amber-400 · red-400 | darker variants | Status only |
| `brand-gold` | `#FFD500` | same | The virality score icon |

A custom accent (Appearance → Accent color) replaces `accent` in both schemes; `accent-fg` becomes it lightened 30% toward white in dark mode and darkened 25% in light mode, and `accent-ink` black or white by luminance.

Text ladder on the base: title `text-ink`, body `text-content/80`, secondary and descriptions `text-ink-secondary`, group labels and placeholders `text-ink-tertiary`, disabled `text-content/25`. The emphasis strengths are per scheme; see Typography.

---

## Typography

One small, named scale. Tokens live in the **Typography** block of `globals.css` (rem sizes and line heights, font stacks, text emphasis, smoothing) and are mapped in `tailwind.config.ts` (`TYPE_SCALE`, `TYPE_FAMILIES`, `TYPE_COLORS`). Each `text-*` token sets size, line height, weight and tracking together; add a `font-*` or `leading-*` utility only to depart from it on purpose. Sizes are rem, so Settings → Appearance → Interface scale (webview zoom, or a root font size) scales all text.

| Token | Size / line | Weight | Tracking | Use |
| --- | --- | --- | --- | --- |
| `text-display` | 28 / 32 | 600 | −0.02em | One big live number per screen (job progress) |
| `text-title` | 20 / 26 | 600 | −0.012em | Page titles (`PageHeader`, failure and progress pages), stat values, the update dialog |
| `text-heading` | 15 / 20 | 600 | −0.006em | Dialog titles, hero cards (setup card, "Clipping started"), About |
| `text-section` | 13 / 18 | 600 | 0 | Card and step titles (`PanelHeader`, wizard step, empty states) |
| `text-label` | 13 / 18 | 500 | 0 | Row, option, field and nav labels; clip and run titles |
| `text-body` | 13 / 20 | 400 | 0 | Running text, values, breadcrumbs, dialog bodies, text areas (the `body` default) |
| `text-caption` | 12 / 18 | 400 | 0 | Descriptions, hints, errors under fields, control text (buttons, inputs, menus, tabs) |
| `text-meta` | 11 / 16 | 400 | 0.005em | Metadata under a title, badges, chips over video, shortcuts, counts |
| `text-micro` | 10 / 14 | 500 | 0.01em | Step numbers, counters inside a ring or pill. Never prose. |

`.eyebrow` (group labels, stat labels) is `text-caption font-semibold text-ink-tertiary`, sentence case. There is no all-caps text in the UI (caption presets preview their own styling).

The old `text-3xs` … `text-3xl` sizes remain as rem aliases so older code keeps working; new code uses the names above, never `text-[13px]`-style values. `cn()` (`lib/utils.ts`) teaches tailwind-merge the token names (`TYPE_TOKENS`); without that it would treat `text-label` as a colour and drop `text-ink` next to it. Add new tokens to both lists.

### Hierarchy

Page title (`title`) › card title (`section`) › row label (`label`) › description (`caption`, secondary) › metadata (`meta`, secondary or tertiary). Adjacent levels differ in at least two of size, weight and emphasis. A card's title and its rows share 13px, so the title is semibold and the rows medium.

### Emphasis and contrast

| Class | Strength (dark · glass · light) | Use |
| --- | --- | --- |
| `text-ink` | 100% | Titles, labels, values |
| `text-content/80` | 80% | Body prose, emphasised numbers inside secondary text |
| `text-ink-secondary` | 56% · 64% · 70% | Anything the user must read: descriptions, details, metadata, inactive tabs and nav. At least 4.5:1 (WCAG AA) on the base and on cards: 5.4:1 on `#171717`, 5.0:1 on `#F7F7F7`. |
| `text-ink-tertiary` | 40% · 48% · 54% | Group labels, shortcuts, counts, placeholders, separators, "or". At least 3:1. |
| `text-content/25` | 25% | Disabled only |

Light text on a dark base needs less strength than dark text on a light base for the same contrast, so the two emphasis levels are per-scheme variables (`--ink-secondary-alpha`, `--ink-tertiary-alpha`) rather than fixed `/45`-style alphas; glass lifts them because the base under the text is translucent, and in dark mode they rise with the Appearance lightness slider (at the 30% maximum, secondary is 79%, still 5.2:1). Don't write `text-content/45` or `/50` for text: it is 3.9:1 in dark mode and 2.6:1 in light mode. The solid `ink-muted`/`ink-subtle`/`ink-faint` greys are kept for existing non-text uses.

### Families

- `font-sans`: SF Pro on macOS (`-apple-system`), Segoe UI on Windows, and the bundled **Geist** everywhere else. Geist comes before `system-ui` on purpose: on Linux `system-ui` resolves to the fontconfig default, usually DejaVu Sans, which is about 15% wider and wrapped dates and cut labels the layout was sized for.
- `font-mono` (`ui-monospace`, SF Mono, Menlo, Cascadia Mono, Consolas, then the bundled **Geist Mono**) is for machine text only: file paths, versions, engine error output, failure codes, the hex colour field, and typed timecodes in inputs (`TextInput mono`).
- Numbers are **not** mono. Timecodes, durations, costs, percentages, counts, speeds and dates use the UI face with `tabular-nums`, so they line up in columns and don't jitter while they count, without switching typeface mid-sentence. (`.tabular` remains as an alias.)

### Rules

- **Measure.** Descriptions stop at about 70 characters (`max-w-[70ch]` in `PageHeader` and `PanelHeader`, `65ch` in `SettingRow`, `48ch` in `EmptyState`). Don't let 12px prose run the full card width.
- **Line height** comes from the token. Don't add `leading-relaxed` to captions (18px on 12px is already 1.5); use `leading-tight`/`leading-none` only for single-line text inside a fixed-size mark.
- **Truncation.** Truncate only where the full text is one hover away (`title=`) or elsewhere on screen. Titles that identify something may wrap: page titles clamp to two lines with `text-balance`, clip and run card titles clamp to two lines (run cards reserve both lines so meta rows align). Status words ("Failed", "Cancelled") never truncate; the error after them gives way first and is in the row's tooltip.
- **Columns.** Right-aligned numeric columns are `tabular-nums whitespace-nowrap` at a fixed rem width wide enough for the longest value in Geist, SF Pro and Segoe UI ("Yesterday, 12:58 PM" needs `w-32`).
- **Weights.** 400, 500 and 600 only; never bold (700) in the UI.
- **Case.** Sentence case everywhere, including group labels and buttons.
- **Rendering.** Grayscale antialiasing in dark mode (light text on dark), the platform default in light mode; `kbd` uses the UI face.

---

## Surfaces (`globals.css`)

| Class | What it is | Use |
| --- | --- | --- |
| `window-bg` | Base (a tint in glass mode) | The window backdrop behind the title bar and pages |
| `app-bg` | Solid base (82% with blur in glass mode) | Sticky strips that content scrolls under, insets inside a card |
| `sidebar-bg` | Solid sidebar tone (a tint in glass mode) | The sidebar |
| `card` | content 3% fill, content 8% border | Panels, empty states, media cards. Use `rounded-xl`. |
| `tile` (+ `tile-hover`) | Same recipe, for rows and options inside a card | Option cards, setting rows. `rounded-lg`. |
| `tile-selected` | Accent 10% fill, accent 70% border | The chosen option |
| `field` | content 3% fill, content 10% border | Inputs, selects, text areas, inset lists. Focus turns the border accent. |
| `popover-surface` | Base at 86% with `blur(24px)`, content 10% border, soft shadow; opaque in light mode | Menus, hover cards, dialogs, the floating clip toolbar |
| `media-chip` | Black 50% with blur, white text | Labels and buttons over video or artwork |

`prefers-reduced-transparency` makes floating surfaces solid and turns window glass off; `prefers-reduced-motion` stops animation.

### Window glass

The second glass layer, after the CSS blur on floating surfaces. On macOS and Windows the app shell (`src-tauri/src/window_glass.rs`) creates the window transparent and blurs the desktop behind it: on macOS a WindowServer blur whose radius is the Appearance blur setting (over a visual-effect view faded to 1%, which falls back to the full sidebar material if the private blur call is missing); on Windows 11 Mica, or acrylic on Windows 10, with a fixed blur. Linux stays opaque. Settings → Appearance → **Window transparency** switches it at runtime (`settings.setWindowGlass`, the `window_set_glass` command); it is stored in `settings.json` and on by default.

The renderer sets `html.glass` (`src/renderer/lib/glass.ts`) when the platform supports it, the setting is on and the OS does not ask for reduced transparency. The page, `body` and `#root` go clear, and a few variables in `globals.css` turn chrome into tints of the base over the material:

| Variable | Glass | Opaque | Paints |
| --- | --- | --- | --- |
| `--glass-window` | sidebar opacity with main pane glass, else 1 | 1 | `window-bg`: backdrop and title bar |
| `--glass-sidebar` | sidebar opacity (0.85) | 1 | `sidebar-bg` |
| `--glass-strip` | 0.82 | 1 | `app-bg` sticky strips and insets (also blurred) |
| `--glass-surface` | 0.72 | 0 | A layer of base under `card` and `field`, so text, inputs and thumbnails stay readable |

Video, thumbnails and `media-chip` are unchanged. Light mode keeps the opaque values (pale desktops make translucent UI illegible), so glass is a dark-mode look. Don't add new translucent chrome with raw alphas: use these classes, so opaque mode and reduced transparency keep working.

---

### Appearance settings

Settings → Appearance ports MonoCode's appearance page with its ranges and defaults. The app shell stores the values in `settings.json` (`src-tauri/src/appearance.rs` validates them; `src/shared/appearance.ts` mirrors the ranges) and the renderer applies them live (`src/renderer/lib/appearance.ts`) as a class or variable on the root, all in the **Appearance** block of `globals.css`:

| Setting | Range (default) | Drives |
| --- | --- | --- |
| Theme | System · Dark · Light (Dark) | `html.theme-light` / `html.theme-dark` |
| Accent color | Default, 6 presets, custom (Default) | `html.has-user-accent`, `--user-accent*` → `--accent`, `--accent-fg`, `--accent-ink` |
| Hue | 0–360° (240) | `--theme-hue`, every neutral token |
| Saturation | 0–100% (0) | `--theme-saturation`, every neutral token |
| Dark-mode lightness | 0–30% (9) | `--theme-dark-lightness` → dark `--bg`, `--sidebar` (×0.9), `--elevated` (+3%) |
| Sidebar opacity | 15–100% (85) | `--sidebar-opacity` → `--glass-sidebar` (glass only) |
| Blur radius | 1–64 (24) | macOS WindowServer blur, set by the shell (glass only) |
| Main pane glass | on/off (on) | `html.glass-body`: `--glass-window` takes the sidebar opacity |
| Interface scale | 50–200% in 10% steps (100) | Webview zoom, set by the shell (also View → Zoom, ⌘= ⌘- ⌘0); CSS `zoom` outside it |
| Collapsed rail | on/off (off) | The 52px sidebar rail (⌘\ toggles it) |

Glass controls are disabled when window transparency is off or unsupported, in light mode and with reduced transparency; dark-mode lightness is disabled in light mode. **Reset appearance** restores every default and turns window transparency back on. `public/appearance-boot.js` paints the cached theme, tint and accent before the bundle loads, so launch doesn't flash the defaults. The shell sets the webview zoom before the page loads. Pages size with `h-full`/`w-full`, not viewport units, so the CSS-zoom fallback lays out correctly.

---

## Shape

| Radius | Px | Use |
| --- | --- | --- |
| `rounded` | 4 | Badges, chips over media, segments inside a track |
| `rounded-md` | 6 | Buttons, inputs, nav rows, tabs, segmented tracks |
| `rounded-lg` | 8 | Tiles, menu rows, media inside a card, callouts |
| `rounded-xl` | 12 | Cards, popovers, menus |
| `rounded-2xl` | 16 | Dialogs |
| `rounded-full` | — | Switches, status dots, progress bars, step numbers |

Nested shapes step down one size (card `xl` with `p-1` → media `lg`).

---

## Layout

```
┌ sidebar (sidebar-bg, border-r) ─┬ title bar (h-10, border-b, drag region) ───────────┐
│ ● ● ●             [Clipward]    │ [▯]  Studio › Library                               │
│ Studio                          ├─────────────────────────────────────────────────────┤
│  ✦ Create           ⌘1          │ #page-scroll                                        │
│  ▦ Library          ⌘2          │   <Page width>                                      │
│  ≡ Jobs          [3]            │     PageHeader                                      │
│ App                             │     cards …                                         │
│  ⚙ Settings         ⌘,          │                                                     │
│ [running job tile]              │                                                     │
├─────────────────────────────────┤                                                     │
│ ● Ready to clip  v0.2.0         │                                                     │
└─────────────────────────────────┴─────────────────────────────────────────────────────┘
```

- **Window**: Tauri overlay title bar with a hidden title (`titleBarStyle: "Overlay"`, `hiddenTitle: true`). The sidebar's top 40px strip and the page title bar are `data-tauri-drag-region="deep"`; buttons inside carry `data-tauri-drag-region="false"`. On macOS the sidebar strip pads 78px for the traffic lights (`TRAFFIC_LIGHT_INSET`); when the sidebar is a rail, the title bar adds the difference.
- **Sidebar**: 220px, or a 52px icon rail when collapsed or when the window is narrower than 1024px. Nav rows are 32px, `rounded-md`, icon at 70% opacity, `text-label`; active rows get `bg-selection`, others `text-ink-secondary` and `hover:bg-content/10`. The shortcut sits at the right in `text-meta text-ink-tertiary`; Jobs shows a solid accent count instead while jobs run. Group labels are `text-caption font-semibold text-ink-tertiary`; in the rail a hairline replaces them. The footer, above a hairline, shows setup status and the version.
- **Title bar**: the sidebar toggle (⌘\, saved as Settings → Appearance → Collapsed rail) and a breadcrumb, group › page. Pages scroll below it, so sticky elements use small offsets (`top-2`, `top-6`).
- **Page**: wrap every page in `<Page width>`: `focus` 720, `narrow` 880, `default` 1280, `wide` 1680. Gutters `px-5`/`sm:px-8`, `pt-6`. Stacks use `gap-2.5`/`space-y-3`; cards pad `p-4`.
- **Create** is a wizard: Video → Brief → Format → Clips → Captions → Review. Its step strip uses the tab style (active `bg-selection`), the step card has a header row above a hairline, and Back/Next/Generate stick to the bottom on an `app-bg` strip. ⌘↵ generates from any step.

---

## Components (`components/ui/`)

| Component | Notes |
| --- | --- |
| `Page`, `PageHeader` | Page column; title (`text-title`, up to two lines), description (`text-caption text-ink-secondary`, 70ch), actions, optional `leading` back link. |
| `Panel` / `PanelHeader` | `card rounded-xl p-4`; header takes an `IconTile` (`sm`), title and description. |
| `Button` | `primary` (solid accent) · `secondary` (hairline) · `ghost` · `danger` (red hairline). Sizes `sm` 24, `md` 28, `lg` 32. `iconOnly` squares it (needs `aria-label`). `loading`. |
| `TextInput` · `TextArea` | `field` surfaces matching Button heights (`sm` 24, `md` 28, `lg` 32), `leading`/`trailing` slots, `mono`. |
| `Select` | Never a native `<select>`: a `field` combobox that opens a `popover-surface` listbox with `rounded-lg` rows, `bg-selection` highlight and a check on the chosen option. Arrows, Home/End, Page Up/Down, type-ahead, Enter/Space, Escape (closes only the menu), optional search. Top-layer popover. `MENU_SURFACE` and `menuOptionClass` style any other menu. |
| `Field` | Label, optional `aside`, hint. |
| `Segmented` | Hairline track, `bg-selection` on the chosen segment, roving tab stop. `onRadioKeyDown` serves custom radio groups. |
| `SettingRow` | Title and description with a control on a `tile`; `bare` drops the tile. |
| `Switch` | 36×20, `bg-content/20` off, solid accent on. |
| `Slider` | 224px range: 4px `content/15` track, 12px content thumb, mono value to the right. Streams changes while dragging. |
| `ColorSwatchRow` / `ColorPicker` | 16px round swatches with a ring on the chosen one and a rainbow custom button; the picker is a saturation/brightness square, hue strip and hex field (arrow keys work), shown in a `popover-surface`. |
| `Checkbox` | 16px, hairline off, solid accent on; `indeterminate`; `variant="overlay"` for video. |
| `Badge`, `StatusDot` | 18px tinted labels with a hairline border; tones `neutral` · `accent` · `success` · `warning` · `danger`. Dots are solid and can pulse. |
| `IconTile` | Tinted square with a hairline border, sizes `sm`–`xl`. |
| `Callout` | Tinted strip with a hairline border for errors, warnings, info and success, with optional title, action and dismiss. `role` is alert for danger. |
| `ProgressBar` / `ProgressRing` | 4px bar or thin ring on `content/10`, accent fill, red when failed. |
| `EmptyState` | Card with an icon tile, title, one sentence and one action. |
| `Skeleton` | `bg-content/5` block that pulses. |
| `HoverCard` | Read-only tooltip card on hover or focus; `popover-surface`, top layer, Escape closes it. |
| `Dialog` / `DialogFooter` | `bg-black/40` backdrop and a `popover-surface` `rounded-2xl` panel set at 10% from the top; footer actions sit under a hairline. `layer="system"` for the update prompt. Callers own focus trapping and Escape. |

Layering is decided in one place, `lib/layers.ts`: popovers 80, dialogs 90, popovers inside dialogs 91, system prompts 100.

Brand: `components/brand/ClipwardLogo` renders the lockup (switching to the dark-ink artwork in light mode), the app icon or the mark. Every logo SVG is generated by `scripts/icon/build-logo.py`; edit the generator, not the exports.

---

## Patterns

- **Option cards** (format, framing, speed, provider): `tile tile-hover rounded-lg`, plus `tile-selected` when chosen.
- **Filter chips and tabs** (job filters, wizard steps): `rounded-md h-6/h-7`, `bg-selection text-content` when chosen, `text-content/50 hover:bg-content/5` otherwise.
- **Lists** (jobs, system check, review): one card, rows separated by `divide-stroke`, hover `bg-content/[0.04]`. Row title `text-body`/`text-label`, detail `text-caption text-ink-secondary`, numbers `tabular-nums` in fixed-width right-aligned columns.
- **Media cards** (clips, runs): `card rounded-xl p-1` with the media inset `rounded-lg`. Hover brightens the border and fill; selection gives an accent border. Overlays over footage are `media-chip`s. The media is the play button.
- **Floating toolbar** (clip selection): `popover-surface rounded-lg`, sticky at `top-2`.
- **Autosave**: Settings has no Save button; a "Saved" badge confirms it in the header.
- **Thumbnails** go through `lib/thumbnails.ts`, one at a time.

---

## Motion

- Feedback (hover, press, selection): 120ms (`duration-feedback`), `ease-out` = `cubic-bezier(0.22, 1, 0.36, 1)`.
- Entrances use `cubic-bezier(0.16, 1, 0.3, 1)`: pages `animate-fade-in` (200ms, 4px rise), popovers `.popover-open` (170ms, scale from 0.96), dialogs `.modal-panel` (200ms, 8px rise, scale from 0.98) over `.modal-backdrop` (160ms fade).
- Animate content, not the blurred surface under it, and end at `transform: none` (`backwards` fill) so no transform traps fixed descendants.
- Motion communicates state: the pulsing active step, the skeleton pulse, progress. Nothing loops to decorate.

---

## Accessibility

- `:focus-visible` shows a 2px accent outline on every control. Icon-only buttons carry `aria-label`.
- Radio-like groups use `role="radiogroup"`/`role="radio"` with `aria-checked` and a roving tab stop; toggles use `aria-pressed`; switches use `role="switch"`.
- Anything the user must read is at least `text-ink-secondary` (4.5:1 in every scheme); `text-ink-tertiary` is for placeholders, separators, counts and group labels.
- Keyboard: ⌘1–⌘3 and ⌘, switch pages, ⌘\ toggles the sidebar, ⌘↵ generates clips (Ctrl on Windows and Linux).

---

## Adding UI

1. Reach for `components/ui/` first. If you write the same class string twice, it belongs there.
2. Pick a surface (`card`, `tile`, `field`, `popover-surface`, `media-chip`) and tint with `content/…`. Check it in light mode.
3. Keep controls at 24–32px and corners at `md`/`lg`/`xl`.
4. Colour needs a reason: status, the one primary action, or a chosen option.
5. Give every async action a loading state, and every failure the real message and a next step.
