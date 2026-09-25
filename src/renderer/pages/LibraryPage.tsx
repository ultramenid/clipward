import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { AlertTriangle, Clapperboard, FolderOpen, ListVideo, RefreshCw, Search, Sparkles } from 'lucide-react'
import { getApi } from '../lib/ipc'
import { cn, errorMessage, formatRelativeDate, formatUsd, localFileUrl } from '../lib/utils'
import { clipFilePath, loadThumbnail } from '../lib/thumbnails'
import { useSettingsStore } from '../store/use-settings-store'
import type { JobOutput } from '../store/use-job-store'
import { parseJobOutput } from '../../shared/job-output'
import type { HistoryEntry } from '../../preload/index'
import { BackLink, ClipList } from '../components/ClipList'
import { Page } from '../components/ui/Page'
import { PageHeader } from '../components/ui/PageHeader'
import { Button } from '../components/ui/Button'
import { TextInput } from '../components/ui/Field'
import { EmptyState } from '../components/ui/EmptyState'
import { Callout } from '../components/ui/Callout'
import { Skeleton } from '../components/ui/Skeleton'
import type { Page as AppPage } from '../components/Sidebar'

export function LibraryPage({ onNavigate }: { onNavigate: (page: AppPage) => void }): React.JSX.Element {
  const outputDirectory = useSettingsStore((s) => s.outputDirectory)
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null)
  const [query, setQuery] = useState('')
  const [open, setOpen] = useState<{ entry: HistoryEntry; output: JobOutput } | null>(null)
  const requestId = useRef(0)
  const openRequestId = useRef(0)
  const [error, setError] = useState<string | null>(null)
  const [refreshing, setRefreshing] = useState(false)

  const load = useCallback(async () => {
    const request = ++requestId.current
    setRefreshing(true)
    setError(null)
    try {
      const result = await getApi().history.list()
      if (request === requestId.current) setEntries(result.filter((entry) => entry.status === 'completed'))
    } catch (err) {
      if (request === requestId.current) {
        setEntries((previous) => previous ?? [])
        setError(errorMessage(err, 'Could not load the library. Please refresh to retry.'))
      }
    } finally {
      if (request === requestId.current) setRefreshing(false)
    }
  }, [])

  useEffect(() => {
    load()
  }, [load])

  const filtered = useMemo(() => {
    if (!entries) return []
    const q = query.trim().toLowerCase()
    return q ? entries.filter((e) => e.videoTitle.toLowerCase().includes(q)) : entries
  }, [entries, query])

  const totalClips = entries?.reduce((sum, e) => sum + e.clipCount, 0) ?? 0

  const openRun = async (entry: HistoryEntry): Promise<void> => {
    const request = ++openRequestId.current
    setError(null)
    try {
      const output = await getApi().history.getJob(entry.outputDir)
      if (request !== openRequestId.current) return
      if (!output) {
        setError('This run is no longer available. Its files may have moved or been removed.')
        return
      }
      const parsed = parseJobOutput(output)
      if (!parsed) {
        setError('This run has an unsupported or damaged result file.')
        return
      }
      setOpen({ entry, output: parsed })
      document.getElementById('page-scroll')?.scrollTo({ top: 0 })
    } catch (err) {
      if (request === openRequestId.current) setError(errorMessage(err, 'Could not open this run.'))
    }
  }

  if (open) {
    return (
      <ClipList
        output={open.output}
        outputDir={open.entry.outputDir}
        leading={<BackLink label="Library" onClick={() => setOpen(null)} />}
      />
    )
  }

  return (
    <Page width="wide">
      <PageHeader
        eyebrow="Studio"
        title="Library"
        description={
          entries && entries.length > 0
            ? `${entries.length} run${entries.length === 1 ? '' : 's'} · ${totalClips} clips`
            : 'Every run you finish lands here.'
        }
        actions={
          <>
            <Button variant="ghost" icon={<ListVideo className="h-4 w-4" />} onClick={() => onNavigate('jobs')}>
              Jobs
            </Button>
            <Button
              variant="ghost"
              iconOnly
              aria-label="Refresh"
              title="Refresh"
              onClick={load}
              disabled={refreshing}
              icon={<RefreshCw className={cn('h-4 w-4', refreshing && 'animate-spin')} />}
            />
            {outputDirectory && (
              <Button icon={<FolderOpen className="h-4 w-4" />} onClick={() => getApi().shell.openPath(outputDirectory)}>
                Open folder
              </Button>
            )}
          </>
        }
      />

      {error && (
        <Callout tone="danger" className="mt-4" onDismiss={() => setError(null)}>
          {error}
        </Callout>
      )}

      {entries && entries.length > 0 && (
        <TextInput
          className="mt-5 max-w-sm rounded-full"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search by video title"
          leading={<Search className="h-3.5 w-3.5" />}
          aria-label="Search runs"
        />
      )}

      <div className="mt-4">
        {entries === null ? (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-4" aria-busy="true" aria-label="Loading library">
            {Array.from({ length: 6 }).map((_, i) => (
              <div key={i} className="glass rounded-2xl p-1.5">
                <Skeleton className="aspect-video rounded-xl" />
                <div className="space-y-2.5 px-2 pb-2 pt-3.5">
                  <Skeleton className="h-3.5 w-3/4 rounded-full" />
                  <Skeleton className="h-3 w-1/3 rounded-full" />
                </div>
              </div>
            ))}
          </div>
        ) : entries.length === 0 ? (
          <EmptyState
            icon={<Clapperboard />}
            title="No clips yet"
            description="Generate clips from a long video and every run will show up here, newest first."
            action={
              <Button variant="primary" size="lg" icon={<Sparkles className="h-4 w-4" />} onClick={() => onNavigate('clip')}>
                Create your first clips
              </Button>
            }
          />
        ) : filtered.length === 0 ? (
          <div className="glass flex flex-col items-center rounded-3xl px-5 py-10 text-center">
            <Search className="h-5 w-5 text-ink-subtle" />
            <p className="mt-3 text-sm text-ink-muted">No runs match “{query}”.</p>
          </div>
        ) : (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-4">
            {filtered.map((entry) => (
              <RunCard
                key={entry.jobId}
                entry={entry}
                onOpen={() => openRun(entry)}
                onOpenFolder={async () => {
                  try {
                    if (!await getApi().shell.openPath(entry.outputDir)) setError('This run folder is no longer available.')
                  } catch (err) {
                    setError(errorMessage(err, 'Could not open this run folder.'))
                  }
                }}
              />
            ))}
          </div>
        )}
      </div>
    </Page>
  )
}

function RunCard({ entry, onOpen, onOpenFolder }: {
  entry: HistoryEntry
  onOpen: () => void
  onOpenFolder: () => void
}): React.JSX.Element {
  const failed = entry.status !== 'completed'
  const thumb = useRunThumbnail(failed ? null : entry.outputDir)
  const [previewFailed, setPreviewFailed] = useState(false)

  return (
    <button
      onClick={failed ? onOpenFolder : onOpen}
      aria-label={failed ? `Open folder for ${entry.status === 'incomplete' ? 'unfinished' : 'unreadable'} run` : undefined}
      className={cn(
        'glass group rounded-2xl p-1.5 text-left transition-[transform,box-shadow] duration-300 ease-out',
        'hover:-translate-y-1 hover:shadow-[inset_0_1px_0_rgb(255_255_255/0.1),0_0_0_1px_rgb(255_255_255/0.08),0_28px_56px_-24px_rgb(0_0_0/0.8)]'
      )}
    >
      <div className="relative aspect-video overflow-hidden rounded-xl bg-black/40 shadow-[inset_0_0_0_1px_rgb(255_255_255/0.06)]">
        {thumb && !previewFailed ? (
          <>
            {/* Vertical clips sit on a blurred copy of themselves to fill the 16:9 frame. */}
            <img src={localFileUrl(thumb)} alt="" className="absolute inset-0 h-full w-full scale-125 object-cover opacity-60 blur-2xl saturate-150" />
            <img
              src={localFileUrl(thumb)}
              alt=""
              draggable={false}
              className="relative h-full w-full object-contain transition-transform duration-500 ease-out group-hover:scale-[1.04]"
              onError={() => setPreviewFailed(true)}
            />
          </>
        ) : failed ? (
          <div className="flex h-full items-center justify-center text-danger/70">
            <AlertTriangle className="h-6 w-6" />
          </div>
        ) : (
          <Skeleton className="h-full rounded-none" />
        )}
        {!failed && (
          <span className="glass-chip absolute bottom-2.5 right-2.5 inline-flex items-center gap-1 rounded-full px-2.5 py-0.5 text-2xs font-medium text-white">
            <Clapperboard className="h-3 w-3" />
            {entry.clipCount} clip{entry.clipCount === 1 ? '' : 's'}
          </span>
        )}
      </div>
      <div className="px-2 pb-2 pt-3.5">
        <p className="truncate text-sm font-medium text-ink" title={entry.videoTitle}>
          {failed ? `${entry.status === 'incomplete' ? 'Unfinished' : 'Unreadable'} run · Open folder` : entry.videoTitle}
        </p>
        <p className="mt-1 flex items-center gap-1.5 text-xs text-ink-subtle">
          <span>{formatRelativeDate(entry.date)}</span>
          {entry.totalCostUsd != null && (
            <>
              <span className="text-ink-faint">·</span>
              <span className="font-mono tabular">{formatUsd(entry.totalCostUsd)}</span>
            </>
          )}
        </p>
      </div>
    </button>
  )
}

/** Thumbnail of a run's best clip, loaded through the shared thumbnail queue. */
function useRunThumbnail(outputDir: string | null): string | null {
  const [thumb, setThumb] = useState<string | null>(null)
  useEffect(() => {
    setThumb(null)
    if (!outputDir) return
    let cancelled = false
    getApi()
      .history.getJob(outputDir)
      .then((raw) => {
        const output = parseJobOutput(raw)
        const best = output?.clips.reduce<JobOutput['clips'][number] | null>(
          (top, c) => (!top || c.virality_score > top.virality_score ? c : top),
          null
        )
        if (!best || cancelled) return null
        return loadThumbnail(clipFilePath(best.s3_url), best.duration_ms > 0 ? best.duration_ms / 2000 : undefined)
      })
      .then((path) => {
        if (!cancelled && path) setThumb(path)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [outputDir])
  return thumb
}
