import { useEffect, useState } from 'react'
import { FolderOpen, ImageOff, Play, TrendingUp, TriangleAlert } from 'lucide-react'
import { cn, formatTimecode, isMac, localFileUrl } from '../lib/utils'
import { getApi } from '../lib/ipc'
import { clipFilePath, loadThumbnail } from '../lib/thumbnails'
import type { ClipArtifact } from '../store/use-job-store'
import { Checkbox } from './ui/Checkbox'
import { Badge } from './ui/Badge'
import { Skeleton } from './ui/Skeleton'

// How the engine framed a vertical clip (its dominant layout).
const LAYOUT_LABELS: Record<string, string> = {
  talking_head: 'Speaker',
  two_shot: 'Two people',
  screen_cam: 'Screen + webcam',
  screen: 'Whole frame',
  fit: 'Whole frame',
  center_crop: 'Center crop'
}

interface ClipCardProps {
  clip: ClipArtifact
  vertical: boolean
  topPick?: boolean
  /** Off for manual runs, whose clips have no virality score. */
  showScore?: boolean
  selected: boolean
  selecting: boolean
  onToggleSelect: () => void
  /** Reports the thumbnail's aspect ratio so the grid can size to the output. */
  onAspect?: (ratio: number) => void
  /** Opens the post dialog for this clip. */
}

export function ClipCard({
  clip,
  vertical,
  topPick,
  showScore = true,
  selected,
  selecting,
  onToggleSelect,
  onAspect
}: ClipCardProps): React.JSX.Element {
  const filePath = clipFilePath(clip.s3_url)
  const [thumb, setThumb] = useState<string | null | undefined>(undefined)
  const [aspect, setAspect] = useState<number | null>(null)
  const [hovering, setHovering] = useState(false)
  const [previewFailed, setPreviewFailed] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const title = clip.summary || `Clip ${clip.clip_index + 1}`
  const score = (clip.virality_score * 10).toFixed(1)
  const clipVertical = aspect == null ? vertical : aspect < 1
  const layout = clipVertical && Object.prototype.hasOwnProperty.call(LAYOUT_LABELS, clip.layout_type)
    ? LAYOUT_LABELS[clip.layout_type]
    : undefined

  useEffect(() => {
    let cancelled = false
    setThumb(undefined)
    setAspect(null)
    setPreviewFailed(false)
    setActionError(null)
    const seek = clip.duration_ms > 0 ? (clip.duration_ms / 1000) * 0.5 : undefined
    loadThumbnail(filePath, seek).then((path) => {
      if (!cancelled) setThumb(path)
    })
    return () => {
      cancelled = true
    }
  }, [filePath, clip.duration_ms])

  const openClip = async (): Promise<void> => {
    setActionError(null)
    try {
      if (!await getApi().shell.openPath(filePath)) setActionError('Clip file is no longer available.')
    } catch {
      setActionError('Could not open this clip.')
    }
  }

  const showInFolder = async (): Promise<void> => {
    setActionError(null)
    try {
      if (!await getApi().shell.showItemInFolder(filePath)) setActionError('Clip file is no longer available.')
    } catch {
      setActionError('Could not show this clip in its folder.')
    }
  }

  return (
    <article
      className={cn(
        'card group relative flex flex-col rounded-xl p-1 transition-[border-color,background-color] duration-feedback ease-out',
        selected ? 'border-accent/80 bg-accent/[0.06]' : 'hover:border-content/15 hover:bg-content/[0.05]'
      )}
    >
      <div
        className={cn(
          'relative overflow-hidden rounded-lg bg-black/40',
          clipVertical ? 'aspect-[9/16]' : 'aspect-video'
        )}
        onMouseEnter={() => setHovering(true)}
        onMouseLeave={() => setHovering(false)}
      >
        {thumb ? (
          <img
            src={localFileUrl(thumb)}
            alt=""
            draggable={false}
            className="absolute inset-0 h-full w-full object-cover"
            onLoad={(e) => {
              const ratio = e.currentTarget.naturalWidth / e.currentTarget.naturalHeight
              if (Number.isFinite(ratio) && ratio > 0) {
                setAspect(ratio)
                onAspect?.(ratio)
              }
            }}
            onError={() => setThumb(null)}
          />
        ) : thumb === null ? (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 text-ink-tertiary">
            <ImageOff className="h-5 w-5" strokeWidth={1.75} />
            <span className="text-caption">Preview unavailable</span>
          </div>
        ) : (
          <Skeleton className="absolute inset-0 rounded-none" />
        )}
        {hovering && !previewFailed && (
          <video
            src={localFileUrl(filePath)}
            autoPlay
            muted
            loop
            playsInline
            className="absolute inset-0 h-full w-full object-cover"
            onError={() => setPreviewFailed(true)}
          />
        )}

        {/* The media plays the clip, or picks it while clips are being selected (the checkbox is the labelled control then). */}
        <button
          type="button"
          onClick={selecting ? onToggleSelect : () => { void openClip() }}
          aria-label={`Play “${title}”`}
          aria-hidden={selecting || undefined}
          tabIndex={selecting ? -1 : undefined}
          className="absolute inset-0 flex items-center justify-center focus-visible:[outline-offset:-3px]"
        >
          {!selecting && (
            <span className="media-chip flex h-10 w-10 items-center justify-center rounded-full opacity-0 transition-opacity duration-150 ease-out group-focus-within:opacity-100 group-hover:opacity-100">
              <Play className="ml-0.5 h-4 w-4" fill="currentColor" />
            </span>
          )}
        </button>

        <div
          className={cn(
            'absolute left-2 top-2 z-10 transition-opacity duration-150',
            selecting || selected ? 'opacity-100' : 'opacity-0 group-focus-within:opacity-100 group-hover:opacity-100'
          )}
        >
          <Checkbox variant="overlay" checked={selected} onChange={onToggleSelect} label={`Select ${title}`} />
        </div>
        <div className="pointer-events-none absolute right-2 top-2 z-10 flex items-center gap-1">
          {topPick && (
            <span className="inline-flex h-[1.125rem] items-center rounded bg-accent px-1.5 text-meta font-semibold text-white">
              Top pick
            </span>
          )}
          {showScore && (
            <span
              className="media-chip pointer-events-auto inline-flex h-[1.125rem] items-center gap-1 rounded px-1.5 text-meta font-medium tabular-nums"
              title="Virality score"
            >
              <TrendingUp className="h-3 w-3 text-brand-gold" />
              {score}
            </span>
          )}
        </div>
        <span className="media-chip pointer-events-none absolute bottom-2 left-2 z-10 rounded px-1 py-px text-meta font-medium tabular-nums">
          {formatTimecode(clip.duration_ms)}
        </span>
        {!selecting && (
          <div className="absolute bottom-2 right-2 z-10 flex items-center gap-1 opacity-0 transition-opacity duration-150 focus-within:opacity-100 group-hover:opacity-100">
            <MediaAction label={isMac ? 'Show in Finder' : 'Show in folder'} icon={<FolderOpen />} onClick={() => { void showInFolder() }} />
          </div>
        )}
      </div>

      <div className="px-1.5 pb-1 pt-2">
        <h3 className="line-clamp-2 text-label text-ink" title={title}>
          {title}
        </h3>
        <p className="mt-0.5 text-meta text-ink-secondary">
          <span className="tabular-nums" title="Position in the source video">
            {formatTimecode(clip.start_time_ms)} – {formatTimecode(clip.end_time_ms)}
          </span>
          {layout && <span title="How this clip was framed"> · {layout}</span>}
        </p>
        {clip.render_fallback && (
          <Badge
            tone="warning"
            icon={<TriangleAlert className="h-3 w-3" />}
            className="mt-1.5 self-start"
          >
            <span title="Smart framing failed to render this clip, so it used the classic whole-frame layout. The log has the details.">
              {clip.render_fallback === 'letterbox_natural' ? 'Fallback: whole frame, no cuts' : 'Fallback: whole frame'}
            </span>
          </Badge>
        )}
        {actionError && <p role="alert" className="mt-1.5 text-caption text-danger">{actionError}</p>}
      </div>
    </article>
  )
}

/** Small dark button over the clip's media; shown on hover or focus. */
function MediaAction({ label, title, icon, onClick }: { label: string; title?: string; icon: React.ReactNode; onClick: () => void }): React.JSX.Element {
  return (
    <button
      type="button"
      aria-label={label}
      title={title ?? label}
      onClick={onClick}
      className="media-chip flex h-6 w-6 items-center justify-center rounded-md transition-colors duration-feedback hover:bg-black/70 hover:text-white [&_svg]:h-3.5 [&_svg]:w-3.5"
    >
      {icon}
    </button>
  )
}
