import { normalizeVideoSource, twitchSourceError, twitchVodId } from '../../shared/video-source'
import { useCallback, useRef, useState } from 'react'
import { FileVideo, FolderOpen, Link2, UploadCloud, X, Youtube, Twitch } from 'lucide-react'
import { basename, cn, formatTimecode, isUrl, localFileUrl, youtubeId } from '../lib/utils'
import { getApi } from '../lib/ipc'
import { Button } from './ui/Button'
import { TextInput } from './ui/Field'
import { Badge } from './ui/Badge'
import { IconTile } from './ui/IconTile'

interface SourcePickerProps {
  value: string
  onChange: (source: string) => void
  disabled?: boolean
}

/** Match the main process's HTTP(S) URL requirements before accepting a link. */
export function isValidSourceLink(value: string): boolean {
  try {
    const url = new URL(value.trim())
    return (url.protocol === 'https:' || url.protocol === 'http:') && Boolean(url.hostname) && !url.username && !url.password && !twitchSourceError(value)
  } catch {
    return false
  }
}

function displaySourceLink(value: string): string {
  try {
    const url = new URL(value)
    return `${url.hostname}${url.pathname === '/' ? '' : url.pathname}`
  } catch {
    return 'Video link'
  }
}

export function SourcePicker({ value, onChange, disabled }: SourcePickerProps): React.JSX.Element {
  const [pickerError, setPickerError] = useState<string | null>(null)
  const browse = useCallback(async () => {
    try {
      const file = await getApi().dialog.selectVideo()
      if (file) {
        setPickerError(null)
        onChange(file)
      }
    } catch {
      setPickerError('Could not open the file picker. Please try again.')
    }
  }, [onChange])

  if (value) {
    return <>
      <SourcePreview key={value} source={value} onClear={() => { setPickerError(null); onChange('') }} onReplace={browse} disabled={disabled} />
      {pickerError && <p role="alert" className="mt-2.5 px-1 text-xs text-danger">{pickerError}</p>}
    </>
  }
  return <>
    <DropZone onChange={onChange} onBrowse={browse} disabled={disabled} />
    {pickerError && <p role="alert" className="mt-2.5 px-1 text-xs text-danger">{pickerError}</p>}
  </>
}

function DropZone({
  onChange,
  onBrowse,
  disabled
}: {
  onChange: (source: string) => void
  onBrowse: () => void
  disabled?: boolean
}): React.JSX.Element {
  const [dragging, setDragging] = useState(false)
  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  // dragenter/dragleave fire for every child; count them to know when the
  // pointer has really left the zone.
  const depth = useRef(0)

  const submitLink = (text: string): void => {
    const link = text.trim()
    if (!link) return
    if (!isValidSourceLink(link)) {
      setError(twitchSourceError(link) ?? 'Paste a valid HTTP(S) link without a username or password.')
      return
    }
    setError(null)
    onChange(normalizeVideoSource(link))
  }

  const onDrop = async (e: React.DragEvent): Promise<void> => {
    e.preventDefault()
    depth.current = 0
    setDragging(false)
    if (disabled) return

    if (e.dataTransfer.files?.length) {
      try {
        // A renderer-supplied path cannot grant file access. Let the native
        // picker make the local selection in the app shell.
        const path = await getApi().dialog.selectVideo()
        if (!path) return
        setError(null)
        onChange(path)
      } catch {
        setError('Could not open that video. Try another file or use Browse.')
      }
      return
    }
    const text = e.dataTransfer.getData('text/uri-list') || e.dataTransfer.getData('text/plain')
    const firstLink = text.split(/\r?\n/).find((line) => line.trim() && !line.trim().startsWith('#'))
    if (firstLink) submitLink(firstLink)
  }

  // A compact drop target: a headline row, then the link field and Browse. The
  // whole card accepts a dropped file.
  return (
    <div
      onDragEnter={(e) => {
        e.preventDefault()
        depth.current += 1
        if (!disabled) setDragging(true)
      }}
      onDragOver={(e) => e.preventDefault()}
      onDragLeave={() => {
        depth.current = Math.max(0, depth.current - 1)
        if (depth.current === 0) setDragging(false)
      }}
      onDrop={onDrop}
      className={cn(
        'relative rounded-xl border border-dashed border-content/15 bg-content/[0.02] p-4 transition-[border-color,background-color] duration-feedback ease-out',
        dragging && 'border-accent/70 bg-accent/[0.06]',
        disabled && 'pointer-events-none opacity-50'
      )}
    >
      <div className="relative flex items-center gap-3">
        <IconTile tone="accent">
          <UploadCloud strokeWidth={1.75} />
        </IconTile>
        <div className="min-w-0">
          <h2 className="truncate text-[13px] font-semibold text-ink">
            {dragging ? 'Choose this video in the file picker' : 'Add a long video'}
          </h2>
          <p className="text-xs text-content/45">Drop a video file or link here, or paste a link below.</p>
          <div role="group" aria-label="Supported video links" className="mt-2 flex flex-wrap items-center gap-1.5">
            <Badge icon={<Youtube aria-hidden className="h-3 w-3 text-[#ff0033]" />}>YouTube</Badge>
            <Badge icon={<Twitch aria-hidden className="h-3 w-3 text-[#a970ff]" />}>Twitch VODs</Badge>
          </div>
        </div>
      </div>

      <div className="relative mt-3 flex items-center gap-2">
        <TextInput
          inputSize="lg"
          className="min-w-0 flex-1"
          value={draft}
          placeholder="YouTube, Twitch VOD or direct video link"
          aria-label="Video link"
          leading={<Link2 />}
          onChange={(e) => {
            setDraft(e.target.value)
            if (error) setError(null)
          }}
          aria-invalid={Boolean(error)}
          aria-describedby={error ? 'source-link-error' : undefined}
          disabled={disabled}
          onPaste={(e) => {
            const text = e.clipboardData.getData('text')
            if (isValidSourceLink(text) || twitchSourceError(text)) {
              e.preventDefault()
              submitLink(text)
            }
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter') submitLink(draft)
          }}
          trailing={
            draft.trim() ? (
              <Button size="sm" variant="primary" className="mr-0.5 animate-fade-in" onClick={() => submitLink(draft)} disabled={disabled}>
                Use link
              </Button>
            ) : undefined
          }
        />
        <span className="text-xs text-content/35 max-sm:hidden">or</span>
        <Button size="lg" variant="secondary" icon={<FolderOpen className="h-3.5 w-3.5" />} onClick={onBrowse} disabled={disabled}>
          Browse
        </Button>
      </div>
      {error && <p id="source-link-error" role="alert" className="relative mt-2 px-1 text-xs text-danger">{error}</p>}
    </div>
  )
}

function SourcePreview({
  source,
  onClear,
  onReplace,
  disabled
}: {
  source: string
  onClear: () => void
  onReplace: () => void
  disabled?: boolean
}): React.JSX.Element {
  const link = isUrl(source)
  const ytId = link ? youtubeId(source) : null
  const twitchId = link ? twitchVodId(source) : null
  const displaySource = link ? displaySourceLink(source) : basename(source)
  const [durationMs, setDurationMs] = useState<number | null>(null)
  const [mediaFailed, setMediaFailed] = useState(false)

  return (
    <div className="card flex items-center gap-3 rounded-xl p-2 pr-3 animate-fade-in">
      <div className="relative aspect-video h-[72px] shrink-0 overflow-hidden rounded-lg border border-stroke bg-black/40">
        {!mediaFailed && ytId && (
          <img
            src={`https://i.ytimg.com/vi/${ytId}/hqdefault.jpg`}
            alt=""
            className="h-full w-full object-cover"
            onError={() => setMediaFailed(true)}
            draggable={false}
          />
        )}
        {!mediaFailed && !link && (
          <video
            src={localFileUrl(source)}
            preload="metadata"
            muted
            className="h-full w-full object-cover"
            onLoadedMetadata={(e) => {
              const seconds = e.currentTarget.duration
              if (Number.isFinite(seconds)) {
                setDurationMs(seconds * 1000)
                // Seek past a black first frame for a representative still. Not a
                // #t= fragment: the local-file protocol would treat it as part of the path.
                e.currentTarget.currentTime = Math.min(2, seconds / 2)
              }
            }}
            onError={() => setMediaFailed(true)}
          />
        )}
        {(mediaFailed || (link && !ytId)) && (
          <div className="flex h-full w-full items-center justify-center bg-content/5 text-content/45">
            {twitchId ? <Twitch className="h-5 w-5" /> : link ? <Link2 className="h-5 w-5" /> : <FileVideo className="h-5 w-5" />}
          </div>
        )}
        {durationMs != null && (
          <span className="media-chip absolute bottom-1 right-1 rounded px-1 font-mono text-[10px] tabular">
            {formatTimecode(durationMs)}
          </span>
        )}
      </div>

      <div className="min-w-0 flex-1">
        <p className="truncate text-[13px] font-medium text-ink" title={displaySource} data-selectable>
          {displaySource}
        </p>
        <div className="mt-1.5 flex min-w-0 items-center gap-2">
          <Badge
            icon={twitchId ? <Twitch className="h-3 w-3" /> : ytId ? <Youtube className="h-3 w-3" /> : link ? <Link2 className="h-3 w-3" /> : <FileVideo className="h-3 w-3" />}
          >
            {twitchId ? 'Twitch VOD' : ytId ? 'YouTube' : link ? 'Link' : 'Local file'}
          </Badge>
          <span className="truncate text-xs text-content/45">{twitchId ? 'Public, completed videos only' : 'Ready to clip'}</span>
        </div>
      </div>

      <div className="flex shrink-0 items-center gap-1.5">
        <Button variant="secondary" size="sm" onClick={onReplace} disabled={disabled}>
          Replace
        </Button>
        <Button
          variant="ghost"
          size="sm"
          iconOnly
          aria-label="Remove video"
          onClick={onClear}
          disabled={disabled}
          icon={<X className="h-3.5 w-3.5" />}
        />
      </div>
    </div>
  )
}
