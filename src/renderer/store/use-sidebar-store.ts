import { useSyncExternalStore } from 'react'
import { create } from 'zustand'

const COLLAPSED_STORAGE_KEY = 'clipward.sidebar.collapsed'

/** Windows narrower than this always get the icon rail, whatever the preference. */
const WIDE_QUERY = '(min-width: 1024px)'

function readCollapsed(): boolean {
  try {
    return localStorage.getItem(COLLAPSED_STORAGE_KEY) === '1'
  } catch {
    return false
  }
}

function saveCollapsed(collapsed: boolean): void {
  try {
    localStorage.setItem(COLLAPSED_STORAGE_KEY, collapsed ? '1' : '0')
  } catch {
    // Only a convenience; the sidebar opens expanded next time.
  }
}

interface SidebarState {
  /** The user's choice. Remembered across launches. */
  collapsed: boolean
  toggle: () => void
}

export const useSidebarStore = create<SidebarState>((set, get) => ({
  collapsed: readCollapsed(),
  toggle: () => {
    const collapsed = !get().collapsed
    saveCollapsed(collapsed)
    set({ collapsed })
  }
}))

function subscribeWide(onChange: () => void): () => void {
  const query = window.matchMedia(WIDE_QUERY)
  query.addEventListener('change', onChange)
  return () => query.removeEventListener('change', onChange)
}

/** True when the window is wide enough for the full sidebar. */
export function useIsWide(): boolean {
  return useSyncExternalStore(subscribeWide, () => window.matchMedia(WIDE_QUERY).matches)
}

/** The full sidebar shows only when the window is wide and the user hasn't collapsed it. */
export function useSidebarExpanded(): boolean {
  const collapsed = useSidebarStore((s) => s.collapsed)
  return useIsWide() && !collapsed
}
