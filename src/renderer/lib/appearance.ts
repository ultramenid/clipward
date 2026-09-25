import { useEffect, useLayoutEffect, useState } from 'react'
import { useSettingsStore } from '../store/use-settings-store'
import {
  accentForeground,
  normalizeAccentColor,
  normalizeAppearance,
  normalizeUiScale,
  resolveColorScheme,
  type Appearance,
  type ColorScheme
} from '../../shared/appearance'
import { hexToChannels, mixChannels } from './color'

/**
 * Settings → Appearance on the page: the root's theme class and the CSS
 * variables in globals.css' Appearance block. Ported from MonoCode's
 * `appearance.ts` apply functions (MIT, Copyright (c) 2026 Nick; see
 * THIRD_PARTY_NOTICES.md).
 *
 * The app shell owns the values (settings.json). A copy is cached in
 * localStorage so public/appearance-boot.js can paint the right theme, tint
 * and accent before the bundle runs; keep that script in step with
 * `applyAppearance`. Interface scale is the webview zoom, set by the shell;
 * outside it (a plain browser) the page scales itself with CSS zoom.
 */

export const APPEARANCE_CACHE_KEY = 'clipward.appearance'

/** Fired on `window` when the resolved colour scheme flips (detail: ColorScheme). */
export const SCHEME_CHANGE_EVENT = 'clipward:schemechange'

const LIGHT_QUERY = '(prefers-color-scheme: light)'

function systemPrefersLight(): boolean | null {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return null
  return window.matchMedia(LIGHT_QUERY).matches
}

/** The scheme the page shows now. */
export function currentColorScheme(): ColorScheme {
  const root = document.documentElement
  if (root.classList.contains('theme-light')) return 'light'
  if (root.classList.contains('theme-dark')) return 'dark'
  return systemPrefersLight() ? 'light' : 'dark'
}

export interface ApplyOptions {
  /** The shell zooms the webview, so the page must not scale itself. */
  nativeUiScale: boolean
}

/** Write `appearance` onto the root element. Returns the resolved scheme. */
export function applyAppearance(value: Appearance, { nativeUiScale }: ApplyOptions): ColorScheme {
  const appearance = normalizeAppearance(value)
  const root = document.documentElement
  const style = root.style
  const previous = currentColorScheme()

  const scheme = resolveColorScheme(appearance.theme, systemPrefersLight())
  root.classList.toggle('theme-light', scheme === 'light')
  root.classList.toggle('theme-dark', scheme === 'dark')

  style.setProperty('--theme-hue', String(appearance.hue))
  style.setProperty('--theme-saturation', `${appearance.saturation}%`)
  style.setProperty('--theme-dark-lightness', `${appearance.darkLightness}%`)
  style.setProperty('--sidebar-opacity', String(appearance.sidebarOpacity))
  root.classList.toggle('glass-body', appearance.mainPaneGlass)

  applyAccentColor(appearance.accentColor)

  const scale = normalizeUiScale(appearance.uiScale)
  if (nativeUiScale || scale === 1) style.removeProperty('zoom')
  else style.setProperty('zoom', String(scale))

  if (scheme !== previous && typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent<ColorScheme>(SCHEME_CHANGE_EVENT, { detail: scheme }))
  }
  return scheme
}

export function applyAccentColor(value: string | null): string | null {
  const next = normalizeAccentColor(value)
  const root = document.documentElement
  root.classList.toggle('has-user-accent', next != null)
  const names = ['--user-accent', '--user-accent-ink', '--user-accent-fg-dark', '--user-accent-fg-light']
  if (next == null) {
    for (const name of names) root.style.removeProperty(name)
    return next
  }
  root.style.setProperty('--user-accent', hexToChannels(next))
  root.style.setProperty('--user-accent-ink', hexToChannels(accentForeground(next)))
  root.style.setProperty('--user-accent-fg-dark', mixChannels(next, 255, 0.3))
  root.style.setProperty('--user-accent-fg-light', mixChannels(next, 0, 0.25))
  return next
}

export function readCachedAppearance(): Appearance | null {
  try {
    const raw = localStorage.getItem(APPEARANCE_CACHE_KEY)
    return raw ? normalizeAppearance(JSON.parse(raw)) : null
  } catch {
    return null
  }
}

export function writeCachedAppearance(value: Appearance): void {
  try {
    localStorage.setItem(APPEARANCE_CACHE_KEY, JSON.stringify(value))
  } catch {
    // Private mode or quota: the next launch paints the default theme until settings load.
  }
}

function useSystemPrefersLight(): boolean | null {
  const [light, setLight] = useState(systemPrefersLight)
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return
    const query = window.matchMedia(LIGHT_QUERY)
    const update = (): void => setLight(query.matches)
    query.addEventListener('change', update)
    return () => query.removeEventListener('change', update)
  }, [])
  return light
}

/**
 * Keeps the page in step with the appearance settings (live, as a slider
 * drags) and with the OS scheme when the theme is System. Before settings
 * load it leaves the boot script's work alone.
 */
export function useAppearance(): void {
  const loaded = useSettingsStore((s) => s.loaded)
  const appearance = useSettingsStore((s) => s.appearance)
  const nativeUiScale = useSettingsStore((s) => s.nativeUiScale)
  const systemLight = useSystemPrefersLight()
  useLayoutEffect(() => {
    if (!loaded) return
    applyAppearance(appearance, { nativeUiScale })
    writeCachedAppearance(appearance)
  }, [loaded, appearance, nativeUiScale, systemLight])
}

/** The resolved colour scheme, following Settings → Appearance → Theme. */
export function useColorScheme(): ColorScheme {
  const [scheme, setScheme] = useState<ColorScheme>(() =>
    typeof document === 'undefined' ? 'dark' : currentColorScheme()
  )
  useEffect(() => {
    const onChange = (event: Event): void => {
      setScheme((event as CustomEvent<ColorScheme>).detail === 'light' ? 'light' : 'dark')
    }
    setScheme(currentColorScheme())
    window.addEventListener(SCHEME_CHANGE_EVENT, onChange)
    return () => window.removeEventListener(SCHEME_CHANGE_EVENT, onChange)
  }, [])
  return scheme
}
