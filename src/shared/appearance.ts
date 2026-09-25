/**
 * Settings → Appearance: the values, their ranges and defaults.
 *
 * Ported from MonoCode's `src/features/settings/model/appearance.ts` and
 * `uiScale.ts` (MIT, Copyright (c) 2026 Nick; see THIRD_PARTY_NOTICES.md).
 * MonoCode keeps each value in localStorage; Clipward stores them in the app
 * shell's settings.json (src-tauri/src/appearance.rs, which validates the same
 * ranges; keep the two in step). How they reach the page is in
 * src/renderer/lib/appearance.ts.
 */

export type ColorScheme = 'dark' | 'light'
export type ThemePreference = ColorScheme | 'system'

export interface Appearance {
  theme: ThemePreference
  /** `#rrggbb`, or null for Clipward's own accent. */
  accentColor: string | null
  /** Hue of the neutral chrome, in degrees. */
  hue: number
  /** How strongly the hue tints the chrome, in percent. Zero is neutral grey. */
  saturation: number
  /** Lightness of the dark theme's base, in percent. Zero is true black. */
  darkLightness: number
  /** Opacity of the sidebar (and, with mainPaneGlass, the main pane) over window glass. */
  sidebarOpacity: number
  /** macOS WindowServer blur behind the window, in points. */
  blurRadius: number
  /** Extend the translucent treatment from the sidebar to the main pane. */
  mainPaneGlass: boolean
  /** Interface scale (webview zoom), in steps of 0.1. */
  uiScale: number
  /** Show the sidebar as the 52px icon rail. */
  collapsedRail: boolean
}

export const THEME_PREFERENCE_DEFAULT: ThemePreference = 'dark'
export const ACCENT_COLOR_DEFAULT = null

export const THEME_HUE_MIN = 0
export const THEME_HUE_MAX = 360
export const THEME_HUE_DEFAULT = 240

export const THEME_SATURATION_MIN = 0
export const THEME_SATURATION_MAX = 100
export const THEME_SATURATION_DEFAULT = 0

export const THEME_DARK_LIGHTNESS_MIN = 0
export const THEME_DARK_LIGHTNESS_MAX = 30
export const THEME_DARK_LIGHTNESS_DEFAULT = 9

export const SIDEBAR_OPACITY_MIN = 0.15
export const SIDEBAR_OPACITY_MAX = 1
export const SIDEBAR_OPACITY_DEFAULT = 0.85

export const SIDEBAR_BLUR_MIN = 1
export const SIDEBAR_BLUR_MAX = 64
export const SIDEBAR_BLUR_DEFAULT = 24

export const BODY_GLASS_DEFAULT = true

export const UI_SCALE_DEFAULT = 1
export const UI_SCALE_MIN = 0.5
export const UI_SCALE_MAX = 2
export const UI_SCALE_STEP = 0.1

export const COLLAPSED_RAIL_DEFAULT = false

/** Accent presets, as in MonoCode: blue, violet, pink, red, orange, green. */
export const ACCENT_COLOR_PRESETS = ['#4da3f5', '#8b5cf6', '#ec4899', '#ef4444', '#f59e0b', '#10b981'] as const
export const ACCENT_COLOR_LABELS = ['Blue', 'Violet', 'Pink', 'Red', 'Orange', 'Green'] as const

export const APPEARANCE_DEFAULTS: Readonly<Appearance> = Object.freeze({
  theme: THEME_PREFERENCE_DEFAULT,
  accentColor: ACCENT_COLOR_DEFAULT,
  hue: THEME_HUE_DEFAULT,
  saturation: THEME_SATURATION_DEFAULT,
  darkLightness: THEME_DARK_LIGHTNESS_DEFAULT,
  sidebarOpacity: SIDEBAR_OPACITY_DEFAULT,
  blurRadius: SIDEBAR_BLUR_DEFAULT,
  mainPaneGlass: BODY_GLASS_DEFAULT,
  uiScale: UI_SCALE_DEFAULT,
  collapsedRail: COLLAPSED_RAIL_DEFAULT
})

const HEX_COLOR_RE = /^#[0-9a-fA-F]{6}$/

export function isHexColor(value: string): boolean {
  return HEX_COLOR_RE.test(value)
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value))
}

function finite(value: unknown, fallback: number): number {
  const parsed = typeof value === 'number' ? value : Number(value)
  return Number.isFinite(parsed) ? parsed : fallback
}

export function isThemePreference(value: unknown): value is ThemePreference {
  return value === 'dark' || value === 'light' || value === 'system'
}

export function normalizeAccentColor(value: unknown): string | null {
  return typeof value === 'string' && isHexColor(value) ? value.toLowerCase() : ACCENT_COLOR_DEFAULT
}

/** Clamp to 0.5–2 and snap to 0.1 steps without float drift. */
export function normalizeUiScale(value: unknown): number {
  const parsed = typeof value === 'number' ? value : Number(value)
  if (!Number.isFinite(parsed)) return UI_SCALE_DEFAULT
  return Math.round(clamp(parsed, UI_SCALE_MIN, UI_SCALE_MAX) * 10) / 10
}

export function zoomInUiScale(current: number): number {
  return normalizeUiScale(current + UI_SCALE_STEP)
}

export function zoomOutUiScale(current: number): number {
  return normalizeUiScale(current - UI_SCALE_STEP)
}

/**
 * Every field present, in range and on its step; anything unreadable takes
 * its default. Accepts partial or foreign input (a shell that predates
 * appearance settings, a stale cache).
 */
export function normalizeAppearance(value: unknown): Appearance {
  const input = (value && typeof value === 'object' ? value : {}) as Partial<Record<keyof Appearance, unknown>>
  const d = APPEARANCE_DEFAULTS
  return {
    theme: isThemePreference(input.theme) ? input.theme : d.theme,
    accentColor: normalizeAccentColor(input.accentColor),
    hue: Math.round(clamp(finite(input.hue, d.hue), THEME_HUE_MIN, THEME_HUE_MAX)),
    saturation: Math.round(clamp(finite(input.saturation, d.saturation), THEME_SATURATION_MIN, THEME_SATURATION_MAX)),
    darkLightness: Math.round(
      clamp(finite(input.darkLightness, d.darkLightness), THEME_DARK_LIGHTNESS_MIN, THEME_DARK_LIGHTNESS_MAX)
    ),
    sidebarOpacity:
      Math.round(clamp(finite(input.sidebarOpacity, d.sidebarOpacity), SIDEBAR_OPACITY_MIN, SIDEBAR_OPACITY_MAX) * 100) / 100,
    blurRadius: Math.round(clamp(finite(input.blurRadius, d.blurRadius), SIDEBAR_BLUR_MIN, SIDEBAR_BLUR_MAX)),
    mainPaneGlass: typeof input.mainPaneGlass === 'boolean' ? input.mainPaneGlass : d.mainPaneGlass,
    uiScale: input.uiScale == null ? d.uiScale : normalizeUiScale(input.uiScale),
    collapsedRail: typeof input.collapsedRail === 'boolean' ? input.collapsedRail : d.collapsedRail
  }
}

export function sameAppearance(a: Appearance, b: Appearance): boolean {
  return (Object.keys(APPEARANCE_DEFAULTS) as (keyof Appearance)[]).every((key) => a[key] === b[key])
}

/** Black or white text on a solid `#rrggbb` fill, by WCAG relative luminance. */
export function accentForeground(color: string): '#000000' | '#ffffff' {
  const channels = [1, 3, 5].map((offset) => {
    const value = Number.parseInt(color.slice(offset, offset + 2), 16) / 255
    return value <= 0.04045 ? value / 12.92 : Math.pow((value + 0.055) / 1.055, 2.4)
  })
  const luminance = 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2]
  return luminance > 0.179 ? '#000000' : '#ffffff'
}

/** Resolve a preference against the OS; without matchMedia, dark. */
export function resolveColorScheme(value: ThemePreference, prefersLight: boolean | null): ColorScheme {
  if (value !== 'system') return value
  return prefersLight ? 'light' : 'dark'
}
