import { useEffect, useState } from 'react'
import { Check, Download, Trash2, X } from 'lucide-react'
import { WHISPER_MODELS } from '../../shared/whisper-models'
import { useSettingsStore } from '../store/use-settings-store'
import { getApi } from '../lib/ipc'
import { cn, errorMessage } from '../lib/utils'
import { onRadioKeyDown } from './ui/Segmented'
import { Button } from './ui/Button'
import { ProgressBar } from './ui/ProgressBar'

/**
 * Offline transcription models. The selected one must be downloaded before a
 * job can start; downloads run in the main process and survive navigation.
 */
export function WhisperModels({ compact = false }: { compact?: boolean }): React.JSX.Element {
  const { whisperModels, whisperModel, save, setWhisperStatus, loadWhisperModels } = useSettingsStore()
  const [error, setError] = useState<string | null>(null)

  useEffect(() => getApi().whisper.onProgress(setWhisperStatus), [setWhisperStatus])

  const download = (id: string): void => {
    setError(null)
    getApi().whisper.download(id).then(() => loadWhisperModels())
      .catch((err) => setError(errorMessage(err, 'Download failed')))
  }
  const select = (id: string): void => {
    save({ whisperModel: id }).catch((err) => setError(errorMessage(err, 'Could not save')))
    if (!whisperModels.find((model) => model.id === id)?.downloaded) download(id)
  }

  const models = compact ? WHISPER_MODELS.filter((model) => model.id === whisperModel) : WHISPER_MODELS
  return (
    <div className="space-y-2">
      <div className="space-y-1.5" role="radiogroup" aria-label="Whisper model">
        {models.map((model) => {
          const status = whisperModels.find((item) => item.id === model.id)
          const selected = whisperModel === model.id
          const downloading = status?.receivedBytes != null
          const percent = downloading && status?.totalBytes ? (status.receivedBytes! / status.totalBytes) * 100 : 0
          return (
            <div key={model.id} className={cn('tile rounded-lg px-3 py-2.5', selected && 'tile-selected')}>
              <div className="flex items-center gap-3">
                <button type="button" role="radio" aria-checked={selected} tabIndex={selected ? 0 : -1}
                  onKeyDown={onRadioKeyDown} onClick={() => select(model.id)} className="flex min-w-0 flex-1 items-center gap-2.5 text-left">
                  <span className={cn('flex h-4 w-4 shrink-0 items-center justify-center rounded-full border',
                    selected ? 'border-accent bg-accent' : 'border-content/30')}>
                    {selected && <Check className="h-2.5 w-2.5 text-accent-ink" strokeWidth={4} />}
                  </span>
                  <span className="min-w-0">
                    <span className="block text-label text-ink">
                      {model.label} <span className="text-meta font-normal tabular-nums text-ink-secondary">{formatSize(model.sizeMb)}</span>
                    </span>
                    <span className="block truncate text-caption text-ink-secondary">{model.note}</span>
                  </span>
                </button>
                {downloading ? (
                  <Button size="sm" variant="ghost" icon={<X className="h-3.5 w-3.5" />} onClick={() => void getApi().whisper.cancel(model.id)}>Cancel</Button>
                ) : status?.downloaded ? (
                  <span className="flex items-center gap-1">
                    <span className="text-meta text-success">Downloaded</span>
                    {!compact && (
                      <Button size="sm" variant="ghost" iconOnly aria-label={`Delete ${model.label}`} icon={<Trash2 className="h-3.5 w-3.5" />}
                        onClick={() => getApi().whisper.remove(model.id).then(() => loadWhisperModels())} />
                    )}
                  </span>
                ) : (
                  <Button size="sm" variant={selected ? 'primary' : 'secondary'} icon={<Download className="h-3.5 w-3.5" />} onClick={() => download(model.id)}>
                    Download
                  </Button>
                )}
              </div>
              {downloading && (
                <div className="mt-2 flex items-center gap-2">
                  <ProgressBar value={percent} />
                  <span className="w-24 shrink-0 whitespace-nowrap text-right text-meta tabular-nums text-ink-secondary">
                    {formatSize((status!.receivedBytes ?? 0) / 1e6)} / {formatSize(model.sizeMb)}
                  </span>
                </div>
              )}
            </div>
          )
        })}
      </div>
      {error && <p role="alert" className="text-caption text-danger">{error}</p>}
    </div>
  )
}

function formatSize(mb: number): string {
  return mb >= 1000 ? `${(mb / 1000).toFixed(1)} GB` : `${Math.round(mb)} MB`
}
