import type { ReactNode } from 'react'
import { PanelLeft } from 'lucide-react'
import { NAV_ITEMS, SIDEBAR_SHORTCUT_KEY, SIDEBAR_WIDTH, Sidebar, TRAFFIC_LIGHT_INSET, type Page } from './Sidebar'
import { cn, isMac, MOD_KEY } from '../lib/utils'
import { useIsWide, useSidebarExpanded, useSidebarStore } from '../store/use-sidebar-store'

interface LayoutProps {
  children: ReactNode
  currentPage: Page
  onNavigate: (page: Page) => void
}

/**
 * Sidebar on the left; on the right a 40px title bar over the page, which
 * scrolls on its own. The window uses an overlay title bar with a hidden
 * title, so both top strips are drag regions and the sidebar's strip holds
 * the macOS traffic lights.
 */
export function Layout({ children, currentPage, onNavigate }: LayoutProps): React.JSX.Element {
  return (
    <div className="app-bg flex h-screen w-screen overflow-hidden text-ink">
      <Sidebar currentPage={currentPage} onNavigate={onNavigate} />
      <main className="flex min-w-0 flex-1 flex-col">
        <TitleBar currentPage={currentPage} />
        <div id="page-scroll" className="relative min-h-0 flex-1 overflow-y-auto overscroll-none">
          {children}
        </div>
      </main>
    </div>
  )
}

const GROUP_LABEL: Record<(typeof NAV_ITEMS)[number]['group'], string> = { studio: 'Studio', app: 'App' }

/** Title bar: sidebar toggle and where you are. The whole strip drags the window. */
function TitleBar({ currentPage }: { currentPage: Page }): React.JSX.Element {
  const wide = useIsWide()
  const expanded = useSidebarExpanded()
  const toggle = useSidebarStore((s) => s.toggle)
  const item = NAV_ITEMS.find((n) => n.id === currentPage) ?? NAV_ITEMS[0]
  const label = expanded ? 'Collapse sidebar' : 'Expand sidebar'
  // In the rail the traffic lights spill past the sidebar's edge.
  const inset = isMac && !expanded ? TRAFFIC_LIGHT_INSET - SIDEBAR_WIDTH.rail : 0

  return (
    <header
      data-tauri-drag-region="deep"
      className="flex h-10 shrink-0 select-none items-center gap-1 border-b border-stroke pr-3"
      style={{ paddingLeft: 6 + inset }}
    >
      {wide && (
        <button
          type="button"
          onClick={toggle}
          aria-label={label}
          aria-expanded={expanded}
          aria-controls="sidebar"
          title={`${label} (${MOD_KEY}${SIDEBAR_SHORTCUT_KEY})`}
          data-tauri-drag-region="false"
          className="grid h-7 w-7 shrink-0 place-items-center rounded-md text-content/50 transition-colors duration-feedback hover:bg-content/10 hover:text-content"
        >
          <PanelLeft className="h-3.5 w-3.5" strokeWidth={1.75} />
        </button>
      )}
      <nav aria-label="Breadcrumb" className={cn('flex min-w-0 items-center gap-2 text-[13px]', wide ? 'px-1.5' : 'px-2')}>
        <span className="shrink-0 text-content/45">{GROUP_LABEL[item.group]}</span>
        <span aria-hidden className="shrink-0 text-content/25">›</span>
        <span aria-current="page" className="min-w-0 truncate text-content">{item.label}</span>
      </nav>
    </header>
  )
}
