import { useEffect, useRef, useState, type KeyboardEvent } from 'react'
import { Copy, Crosshair, Plus, X } from 'lucide-react'
import { cn, isUrl, localFileUrl } from '../lib/utils'
import {
  checkManualClips,
  formatClipLength,
  formatClipTime,
  newManualRow,
  type ManualClipRow
} from '../lib/manual-clips'
import { MAX_MANUAL_CLIPS, MAX_MANUAL_TITLE_CHARS } from '../../shared/job-contract'
import { Button } from './ui/Button'
import { TextInput } from './ui/Field'

interface ManualClipListProps {
  rows: ManualClipRow[]
  onChange: (rows: ManualClipRow[]) => void
  /** The chosen video: a local file gets a preview player for picking times. */
  source: string
  /** Video length in seconds when known (local files, once the preview loads). */
  videoSeconds: number | null
  onDuration?: (seconds: number) => void
}

const COLUMNS = 'grid-cols-[1.25rem_5.5rem_5.5rem_3.5rem_minmax(0,1fr)_auto] max-sm:grid-cols-[1.25rem_1fr_1fr_3.25rem_auto]'

/**
 * Manual clip mode's editable list: start, end and an optional title per clip.
 * Times take seconds, m:ss or h:mm:ss. Enter moves to the next clip (adding one
 * at the end), so a list can be typed without the mouse.
 */
export function ManualClipList({ rows, onChange, source, videoSeconds, onDuration }: ManualClipListProps): React.JSX.Element {
  const check = checkManualClips(rows, videoSeconds)
  const [activeId, setActiveId] = useState<string | null>(null)
  const pendingFocus = useRef<string | null>(null)
  const full = rows.length >= MAX_MANUAL_CLIPS
  const totalSeconds = check.rows.reduce((sum, row) => sum + (row.error || row.length == null ? 0 : row.length), 0)
  const validCount = check.rows.filter((row) => !row.error).length
  const active = rows.find((row) => row.id === activeId) ?? rows[rows.length - 1]
  const activeIndex = active ? rows.indexOf(active) : -1

  useEffect(() => {
    if (!pendingFocus.current) return
    document.getElementById(pendingFocus.current)?.focus()
    pendingFocus.current = null
  })

  const patch = (id: string, change: Partial<ManualClipRow>): void => {
    onChange(rows.map((row) => (row.id === id ? { ...row, ...change } : row)))
  }
  const add = (after?: number, copy?: ManualClipRow): void => {
    if (full) return
    const row = newManualRow(copy ? { start: copy.start, end: copy.end, title: copy.title } : {})
    const at = after == null ? rows.length : after + 1
    onChange([...rows.slice(0, at), row, ...rows.slice(at)])
    pendingFocus.current = `${row.id}-start`
    setActiveId(row.id)
  }
  const remove = (index: number): void => {
    const next = rows.filter((_, i) => i !== index)
    // The list never empties: removing the only clip leaves a blank one.
    const kept = next.length > 0 ? next : [newManualRow()]
    onChange(kept)
    const focus = kept[Math.min(index, kept.length - 1)]
    pendingFocus.current = `${focus.id}-start`
  }
  const onEnter = (event: KeyboardEvent<HTMLInputElement>, index: number): void => {
    if (event.key !== 'Enter' || event.metaKey || event.ctrlKey) return
    event.preventDefault()
    const next = rows[index + 1]
    if (next) document.getElementById(`${next.id}-start`)?.focus()
    else add()
  }

  return (
    <div className="space-y-3">
      {!isUrl(source) && source && active && (
        <PreviewHelper
          source={source}
          clipNumber={activeIndex + 1}
          onDuration={onDuration}
          onUse={(field, seconds) => patch(active.id, { [field]: formatClipTime(seconds) })}
        />
      )}

      <div>
        <div className={cn('grid items-center gap-2 px-0.5 pb-1.5 text-meta text-ink-tertiary', COLUMNS)} aria-hidden>
          <span className="text-right">#</span>
          <span>Start</span>
          <span>End</span>
          <span className="text-right">Length</span>
          <span className="max-sm:hidden">Title</span>
          <span />
        </div>
        <ol className="space-y-1.5" aria-label="Clips to cut">
          {rows.map((row, i) => {
            const rowCheck = check.rows[i]
            const touched = Boolean(row.start.trim() || row.end.trim() || row.title.trim())
            const error = touched ? rowCheck.error : null
            const errorId = `${row.id}-error`
            const n = i + 1
            return (
              <li key={row.id} onFocus={() => setActiveId(row.id)}>
                <div className={cn('grid items-center gap-2', COLUMNS)}>
                  <span className="text-right text-caption tabular-nums text-ink-tertiary">{n}</span>
                  <TextInput
                    id={`${row.id}-start`}
                    mono
                    placeholder="0:00"
                    inputMode="decimal"
                    value={row.start}
                    onChange={(e) => patch(row.id, { start: e.target.value })}
                    onKeyDown={(e) => onEnter(e, i)}
                    aria-label={`Clip ${n} start`}
                    aria-invalid={error != null && rowCheck.field === 'start'}
                    aria-describedby={error ? errorId : undefined}
                  />
                  <TextInput
                    id={`${row.id}-end`}
                    mono
                    placeholder="0:30"
                    inputMode="decimal"
                    value={row.end}
                    onChange={(e) => patch(row.id, { end: e.target.value })}
                    onKeyDown={(e) => onEnter(e, i)}
                    aria-label={`Clip ${n} end`}
                    aria-invalid={error != null && rowCheck.field === 'end'}
                    aria-describedby={error ? errorId : undefined}
                  />
                  <span className="text-right text-caption tabular-nums text-ink-secondary" aria-label={`Clip ${n} length`}>
                    {rowCheck.length != null && rowCheck.length > 0 ? formatClipLength(rowCheck.length) : '–'}
                  </span>
                  <TextInput
                    id={`${row.id}-title`}
                    className="max-sm:col-span-4 max-sm:col-start-2 max-sm:row-start-2"
                    placeholder={`Clip ${n}`}
                    maxLength={MAX_MANUAL_TITLE_CHARS + 20}
                    value={row.title}
                    onChange={(e) => patch(row.id, { title: e.target.value })}
                    onKeyDown={(e) => onEnter(e, i)}
                    aria-label={`Clip ${n} title (optional)`}
                    aria-invalid={error != null && rowCheck.field === 'title'}
                    aria-describedby={error ? errorId : undefined}
                  />
                  <div className="flex items-center max-sm:row-start-1 max-sm:col-start-5">
                    <Button
                      size="sm"
                      variant="ghost"
                      iconOnly
                      aria-label={`Duplicate clip ${n}`}
                      title="Duplicate"
                      onClick={() => add(i, row)}
                      disabled={full}
                      icon={<Copy className="h-3.5 w-3.5" />}
                    />
                    <Button
                      size="sm"
                      variant="ghost"
                      iconOnly
                      aria-label={`Remove clip ${n}`}
                      title="Remove"
                      onClick={() => remove(i)}
                      icon={<X className="h-3.5 w-3.5" />}
                    />
                  </div>
                </div>
                {error && (
                  <p id={errorId} role="alert" className="mt-1 pl-[1.75rem] text-caption text-danger">{error}</p>
                )}
              </li>
            )
          })}
        </ol>
      </div>

      <div className="flex flex-wrap items-center justify-between gap-2">
        <Button icon={<Plus className="h-3.5 w-3.5" />} onClick={() => add()} disabled={full}>
          Add clip
        </Button>
        <p className="text-caption tabular-nums text-ink-secondary" aria-live="polite">
          {validCount} of {rows.length} ready{totalSeconds > 0 ? ` · ${formatClipLength(totalSeconds)} total` : ''}
          {full ? ` · ${MAX_MANUAL_CLIPS} is the most per job` : ''}
        </p>
      </div>
      <p className="text-caption text-ink-secondary">
        Seconds (90), m:ss (1:30) or h:mm:ss. Each clip runs 3 seconds to 15 minutes; overlaps are fine. Enter moves to the next clip.
        {videoSeconds == null && ' Times are checked against the video once it downloads.'}
      </p>
    </div>
  )
}

/** A small player for local files: scrub to a moment, then use it as the active clip's start or end. */
function PreviewHelper({ source, clipNumber, onDuration, onUse }: {
  source: string
  clipNumber: number
  onDuration?: (seconds: number) => void
  onUse: (field: 'start' | 'end', seconds: number) => void
}): React.JSX.Element | null {
  const [position, setPosition] = useState(0)
  const [failed, setFailed] = useState(false)
  if (failed) return null
  return (
    <div className="tile flex items-center gap-3 rounded-lg p-2 max-sm:flex-col max-sm:items-stretch">
      <video
        src={localFileUrl(source)}
        preload="metadata"
        controls
        className="aspect-video w-56 shrink-0 rounded-md bg-black/60 max-sm:w-full"
        onLoadedMetadata={(e) => {
          const seconds = e.currentTarget.duration
          if (Number.isFinite(seconds) && seconds > 0) onDuration?.(seconds)
        }}
        onTimeUpdate={(e) => setPosition(e.currentTarget.currentTime)}
        onSeeked={(e) => setPosition(e.currentTarget.currentTime)}
        onError={() => setFailed(true)}
      />
      <div className="min-w-0 space-y-2">
        <p className="text-caption text-ink-secondary">
          Preview at <span className="font-mono tabular-nums text-content/80">{formatClipTime(position)}</span>. Use it for clip {clipNumber}:
        </p>
        <div className="flex flex-wrap gap-1.5">
          <Button size="sm" icon={<Crosshair className="h-3.5 w-3.5" />} onClick={() => onUse('start', position)}>
            Set as start
          </Button>
          <Button size="sm" icon={<Crosshair className="h-3.5 w-3.5" />} onClick={() => onUse('end', position)}>
            Set as end
          </Button>
        </div>
      </div>
    </div>
  )
}
