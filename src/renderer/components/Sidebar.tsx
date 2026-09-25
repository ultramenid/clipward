import { ChevronRight, Film, Layers, Settings, WandSparkles, type LucideIcon } from 'lucide-react'
import { cn, isMac, MOD_KEY, sourceLabel } from '../lib/utils'
import { useSidebarExpanded } from '../store/use-sidebar-store'
import { useActiveJobs } from '../store/use-job-store'
import { useSetupState } from '../store/use-settings-store'
import { APP_VERSION } from '../config/brand'
import { ClipwardLogo } from './brand/ClipwardLogo'
import { ProgressBar } from './ui/ProgressBar'
import { StatusDot } from './ui/Badge'
import { STAGE_LABELS } from './JobProgress'

export type Page = 'clip' | 'library' | 'jobs' | 'settings'

export const NAV_ITEMS: { id: Page; label: string; icon: LucideIcon; shortcut: string; group: 'studio' | 'app' }[] = [
  { id: 'clip', label: 'Create', icon: WandSparkles, shortcut: '1', group: 'studio' },
  { id: 'library', label: 'Library', icon: Film, shortcut: '2', group: 'studio' },
  { id: 'jobs', label: 'Jobs', icon: Layers, shortcut: '3', group: 'studio' },
  { id: 'settings', label: 'Settings', icon: Settings, shortcut: ',', group: 'app' }
]

interface SidebarProps {
  currentPage: Page
  onNavigate: (page: Page) => void
}

/** Shortcut that collapses and expands the sidebar (⌘\ on macOS, Ctrl+\ elsewhere). */
export const SIDEBAR_SHORTCUT_KEY = '\\'

/** Width of the traffic-light inset on macOS (overlay title bar). */
export const TRAFFIC_LIGHT_INSET = 78
/** Sidebar widths: full column and icon rail. */
export const SIDEBAR_WIDTH = { expanded: 220, rail: 52 } as const

/**
 * Full sidebar (220px), or a 52px icon rail when the user collapses it or the
 * window is narrower than 1024px. It sits on the darker sidebar tone with a
 * hairline edge; its top 40px strip is a window drag handle that holds the
 * macOS traffic lights. Labels stay available to screen readers and as
 * tooltips in the rail. The collapse toggle lives in the title bar.
 */
export function Sidebar({ currentPage, onNavigate }: SidebarProps): React.JSX.Element {
  const expanded = useSidebarExpanded()
  return (
    <aside
      id="sidebar"
      className={cn(
        'sidebar-bg relative flex shrink-0 flex-col overflow-hidden border-r border-stroke transition-[width] duration-feedback ease-out',
        expanded ? 'w-[220px]' : 'w-[52px]'
      )}
    >
      {/* Title-bar strip: drag handle and home of the macOS traffic lights. */}
      <div
        data-tauri-drag-region="deep"
        className={cn('flex h-10 shrink-0 items-center', expanded ? 'justify-end px-3.5' : 'justify-center')}
        style={isMac && expanded ? { paddingLeft: TRAFFIC_LIGHT_INSET } : undefined}
      >
        {expanded ? (
          <ClipwardLogo className={cn('h-4 opacity-90', !isMac && 'mr-auto')} />
        ) : !isMac ? (
          <ClipwardLogo variant="mark" className="h-5" />
        ) : null}
      </div>

      <nav className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto overscroll-none px-2 py-3" aria-label="Main">
        <NavGroup label="Studio" expanded={expanded}>
          {NAV_ITEMS.filter((item) => item.group === 'studio').map((item) => (
            <NavButton key={item.id} item={item} expanded={expanded} active={currentPage === item.id} onNavigate={onNavigate} />
          ))}
        </NavGroup>
        <NavGroup label="App" expanded={expanded} divider>
          {NAV_ITEMS.filter((item) => item.group === 'app').map((item) => (
            <NavButton key={item.id} item={item} expanded={expanded} active={currentPage === item.id} onNavigate={onNavigate} />
          ))}
        </NavGroup>
      </nav>

      <div className="flex shrink-0 flex-col gap-1 p-2">
        {currentPage !== 'jobs' && <ActiveJobsCard expanded={expanded} onOpen={() => onNavigate('jobs')} />}
      </div>
      <div className="shrink-0 border-t border-stroke p-2">
        <SetupStatus expanded={expanded} onOpenSettings={() => onNavigate('settings')} />
      </div>
    </aside>
  )
}

function NavGroup({ label, expanded, divider = false, children }: {
  label: string
  expanded: boolean
  /** In the rail, a hairline replaces the label to separate groups. */
  divider?: boolean
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div className="flex flex-col gap-px">
      {expanded ? (
        <p aria-hidden className="px-2 pb-1 text-xs font-semibold text-content/35">{label}</p>
      ) : divider ? (
        <span aria-hidden className="mx-2 mb-1 h-px bg-stroke" />
      ) : null}
      {children}
    </div>
  )
}

function NavButton({ item, expanded, active, onNavigate }: {
  item: (typeof NAV_ITEMS)[number]
  expanded: boolean
  active: boolean
  onNavigate: (page: Page) => void
}): React.JSX.Element {
  const Icon = item.icon
  const liveJobs = useActiveJobs().length
  const badge = item.id === 'jobs' && liveJobs > 0 ? liveJobs : null
  return (
    <button
      type="button"
      onClick={() => onNavigate(item.id)}
      aria-current={active ? 'page' : undefined}
      aria-label={badge ? `${item.label}, ${badge} active` : item.label}
      title={expanded ? undefined : badge ? `${item.label} · ${badge} active` : `${item.label} (${MOD_KEY}${item.shortcut})`}
      className={cn(
        'group relative flex h-8 w-full items-center gap-2 rounded-md text-left transition-colors duration-feedback ease-out',
        expanded ? 'px-2' : 'justify-center',
        active ? 'bg-selection text-content' : 'text-content/50 hover:bg-content/10 hover:text-content'
      )}
    >
      <Icon className={cn('h-4 w-4 shrink-0', active ? 'opacity-90' : 'opacity-70')} strokeWidth={1.75} />
      {!expanded && badge && (
        <span aria-hidden className="absolute right-1.5 top-1.5 h-1.5 w-1.5 rounded-full bg-accent" />
      )}
      {expanded && (
        <>
          <span className="min-w-0 flex-1 truncate text-sm font-medium leading-tight">{item.label}</span>
          {badge ? (
            <span aria-hidden className="grid min-w-4 place-items-center rounded-full bg-accent px-1 text-[10px] font-semibold leading-4 text-accent-ink tabular">
              {badge > 99 ? '99+' : badge}
            </span>
          ) : (
            <kbd aria-hidden className="shrink-0 font-sans text-[11px] text-content/40">
              {MOD_KEY}
              {item.shortcut}
            </kbd>
          )}
        </>
      )}
    </button>
  )
}

/** Live jobs at a glance: one job shows its stage; several show a count and their average progress. */
function ActiveJobsCard({ expanded, onOpen }: { expanded: boolean; onOpen: () => void }): React.JSX.Element | null {
  const jobs = useActiveJobs()
  if (jobs.length === 0) return null
  const running = jobs.filter((job) => job.status !== 'queued')
  const queued = jobs.length - running.length
  const percent = running.length > 0 ? Math.round(running.reduce((sum, job) => sum + job.percent, 0) / running.length) : 0
  const single = jobs.length === 1 ? jobs[0] : null
  const title = single
    ? (STAGE_LABELS[single.status] ?? 'Working')
    : `${running.length} running${queued > 0 ? ` · ${queued} queued` : ''}`

  return (
    <button
      type="button"
      onClick={onOpen}
      aria-label={`${title}, ${percent}%. Open jobs`}
      title={expanded ? undefined : `${title} · ${percent}%`}
      className={cn('tile tile-hover group w-full rounded-lg text-left animate-fade-in', expanded ? 'px-2.5 py-2' : 'px-1.5 py-1.5')}
    >
      <div className={cn('flex items-center gap-2 text-xs', !expanded && 'justify-center')}>
        {expanded && <StatusDot tone="accent" pulse />}
        {expanded && <span className="truncate font-medium text-content">{title}</span>}
        <span className={cn('font-mono text-2xs tabular text-accent-fg', expanded && 'ml-auto')}>{percent}%</span>
      </div>
      <ProgressBar value={percent} className={cn('h-0.5', expanded ? 'mt-2' : 'mt-1.5')} />
      {expanded && (
        <p className="mt-1.5 flex items-center gap-1 text-2xs text-content/45">
          <span className="truncate">{single ? sourceLabel(single.request.videoUrl) : 'View all jobs'}</span>
          <ChevronRight className="ml-auto h-3 w-3 shrink-0 opacity-0 transition-opacity group-hover:opacity-100" />
        </p>
      )}
    </button>
  )
}

function SetupStatus({ expanded, onOpenSettings }: { expanded: boolean; onOpenSettings: () => void }): React.JSX.Element {
  const { ready, providerReady, whisperReady, toolsOk } = useSetupState()

  let tone: 'success' | 'warning' | 'danger' | 'idle' = 'success'
  let label = 'Ready to clip'
  if (!providerReady || !whisperReady) {
    tone = 'warning'
    label = !providerReady ? 'Choose an AI provider' : 'Download Whisper model'
  } else if (toolsOk === false) {
    tone = 'danger'
    label = 'System check failed'
  } else if (toolsOk === null) {
    tone = 'idle'
    label = 'Checking system…'
  }

  return (
    <button
      type="button"
      onClick={onOpenSettings}
      aria-label={label}
      title={expanded ? undefined : `${label} · v${APP_VERSION}`}
      className={cn(
        'flex w-full min-w-0 items-center gap-2.5 rounded-md text-left transition-colors duration-feedback hover:bg-content/10',
        expanded ? 'px-2 py-1.5' : 'h-8 justify-center',
        ready ? 'text-content/60 hover:text-content' : 'text-content/80 hover:text-content'
      )}
    >
      <span className="grid h-4 w-4 shrink-0 place-items-center">
        <StatusDot tone={tone} />
      </span>
      {expanded && (
        <span className="flex min-w-0 flex-col">
          <span className="truncate text-[13px] leading-tight">{label}</span>
          <span className="font-mono text-[11px] leading-tight text-content/40 tabular">v{APP_VERSION}</span>
        </span>
      )}
    </button>
  )
}
