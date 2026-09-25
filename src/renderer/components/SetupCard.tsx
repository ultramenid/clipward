import { Check, Sparkles, Wrench } from 'lucide-react'
import { useSettingsStore, useSetupState } from '../store/use-settings-store'
import { cn } from '../lib/utils'
import { ProviderSetup } from './ProviderSetup'
import { WhisperModels } from './WhisperModels'
import { Button } from './ui/Button'
import { IconTile } from './ui/IconTile'

/**
 * First-run setup, inline on the Create page: pick an AI provider and download
 * the offline Whisper model here instead of being sent to Settings. Hidden once
 * both are ready and the tools check out.
 */
export function SetupCard({ onOpenSettings, className }: { onOpenSettings: () => void; className?: string }): React.JSX.Element | null {
  const { providerReady, whisperReady, toolsOk } = useSetupState()
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

  if (!providerReady || !whisperReady) {
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
            <h2 className="mt-0.5 text-[15px] font-medium text-ink">Two things before your first clip</h2>
            <p className="mt-0.5 max-w-2xl text-xs leading-relaxed text-content/50">
              Clipward has no account and no server. Pick the AI that chooses your clips, and download the speech model that transcribes offline.
            </p>
          </div>
        </div>
        <SetupStep n={1} done={providerReady} title="AI that picks the clips">
          <ProviderSetup compact />
        </SetupStep>
        <SetupStep n={2} done={whisperReady} title="Offline transcription (Whisper)">
          <WhisperModels compact />
          <p className="mt-2 text-xs text-content/45">Downloaded once. Your audio never leaves this computer. Other sizes are in Settings.</p>
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
          <p className="text-[13px] font-medium text-ink">{toolError ? 'Could not check required tools' : 'Some required tools are missing'}</p>
          <p className="mt-0.5 text-xs text-content/50">{toolError ? 'Open Settings to retry the system check.' : `Missing: ${missingTools}. Open Settings for details.`}</p>
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
        <span className={cn('flex h-4 w-4 items-center justify-center rounded-full font-mono text-[10px]',
          done ? 'bg-success text-canvas' : 'bg-accent text-accent-ink')}>
          {done ? <Check className="h-2.5 w-2.5" strokeWidth={3} /> : n}
        </span>
        <span className="text-[13px] font-medium text-ink">{title}</span>
        {done && <span className="text-xs text-success">Ready</span>}
      </div>
      {!done && children}
    </div>
  )
}
