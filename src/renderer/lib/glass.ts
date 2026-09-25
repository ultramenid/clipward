import { useEffect, useState } from 'react'
import { useSettingsStore } from '../store/use-settings-store'

/**
 * Window glass: on macOS and Windows the app shell draws a native material
 * (vibrancy, Mica or acrylic) behind a transparent window, and `html.glass`
 * turns the backdrop, sidebar and title bar into translucent tints over it
 * (globals.css, DESIGN.md). Linux, the setting switched off, or the OS asking
 * for reduced transparency keep every surface opaque.
 */
export const GLASS_CLASS = 'glass'

const REDUCED_TRANSPARENCY = '(prefers-reduced-transparency: reduce)'

export function glassActive(supported: boolean, enabled: boolean, reducedTransparency: boolean): boolean {
  return supported && enabled && !reducedTransparency
}

function prefersReducedTransparency(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia(REDUCED_TRANSPARENCY).matches
}

export function useReducedTransparency(): boolean {
  const [reduced, setReduced] = useState(prefersReducedTransparency)
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return
    const query = window.matchMedia(REDUCED_TRANSPARENCY)
    const update = (): void => setReduced(query.matches)
    query.addEventListener('change', update)
    return () => query.removeEventListener('change', update)
  }, [])
  return reduced
}

/** Keeps `html.glass` in step with the platform, the setting and the OS preference. */
export function useWindowGlass(): void {
  const supported = useSettingsStore((s) => s.windowGlassSupported)
  const enabled = useSettingsStore((s) => s.windowGlass)
  const reduced = useReducedTransparency()
  const active = glassActive(supported, enabled, reduced)
  useEffect(() => {
    document.documentElement.classList.toggle(GLASS_CLASS, active)
  }, [active])
}
