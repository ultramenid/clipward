import { useSyncExternalStore } from 'react'
import { create } from 'zustand'
import { useSettingsStore } from './use-settings-store'

/** Where the collapsed state lived before it became an appearance setting. */
const LEGACY_COLLAPSED_KEY = 'clipward.sidebar.collapsed'

/** Windows narrower than this always get the icon rail, whatever the preference. */
const WIDE_QUERY = '(min-width: 1024px)'

interface SidebarState {
  /**
   * Collapse or expand the sidebar (⌘\ and the title bar button). The choice
   * is Settings → Appearance → Collapsed rail, saved with the other settings.
   */
  toggle: () => void
}

export const useSidebarStore = create<SidebarState>(() => ({
  toggle: () => {
    const settings = useSettingsStore.getState()
    void settings.setAppearance({ collapsedRail: !settings.appearance.collapsedRail }).catch(() => {
      // The rail still toggles for this session; Settings shows the save error next time.
    })
  }
}))

/**
 * Carry over a collapsed sidebar chosen before the setting existed, once.
 * Call after settings load.
 */
export function migrateLegacyCollapsed(): void {
  let legacy: string | null = null
  try {
    legacy = localStorage.getItem(LEGACY_COLLAPSED_KEY)
    if (legacy != null) localStorage.removeItem(LEGACY_COLLAPSED_KEY)
  } catch {
    return
  }
  const settings = useSettingsStore.getState()
  if (legacy === '1' && !settings.appearance.collapsedRail) {
    void settings.setAppearance({ collapsedRail: true }).catch(() => {})
  }
}

function subscribeWide(onChange: () => void): () => void {
  const query = window.matchMedia(WIDE_QUERY)
  query.addEventListener('change', onChange)
  return () => query.removeEventListener('change', onChange)
}

/** True when the window is wide enough for the full sidebar. */
export function useIsWide(): boolean {
  return useSyncExternalStore(subscribeWide, () => window.matchMedia(WIDE_QUERY).matches)
}

/** The full sidebar shows only when the window is wide and the rail setting is off. */
export function useSidebarExpanded(): boolean {
  const collapsed = useSettingsStore((s) => s.appearance.collapsedRail)
  return useIsWide() && !collapsed
}
