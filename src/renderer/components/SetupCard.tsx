import { Check, Sparkles, Wrench } from 'lucide-react'
import { useSettingsStore, useSetupState } from '../store/use-settings-store'
import { cn } from '../lib/utils'
import { useDraftStore } from '../store/use-draft-store'
import { ProviderSetup } from './ProviderSetup'
import { WhisperModels } from './WhisperModels'
import { Button } from './ui/Button'
import { IconTile } from './ui/IconTile'

/**
 * First-run setup, inline on the Create page: pick an AI provider and download
 * the offline Whisper model here instead of being sent to Settings. Hidden once
 * both are ready and the tools check out. Manual clip mode needs no provider, so
 * there the card only asks for the Whisper model.
 */
export function SetupCard({ onOpenSettings, className }: { onOpenSettings: () => void; className?: string }): React.JSX.Element | null {
  const { providerReady, whisperReady, toolsOk } = useSetupState()
  const manual = useDraftStore((s) => s.clipMode === 'manual')
  const toolStatus = useSettingsStore((s) => s.toolStatus)
  const toolError = useSettingsStore((s) => s.toolError)
  const missingTools = toolStatus
    ? [
        !toolStatus.smartFraming && 'Smart framing model',
        !toolStatus.ffmpeg && 'FFmpeg',
        !toolStatus.ffprobe && 'FFprobe',
        !toolStatus.ytdlp && 'yt-dlp',
        !toolStatus.engine && 'Clipward clipping engine'
      ].filter(Boolean).join(', ')
    : ''

  // Manual clip mode never calls an AI provider, so only Whisper is asked for there.
  const needsProvider = !manual && !providerReady
  if (needsProvider || !whisperReady) {
    return (
      <section
        className={cn(
          'relative overflow-hidden rounded-xl border border-accent/25 bg-accent/[0.04] animate-fade-in',
          className
        )}
        aria-label="Finish setup"
      >
        <div className="flex items-start gap-3 px-4 pb-1 pt-4">
          <IconTile tone="accent"><Sparkles /></IconTile>
          <div className="min-w-0">
            <p className="eyebrow text-accent-fg">One-time setup</p>
            <h2 className="mt-0.5 text-heading text-ink">{manual ? 'One thing before your first clip' : 'Two things before your first clip'}</h2>
            <p className="mt-0.5 max-w-2xl text-caption text-ink-secondary">
              {manual
                ? 'You’re choosing the moments yourself, so no AI provider is needed. Download the speech model that transcribes offline for captions and pacing.'
                : 'Clipward has no account and no server. Pick the AI that chooses your clips, and download the speech model that transcribes offline.'}
            </p>
          </div>
        </div>
        {!manual && (
          <SetupStep n={1} done={providerReady} title="AI that picks the clips">
            <ProviderSetup compact />
            <p className="mt-2 text-caption text-ink-secondary">Picking the moments yourself? Choose “I’ll choose the moments” in the Clips step and skip this.</p>
          </SetupStep>
        )}
        <SetupStep n={manual ? 1 : 2} done={whisperReady} title="Offline transcription (Whisper)">
          <WhisperModels compact />
          <p className="mt-2 text-caption text-ink-secondary">Downloaded once. Your audio never leaves this computer. Other sizes are in Settings.</p>
        </SetupStep>
      </section>
    )
  }

  if (toolsOk === false) {
    return (
      <section
        className={cn(
          'flex items-center gap-3 rounded-xl border border-danger/25 bg-danger/[0.06] p-3 pr-3.5 animate-fade-in',
          className
        )}
      >
        <IconTile tone="danger">
          <Wrench />
        </IconTile>
        <div className="min-w-0 flex-1">
          <p className="text-label text-ink">{toolError ? 'Could not check required tools' : 'Some required tools are missing'}</p>
          <p className="mt-0.5 text-caption text-ink-secondary">{toolError ? 'Open Settings to retry the system check.' : `Missing: ${missingTools}. Open Settings for details.`}</p>
        </div>
        <Button onClick={onOpenSettings}>See what’s missing</Button>
      </section>
    )
  }

  return null
}

function SetupStep({ n, done, title, children }: { n: number; done: boolean; title: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="app-bg m-3 rounded-lg border border-stroke p-3">
      <div className={cn('flex items-center gap-2', !done && 'mb-3')}>
        <span className={cn('flex h-4 w-4 items-center justify-center rounded-full text-micro tabular-nums',
          done ? 'bg-success text-canvas' : 'bg-accent text-accent-ink')}>
          {done ? <Check className="h-2.5 w-2.5" strokeWidth={3} /> : n}
        </span>
        <span className="text-label text-ink">{title}</span>
        {done && <span className="text-caption text-success">Ready</span>}
      </div>
      {!done && children}
    </div>
  )
}
