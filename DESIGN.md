# Clipward Design System: Workbench

Clipward is a desktop tool people keep open for hours, so it looks like one: a neutral grey base, dense controls, hairline borders and one blue accent. Surfaces are flat. Colour comes from the footage, the one primary action, a chosen option and status. The visual language (palette, tint strengths, radii, control sizes, motion timings, overlay title bar) is adapted from the [MonoCode](https://github.com/hardbeat920/monocode) desktop app (MIT, see `THIRD_PARTY_NOTICES.md`). Clipward keeps its own name, logo and brand artwork.

Tokens live in `src/renderer/globals.css` (RGB channels, redefined per colour scheme, plus the surface classes) and are mapped to Tailwind names in `tailwind.config.ts`. Reusable pieces live in `src/renderer/components/ui/`. Use them before writing new class strings.

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

Dark is the default. Light follows the OS (`prefers-color-scheme`); `html.theme-light` or `html.theme-dark` forces either.

| Token (Tailwind) | Dark | Light | Use |
| --- | --- | --- | --- |
| `canvas` (`--bg`) | `#171717` | `#F7F7F7` | Window base, page background |
| `sidebar` | `#141414` | `#F7F7F7` | Sidebar column (base mixed 10% toward black) |
| `elevated` | `#1F1F1F` | `#FFFFFF` | Solid popover fallback (light mode, reduced transparency) |
| `content` / `ink` | `#EBEBEB` | `#2E2E2E` | Text, and the source of every tint |
| `ink-muted` · `ink-subtle` · `ink-faint` | 163 · 125 · 94 grey | 102 · 128 · 166 grey | Secondary text, hints, placeholders. For new code prefer `text-content/60`, `/45`, `/35`. |
| `stroke` | content 7% | content 7% | Structural hairlines: `border-stroke`, `divide-stroke`, `bg-stroke` |
| `selection` · `-subtle` · `-strong` · `-hover` | content 10 · 8 · 12 · 15% | 6 · 5 · 7 · 10% | Selected and raised fills |
| `accent` | `hsl(211 92% 62%)` | `#2986EE` | Primary button, chosen edges, progress, focus ring |
| `accent-fg` | `#7DB8FA` | `#0A66CC` | Accent used as text on the base |
| `success` · `warning` · `danger` | green-400 · amber-400 · red-400 | darker variants | Status only |
| `brand-gold` | `#FFD500` | same | The virality score icon |

Text ladder on the base: title `text-content`, body `text-content/80`, secondary `/60`, descriptions `/45`, group labels `/35`, placeholders `/35`, disabled `/25`.

---

## Typography

- The system UI font (`system-ui`, SF Pro on macOS, Segoe UI on Windows) for text, `ui-monospace` for numbers, timecodes, paths and costs. Vendored Geist and Geist Mono (SIL OFL 1.1, `assets/fonts/`) are fallbacks where the system has none.
- Scale: `3xs` 10, `2xs` 11, `xs` 12, `sm` 13, `base` 14, `lg` 16, `xl` 20. UI text is 13px (`text-sm` or `text-[13px]`), descriptions and controls 12px, shortcuts and meta 11px.
- Page title: `text-xl font-medium leading-tight`. Card title: `text-[13px] font-semibold`. Group label (`.eyebrow`): `text-xs font-semibold text-content/40`, sentence case.
- Numbers that change or line up use `font-mono tabular`.

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

The second glass layer, after the CSS blur on floating surfaces. On macOS and Windows the app shell (`src-tauri/src/window_glass.rs`) creates the window transparent and draws a native material behind the webview: macOS vibrancy with the sidebar material, Windows 11 Mica, or acrylic on Windows 10. Linux stays opaque. Settings → Appearance → **Window transparency** switches it at runtime (`settings.setWindowGlass`, the `window_set_glass` command); it is stored in `settings.json` and on by default.

The renderer sets `html.glass` (`src/renderer/lib/glass.ts`) when the platform supports it, the setting is on and the OS does not ask for reduced transparency. The page, `body` and `#root` go clear, and a few variables in `globals.css` turn chrome into tints of the base over the material:

| Variable | Glass | Opaque | Paints |
| --- | --- | --- | --- |
| `--glass-window` | 0.72 | 1 | `window-bg`: backdrop and title bar |
| `--glass-sidebar` | 0.5 | 1 | `sidebar-bg` |
| `--glass-strip` | 0.82 | 1 | `app-bg` sticky strips and insets (also blurred) |
| `--glass-surface` | 0.72 | 0 | A layer of base under `card` and `field`, so text, inputs and thumbnails stay readable |

Video, thumbnails and `media-chip` are unchanged. Light mode keeps the opaque values (pale desktops make translucent UI illegible), so glass is a dark-mode look. Don't add new translucent chrome with raw alphas: use these classes, so opaque mode and reduced transparency keep working.

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
- **Sidebar**: 220px, or a 52px icon rail when collapsed or when the window is narrower than 1024px. Nav rows are 32px, `rounded-md`, icon at 70% opacity, `text-sm font-medium`; active rows get `bg-selection`, others `text-content/50` and `hover:bg-content/10`. The shortcut sits at the right in `text-[11px] text-content/40`; Jobs shows a solid accent count instead while jobs run. Group labels are `text-xs font-semibold text-content/35`; in the rail a hairline replaces them. The footer, above a hairline, shows setup status and the version.
- **Title bar**: the sidebar toggle (⌘\, remembered in `store/use-sidebar-store.ts`) and a breadcrumb, group › page. Pages scroll below it, so sticky elements use small offsets (`top-2`, `top-6`).
- **Page**: wrap every page in `<Page width>`: `focus` 720, `narrow` 880, `default` 1280, `wide` 1680. Gutters `px-5`/`sm:px-8`, `pt-6`. Stacks use `gap-2.5`/`space-y-3`; cards pad `p-4`.
- **Create** is a wizard: Video → Brief → Format → Clips → Captions → Review. Its step strip uses the tab style (active `bg-selection`), the step card has a header row above a hairline, and Back/Next/Generate stick to the bottom on an `app-bg` strip. ⌘↵ generates from any step.

---

## Components (`components/ui/`)

| Component | Notes |
| --- | --- |
| `Page`, `PageHeader` | Page column; title, description (`text-xs text-content/50`), actions, optional `leading` back link. |
| `Panel` / `PanelHeader` | `card rounded-xl p-4`; header takes an `IconTile` (`sm`), title and description. |
| `Button` | `primary` (solid accent) · `secondary` (hairline) · `ghost` · `danger` (red hairline). Sizes `sm` 24, `md` 28, `lg` 32. `iconOnly` squares it (needs `aria-label`). `loading`. |
| `TextInput` · `TextArea` | `field` surfaces matching Button heights (`sm` 24, `md` 28, `lg` 32), `leading`/`trailing` slots, `mono`. |
| `Select` | Never a native `<select>`: a `field` combobox that opens a `popover-surface` listbox with `rounded-lg` rows, `bg-selection` highlight and a check on the chosen option. Arrows, Home/End, Page Up/Down, type-ahead, Enter/Space, Escape (closes only the menu), optional search. Top-layer popover. `MENU_SURFACE` and `menuOptionClass` style any other menu. |
| `Field` | Label, optional `aside`, hint. |
| `Segmented` | Hairline track, `bg-selection` on the chosen segment, roving tab stop. `onRadioKeyDown` serves custom radio groups. |
| `SettingRow` | Title and description with a control on a `tile`; `bare` drops the tile. |
| `Switch` | 36×20, `bg-content/20` off, solid accent on. |
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
- **Lists** (jobs, system check, review): one card, rows separated by `divide-stroke`, hover `bg-content/[0.04]`. Row title 13px, detail 12px `text-content/50`, numbers mono.
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
- Anything the user must read is at least `text-content/45`; `/35` and below is for placeholders, separators and group labels.
- Keyboard: ⌘1–⌘3 and ⌘, switch pages, ⌘\ toggles the sidebar, ⌘↵ generates clips (Ctrl on Windows and Linux).

---

## Adding UI

1. Reach for `components/ui/` first. If you write the same class string twice, it belongs there.
2. Pick a surface (`card`, `tile`, `field`, `popover-surface`, `media-chip`) and tint with `content/…`. Check it in light mode.
3. Keep controls at 24–32px and corners at `md`/`lg`/`xl`.
4. Colour needs a reason: status, the one primary action, or a chosen option.
5. Give every async action a loading state, and every failure the real message and a next step.
