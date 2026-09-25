import { useCallback, useEffect, useRef, useState } from 'react'
import { AlertTriangle, ArrowDownToLine, RefreshCw, Sparkles, X } from 'lucide-react'
import { errorMessage } from '../lib/utils'
import { getApi } from '../lib/ipc'
import { APP_NAME } from '../config/brand'
import { Button } from './ui/Button'
import { ProgressBar } from './ui/ProgressBar'
import { Dialog, DialogFooter } from './ui/Dialog'
import { IconTile } from './ui/IconTile'

type UpdateState =
  | { phase: 'idle' }
  | { phase: 'available'; version: string; releaseNotes?: string; releaseDate?: string }
  | { phase: 'downloading'; percent: number; bytesPerSecond: number; transferred: number; total: number }
  | { phase: 'ready' }
  | { phase: 'error'; message: string }

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

export function UpdateModal(): React.JSX.Element | null {
  const [state, setState] = useState<UpdateState>({ phase: 'idle' })
  const [dismissed, setDismissed] = useState(false)
  const dialogRef = useRef<HTMLDivElement>(null)
  const phaseRef = useRef(state.phase)
  phaseRef.current = state.phase
  // The background check on launch fails quietly (offline, no release yet);
  // only surface errors for a download the user asked for.
  const userInitiated = useRef(false)

  useEffect(() => {
    const api = getApi()
    const unsubs = [
      api.update.onAvailable((info) => {
        setState({ phase: 'available', ...info })
        setDismissed(false)
      }),
      api.update.onProgress((info) => setState({ phase: 'downloading', ...info })),
      api.update.onDownloaded(() => {
        setState({ phase: 'ready' })
        setDismissed(false)
      }),
      api.update.onError((info) => {
        if (userInitiated.current) setState({ phase: 'error', message: info.message })
      })
    ]
    return () => unsubs.forEach((unsub) => unsub())
  }, [])

  const handleDownload = useCallback(() => {
    userInitiated.current = true
    setState({ phase: 'downloading', percent: 0, bytesPerSecond: 0, transferred: 0, total: 0 })
    getApi().update.download().catch((err) => setState({ phase: 'error', message: errorMessage(err, 'Could not download the update.') }))
  }, [])

  const handleInstall = useCallback(() => {
    getApi().update.install().catch((err) => setState({ phase: 'error', message: errorMessage(err, 'Could not install the update.') }))
  }, [])

  const open = state.phase !== 'idle' && !dismissed
  useEffect(() => {
    if (!open) return
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null
    const dialog = dialogRef.current
    ;(dialog?.querySelector<HTMLElement>('button:not([disabled])') ?? dialog)?.focus()

    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape' && phaseRef.current !== 'downloading') {
        event.preventDefault()
        setDismissed(true)
      }
      if (event.key !== 'Tab' || !dialog) return
      const buttons = [...dialog.querySelectorAll<HTMLElement>('button:not([disabled])')]
      if (buttons.length === 0) {
        event.preventDefault()
        dialog.focus()
        return
      }
      const first = buttons[0]
      const last = buttons[buttons.length - 1]
      if (event.shiftKey && (document.activeElement === first || !dialog.contains(document.activeElement))) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && (document.activeElement === last || !dialog.contains(document.activeElement))) {
        event.preventDefault()
        first.focus()
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.removeEventListener('keydown', onKeyDown)
      previousFocus?.focus()
    }
  }, [open])

  if (state.phase === 'idle' || dismissed) return null

  const title = {
    available: 'Update available',
    downloading: 'Downloading update',
    ready: 'Ready to install',
    error: 'Update failed'
  }[state.phase]

  const failed = state.phase === 'error'

  return (
    <Dialog ref={dialogRef} layer="system" aria-label={title} panelClassName="max-w-[440px]">
      <div className="flex items-start justify-between gap-2 px-4 pt-3">
        <div className="flex items-center gap-3 pt-0.5">
          <IconTile tone={failed ? 'danger' : 'accent'} size="md">
            {failed ? <AlertTriangle /> : state.phase === 'ready' ? <RefreshCw /> : <ArrowDownToLine />}
          </IconTile>
          <div>
            <p className="eyebrow">Software update</p>
            <h3 className="mt-0.5 text-title text-ink">{title}</h3>
          </div>
        </div>
        {state.phase !== 'downloading' && (
          <Button
            variant="ghost"
            size="sm"
            iconOnly
            aria-label="Close"
            onClick={() => setDismissed(true)}
            className="h-7 w-7 text-ink-secondary"
            icon={<X className="h-3.5 w-3.5" strokeWidth={1.75} />}
          />
        )}
      </div>

      <div className="px-4 py-4 text-body text-ink-secondary">
        {state.phase === 'available' && (
          <div className="space-y-3">
            <p>
              {APP_NAME} <span className="font-medium text-ink">v{state.version}</span> is ready to download.
            </p>
            {state.releaseNotes && (
              <div className="field max-h-40 overflow-y-auto rounded-lg px-3 py-2.5">
                <p className="eyebrow mb-2 flex items-center gap-1.5">
                  <Sparkles className="h-3 w-3 text-accent-fg" />
                  What’s new
                </p>
                <p className="whitespace-pre-wrap text-caption text-ink-secondary" data-selectable>
                  {state.releaseNotes}
                </p>
              </div>
            )}
          </div>
        )}

        {state.phase === 'downloading' && (
          <div className="space-y-3 pb-1">
            <div className="flex items-baseline justify-between">
              <span className="text-title tabular-nums text-ink">
                {Math.round(state.percent)}
                <span className="text-body font-medium text-ink-tertiary">%</span>
              </span>
              <span className="text-meta tabular-nums text-ink-secondary">
                {formatBytes(state.transferred)} / {formatBytes(state.total)}
                {state.bytesPerSecond > 0 && ` · ${formatBytes(state.bytesPerSecond)}/s`}
              </span>
            </div>
            <ProgressBar value={state.percent} className="h-1" />
          </div>
        )}

        {state.phase === 'ready' && (
          <p>Restart to finish updating, or it will install the next time you quit.</p>
        )}

        {state.phase === 'error' && (
          <p className="text-danger" data-selectable>
            {state.message || 'Something went wrong while downloading the update.'}
          </p>
        )}
      </div>

      {state.phase !== 'downloading' && (
        <DialogFooter>
          {state.phase === 'available' && (
            <>
              <Button variant="ghost" onClick={() => setDismissed(true)}>
                Later
              </Button>
              <Button variant="primary" icon={<ArrowDownToLine className="h-3.5 w-3.5" />} onClick={handleDownload}>
                Download
              </Button>
            </>
          )}
          {state.phase === 'ready' && (
            <>
              <Button variant="ghost" onClick={() => setDismissed(true)}>
                Later
              </Button>
              <Button variant="primary" icon={<RefreshCw className="h-3.5 w-3.5" />} onClick={handleInstall}>
                Restart now
              </Button>
            </>
          )}
          {state.phase === 'error' && <Button onClick={() => setDismissed(true)}>Dismiss</Button>}
        </DialogFooter>
      )}
    </Dialog>
  )
}
