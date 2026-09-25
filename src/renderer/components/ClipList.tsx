import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { ArrowLeft, Check, Clapperboard, Download, FolderOpen, Plus } from 'lucide-react'
import { basename, cn, errorMessage } from '../lib/utils'
import { getApi } from '../lib/ipc'
import { clipFilePath } from '../lib/thumbnails'
import type { ApiCosts, ClipArtifact, JobOutput } from '../store/use-job-store'
import { isManualRun } from '../../shared/job-output'
import { ClipCard } from './ClipCard'
import { RunStats } from './RunStats'
import { Page } from './ui/Page'
import { PageHeader } from './ui/PageHeader'
import { Button } from './ui/Button'
import { Checkbox } from './ui/Checkbox'
import { EmptyState } from './ui/EmptyState'
import { Callout } from './ui/Callout'
import { Segmented } from './ui/Segmented'

type Sort = 'score' | 'timeline'

interface ClipListProps {
  output: JobOutput
  /** The directory containing job_output.json, available for Library runs without clips. */
  outputDir?: string
  /** Shown above the title, e.g. a back link from the Library. */
  leading?: ReactNode
  onNewClip?: () => void
}

export function ClipList({ output, outputDir: runDirectory, leading, onNewClip }: ClipListProps): React.JSX.Element {
  // Manual runs have no scores: they list in timeline order with no ranking.
  const manualRun = isManualRun(output)
  const [sort, setSort] = useState<Sort>(manualRun ? 'timeline' : 'score')
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [exporting, setExporting] = useState(false)
  const noticeTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const exportingRef = useRef(false)
  const [exportError, setExportError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  // Older Clipward engine runs do not record output format; use the first thumbnail for those.
  const [aspect, setAspect] = useState<number | null>(null)
  const settings = output.metrics?.requested_settings
  const requestedAspect = settings && typeof settings === 'object' && !Array.isArray(settings)
    ? (settings as Record<string, unknown>).aspect_ratio : null
  const videoSpeed = settings && typeof settings === 'object' && !Array.isArray(settings)
    ? (settings as Record<string, unknown>).video_speed : null
  const vertical = requestedAspect === '9:16' ? true : requestedAspect === '16:9' ? false : aspect == null ? true : aspect < 1

  useEffect(() => () => {
    if (noticeTimer.current) clearTimeout(noticeTimer.current)
  }, [])

  useEffect(() => {
    if (noticeTimer.current) clearTimeout(noticeTimer.current)
    setSelected(new Set())
    setAspect(null)
    setExportError(null)
    setNotice(null)
  }, [output])

  const firstClip = output.clips[0]
  const outputDir = runDirectory ?? (firstClip ? clipFilePath(firstClip.s3_url).replace(/[\\/][^\\/]+$/, '') : '')

  const topIndex = useMemo(() => {
    let best: ClipArtifact | null = null
    for (const clip of output.clips) if (!best || clip.virality_score > best.virality_score) best = clip
    return best?.clip_index ?? null
  }, [output.clips])

  const clips = useMemo(() => {
    const list = [...output.clips]
    return sort === 'score' && !manualRun
      ? list.sort((a, b) => b.virality_score - a.virality_score)
      : list.sort((a, b) => a.start_time_ms - b.start_time_ms)
  }, [output.clips, sort, manualRun])

  const allSelected = selected.size > 0 && selected.size === clips.length

  const toggle = (index: number): void => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(index)) next.delete(index)
      else next.add(index)
      return next
    })
  }

  const exportSelected = async (): Promise<void> => {
    const picked = clips.filter((c) => selected.has(c.clip_index))
    if (picked.length === 0 || exportingRef.current) return
    exportingRef.current = true
    setExporting(true)
    setExportError(null)
    try {
      const result = await getApi().clips.bulkExport(
        picked.map((c) => ({ path: clipFilePath(c.s3_url), name: c.summary || `Clip ${c.clip_index + 1}` }))
      )
      if (result.success) {
        setNotice(`Exported ${result.count} clip${result.count === 1 ? '' : 's'}${result.destDir ? ` to ${basename(result.destDir)}` : ''}${result.failedCount ? `; ${result.failedCount} could not be copied` : ''}`)
        if (noticeTimer.current) clearTimeout(noticeTimer.current)
        noticeTimer.current = setTimeout(() => setNotice(null), 3500)
      } else if (result.failedCount) {
        setExportError('No clips could be copied to that folder.')
      }
    } catch (err) {
      setExportError(errorMessage(err, 'Could not export clips. Please try again.'))
    } finally {
      exportingRef.current = false
      setExporting(false)
    }
  }

  const costs = readCosts(output.metrics?.api_costs)
  const framingNotice = framingProblem(output, vertical)
  const analysisNotice = sourceAnalysisNotice(output)

  return (
    <Page width="wide">
      <PageHeader
        leading={leading}
        eyebrow={leading ? undefined : 'Your clips'}
        title={output.source_video_title || 'Untitled video'}
        actions={
          <>
            {outputDir && (
              <Button icon={<FolderOpen className="h-3.5 w-3.5" />} onClick={() => getApi().shell.openPath(outputDir)}>
                Open folder
              </Button>
            )}
            {onNewClip && (
              <Button variant="primary" icon={<Plus className="h-3.5 w-3.5" />} onClick={onNewClip}>
                New clip
              </Button>
            )}
          </>
        }
      />

      {typeof videoSpeed === 'number' && videoSpeed > 1 && (
        <p className="mt-3 text-caption text-ink-secondary">All clips exported at {videoSpeed}× speed · Original voice pitch</p>
      )}

      {exportError && (
        <Callout tone="danger" className="mt-3" onDismiss={() => setExportError(null)}>
          {exportError}
        </Callout>
      )}

      <div className="mt-4">
        <RunStats key={output.job_id} output={output} costs={costs} videoSpeed={typeof videoSpeed === 'number' && videoSpeed > 1 ? videoSpeed : null} />
      </div>

      {framingNotice && (
        <Callout tone="warning" className="mt-3">
          <span className="text-ink-secondary">{framingNotice}</span>
        </Callout>
      )}

      {analysisNotice && (
        <Callout tone="warning" className="mt-3">
          <span className="text-ink-secondary">{analysisNotice}</span>
        </Callout>
      )}

      {/* Floating card toolbar; sticks to the top of the scroll area, under the title bar. */}
      <div className="popover-surface sticky top-2 z-10 mt-4 flex items-center justify-between gap-3 rounded-lg py-1 pl-2.5 pr-1 shadow-none">
        <div className="flex min-w-0 items-center gap-3">
          <Checkbox
            checked={allSelected}
            indeterminate={selected.size > 0 && !allSelected}
            onChange={() => setSelected(allSelected ? new Set() : new Set(clips.map((c) => c.clip_index)))}
            label="Select all clips"
          />
          <span className="whitespace-nowrap text-caption text-ink-secondary">
            {selected.size > 0 ? (
              <>
                <span className="font-medium text-ink">{selected.size}</span> selected
              </>
            ) : (
              `${clips.length} clips`
            )}
          </span>
          {notice && (
            <span className="inline-flex h-[1.125rem] min-w-0 items-center gap-1 truncate rounded border border-success/25 bg-success/10 px-1.5 text-meta font-medium text-success animate-fade-in">
              <Check className="h-2.5 w-2.5 shrink-0" strokeWidth={3} />
              <span className="truncate" title={notice}>{notice}</span>
            </span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {selected.size > 0 && (
            <div className="flex items-center gap-1.5 animate-fade-in">
              <Button variant="ghost" size="sm" onClick={() => setSelected(new Set())}>
                Clear
              </Button>
              <Button
                size="sm"
                variant="primary"
                loading={exporting}
                icon={<Download className="h-3.5 w-3.5" />}
                onClick={exportSelected}
              >
                Export {selected.size}
              </Button>
              <span aria-hidden className="mx-1 h-4 w-px bg-stroke" />
            </div>
          )}
          {!manualRun && (
            <Segmented<Sort>
              label="Sort clips"
              size="sm"
              value={sort}
              onChange={setSort}
              options={[
                { value: 'score', label: 'Best first' },
                { value: 'timeline', label: 'Timeline' }
              ]}
            />
          )}
        </div>
      </div>

      {clips.length === 0 ? (
        <EmptyState
          className="mt-4"
          icon={<Clapperboard />}
          title="No clips in this run"
          description="The run completed without saved clips. Check the run log for details."
        />
      ) : (
        <div
          className={cn(
            'mt-3 grid gap-2.5',
            vertical
              ? 'grid-cols-[repeat(auto-fill,minmax(140px,1fr))]'
              : 'grid-cols-[repeat(auto-fill,minmax(220px,1fr))]'
          )}
        >
          {clips.map((clip) => (
            <ClipCard
              key={clip.clip_index}
              clip={clip}
              vertical={vertical}
              topPick={!manualRun && clip.clip_index === topIndex && clips.length > 1}
              showScore={!manualRun}
              selected={selected.has(clip.clip_index)}
              selecting={selected.size > 0}
              onToggleSelect={() => toggle(clip.clip_index)}
              onAspect={aspect == null ? setAspect : undefined}
            />
          ))}
        </div>
      )}

    </Page>
  )
}

/** Quiet link that returns from a run to the list it was opened from (ClipList's `leading`). */
export function BackLink({ label, onClick }: { label: string; onClick: () => void }): React.JSX.Element {
  return (
    <button
      type="button"
      onClick={onClick}
      className="-ml-2 inline-flex h-6 items-center gap-1 rounded-md pl-1.5 pr-2 text-caption text-ink-secondary transition-colors duration-feedback hover:bg-content/10 hover:text-content"
    >
      <ArrowLeft className="h-3.5 w-3.5" strokeWidth={1.75} />
      {label}
    </button>
  )
}

/** Persisted runs may contain incomplete cost data; only display complete sections. */
function readCosts(value: unknown): ApiCosts | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const raw = value as Record<string, unknown>
  const amount = raw.total_estimated_cost_usd
  if (typeof amount !== 'number' || !Number.isFinite(amount) || amount < 0) return null
  const section = (name: string): Record<string, unknown> | null => {
    const part = raw[name]
    return part && typeof part === 'object' && !Array.isArray(part) ? part as Record<string, unknown> : null
  }
  const validMoney = (value: unknown): value is number =>
    typeof value === 'number' && Number.isFinite(value) && value >= 0
  const result: ApiCosts = { total_estimated_cost_usd: amount, ...(raw.cost_incomplete === true ? { cost_incomplete: true } : {}) }
  const transcription = section('transcription')
  if (transcription && typeof transcription.provider === 'string' && typeof transcription.model === 'string' &&
      validMoney(transcription.audio_duration_seconds) && validMoney(transcription.estimated_cost_usd)) {
    result.transcription = transcription as unknown as NonNullable<ApiCosts['transcription']>
  }
  const planning = section('planning')
  if (planning && typeof planning.model === 'string' && validMoney(planning.total_tokens) &&
      validMoney(planning.attempts) && validMoney(planning.estimated_cost_usd)) {
    result.planning = planning as unknown as NonNullable<ApiCosts['planning']>
  }
  const layoutVision = section('layout_vision')
  if (layoutVision && typeof layoutVision.model === 'string' && validMoney(layoutVision.estimated_cost_usd)) {
    result.layout_vision = layoutVision as unknown as NonNullable<ApiCosts['layout_vision']>
  }
  return result
}

/** Explain when a vertical clip kept its whole frame or fell back during rendering. */
export function framingProblem(output: JobOutput, vertical: boolean): string | null {
  if (!vertical || output.clips.length === 0) return null
  const requested = output.metrics?.requested_settings
  const requestedClassic = requested && typeof requested === 'object' && !Array.isArray(requested) &&
    (requested as Record<string, unknown>).layout_style !== 'auto'
  if (requestedClassic) return null
  if (output.metrics?.smart_framing_available === false) {
    return 'Smart framing was unavailable, so every clip used the classic whole-frame layout. OpenCV or the face model is missing from the engine; run Settings → System check.'
  }
  const fallbacks = output.clips.filter((clip) => clip.render_fallback).length
  const notices: string[] = []
  if (fallbacks > 0) {
    notices.push(`${fallbacks} of ${output.clips.length} clips couldn't render with smart framing and used the classic whole-frame layout instead. The log has the details.`)
  }
  const clipIndices = new Set(output.clips.map((clip) => clip.clip_index))
  const wholeFrame = new Set<number>()
  const layouts = output.metrics?.clip_layouts
  if (Array.isArray(layouts)) {
    for (const layout of layouts) {
      if (layout && typeof layout === 'object' && layout.framing_status === 'whole_frame_auto' &&
        Number.isSafeInteger(layout.clip_index) && clipIndices.has(layout.clip_index)) {
        wholeFrame.add(layout.clip_index)
      }
    }
  }
  if (wholeFrame.size > 0) {
    const sorted = [...wholeFrame].sort((a, b) => a - b)
    const shown = sorted.slice(0, 5).map((index) => index + 1).join(', ')
    const more = sorted.length > 5 ? ` and ${sorted.length - 5} more` : ''
    notices.push(`Smart framing kept the whole frame for clip${sorted.length === 1 ? '' : 's'} ${shown}${more}. Review the framing if you expected a closer crop.`)
  }
  return notices.join(' ') || null
}

export function sourceAnalysisNotice(output: JobOutput): string | null {
  if (output.metrics?.planning_source !== 'visual') return null
  if (output.metrics.transcription_status === 'no_speech') {
    return 'No speech was detected. Clips were selected from sampled video frames, and spoken-word captions are unavailable for this run.'
  }
  if (output.metrics.transcription_status === 'failed') {
    return 'Transcription failed. Clips were selected from sampled video frames, and spoken-word captions are unavailable for this run.'
  }
  return null
}
