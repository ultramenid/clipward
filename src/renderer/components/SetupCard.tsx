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
        !toolStatus.python && 'Python',
        !toolStatus.pythonDeps && 'Clipping dependencies or smart framing',
        !toolStatus.ffmpeg && 'FFmpeg',
        !toolStatus.ffprobe && 'FFprobe',
        !toolStatus.ytdlp && 'yt-dlp',
        !toolStatus.engine && 'Clipward clipping engine',
        !toolStatus.bridgeRunner && 'Bridge runner'
      ].filter(Boolean).join(', ')
    : ''

  if (!providerReady || !whisperReady) {
    return (
      <section
        className={cn(
          'glass relative overflow-hidden rounded-3xl bg-accent/[0.05] animate-fade-in',
          'shadow-[inset_0_1px_0_rgb(255_255_255/0.08),inset_0_0_0_1px_rgb(var(--accent)/0.22)]',
          className
        )}
        aria-label="Finish setup"
      >
        <div className="flex items-start gap-3 p-3.5 pb-0">
          <IconTile tone="accent"><Sparkles /></IconTile>
          <div className="min-w-0">
            <p className="eyebrow text-accent-hover">One-time setup</p>
            <h2 className="mt-0.5 text-base font-semibold text-ink">Two things before your first clip</h2>
            <p className="mt-0.5 max-w-2xl text-xs text-ink-muted">
              Clipward has no account and no server. Pick the AI that chooses your clips, and download the speech model that transcribes offline.
            </p>
          </div>
        </div>
        <SetupStep n={1} done={providerReady} title="AI that picks the clips">
          <ProviderSetup compact />
        </SetupStep>
        <SetupStep n={2} done={whisperReady} title="Offline transcription (Whisper)">
          <WhisperModels compact />
          <p className="mt-2 text-2xs text-ink-subtle">Downloaded once. Your audio never leaves this computer. Other sizes are in Settings.</p>
        </SetupStep>
      </section>
    )
  }

  if (toolsOk === false) {
    return (
      <section
        className={cn(
          'flex items-center gap-3 rounded-3xl bg-danger/[0.07] p-3 pr-4 animate-fade-in',
          'shadow-[inset_0_0_0_1px_rgb(var(--danger)/0.26),inset_0_1px_0_rgb(255_255_255/0.06)]',
          className
        )}
      >
        <IconTile tone="danger">
          <Wrench />
        </IconTile>
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium text-ink">{toolError ? 'Could not check required tools' : 'Some required tools are missing'}</p>
          <p className="mt-0.5 text-xs text-ink-muted">{toolError ? 'Open Settings to retry the system check.' : `Missing: ${missingTools}. Open Settings for details.`}</p>
        </div>
        <Button onClick={onOpenSettings}>See what’s missing</Button>
      </section>
    )
  }

  return null
}

function SetupStep({ n, done, title, children }: { n: number; done: boolean; title: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="m-3 rounded-xl bg-black/15 p-3 shadow-[inset_0_0_0_1px_rgb(255_255_255/0.06)]">
      <div className="mb-3 flex items-center gap-2">
        <span className={cn('flex h-5 w-5 items-center justify-center rounded-full font-mono text-2xs',
          done ? 'bg-success text-canvas' : 'bg-accent text-accent-ink')}>
          {done ? <Check className="h-3 w-3" strokeWidth={3} /> : n}
        </span>
        <span className="text-sm font-medium text-ink">{title}</span>
        {done && <span className="text-2xs text-success">Ready</span>}
      </div>
      {!done && children}
    </div>
  )
}
