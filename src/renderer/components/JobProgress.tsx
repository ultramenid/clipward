import { useEffect, useState } from 'react'
import { AlertTriangle, Check, Clapperboard, Clock3, Download, FileText, Film, Gauge, Github, RotateCcw, ScrollText, Sparkles } from 'lucide-react'
import { cn, formatTimecode, sourceLabel } from '../lib/utils'
import { getApi } from '../lib/ipc'
import { ISSUES_URL } from '../config/brand'
import type { Job } from '../store/use-job-store'
import { Page } from './ui/Page'
import { ProgressRing } from './ui/ProgressBar'
import { IconTile } from './ui/IconTile'
import { Button } from './ui/Button'

export const STAGE_LABELS: Record<string, string> = {
  queued: 'Queued',
  pending: 'Starting',
  downloading: 'Downloading',
  transcribing: 'Transcribing',
  planning: 'Finding moments',
  rendering: 'Rendering clips',
  uploading: 'Saving clips',
  completed: 'Done',
  failed: 'Failed',
  cancelled: 'Cancelled'
}

const STEPS = [
  { label: 'Download', icon: Download, statuses: ['downloading'] },
  { label: 'Transcribe', icon: FileText, statuses: ['transcribing'] },
  { label: 'Find moments', icon: Sparkles, statuses: ['planning'] },
  { label: 'Render', icon: Film, statuses: ['rendering', 'uploading'] }
]

function stepIndex(status: string): number {
  if (status === 'completed') return STEPS.length
  return STEPS.findIndex((s) => s.statuses.includes(status))
}

function useElapsed(since: string, running: boolean): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!running) return
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [running])
  return Math.max(0, now - new Date(since).getTime())
}

/** Small hairline chip for a single fact under the progress ring. */
function InfoChip({ icon, children }: { icon: React.ReactNode; children: React.ReactNode }): React.JSX.Element {
  return (
    <span className="inline-flex h-6 items-center gap-1.5 rounded-md border border-content/10 px-2 text-caption tabular-nums text-ink-secondary [&_svg]:h-3.5 [&_svg]:w-3.5 [&_svg]:text-ink-tertiary">
      {icon}
      {children}
    </span>
  )
}

interface JobProgressProps {
  job: Job
  onCancel: () => void
  /** Rendered above the title, e.g. a back link to the jobs list. */
  leading?: React.ReactNode
}

export function JobProgress({ job, onCancel, leading }: JobProgressProps): React.JSX.Element {
  const current = stepIndex(job.status)
  const queued = job.status === 'queued'
  const elapsed = useElapsed(job.startedAt ?? job.queuedAt, true)
  const pct = Math.round(Math.min(job.percent, 100))
  const source = job.request.videoUrl

  return (
    <Page width="focus">
      {leading && <div className="mb-2">{leading}</div>}
      <div className="text-center">
        <p className="eyebrow text-accent-fg">{queued ? 'Waiting in the queue' : 'Generating clips'}</p>
        <h1 className="mx-auto mt-1 max-w-[560px] truncate text-title text-ink" title={source}>
          {sourceLabel(source)}
        </h1>
      </div>

      <section className="card relative mt-5 overflow-hidden rounded-xl px-5 pb-5 pt-6">
        <div className="relative flex flex-col items-center">
          <ProgressRing value={job.percent} size={160} stroke={6}>
            <p className="text-display tabular-nums text-ink">
              {pct}
              <span className="ml-0.5 text-heading font-medium text-ink-tertiary">%</span>
            </p>
            <p className="mt-1.5 text-caption font-medium text-accent-fg">{STAGE_LABELS[job.status] ?? 'Working'}</p>
          </ProgressRing>

          <p className="mt-4 h-5 max-w-full truncate text-center text-body text-ink-secondary" aria-live="polite">
            {job.step}
          </p>

          <div className="mt-3 flex flex-wrap items-center justify-center gap-1.5">
            {(job.request.videoSpeed ?? 1) > 1 && (
              <InfoChip icon={<Gauge />}>{job.request.videoSpeed}× export speed</InfoChip>
            )}
            {job.clipsTotal > 0 && (
              <InfoChip icon={<Clapperboard />}>
                {job.clipsDone} of {job.clipsTotal} clip{job.clipsTotal === 1 ? '' : 's'} rendered
              </InfoChip>
            )}
            <InfoChip icon={<Clock3 />}>{formatTimecode(elapsed)} {queued ? 'waiting' : 'elapsed'}</InfoChip>
          </div>
        </div>

        <ol className="relative mt-6 flex items-start" aria-label="Progress">
          {STEPS.map((step, i) => {
            const done = current > i
            const active = current === i
            const Icon = step.icon
            return (
              <li key={step.label} className="flex flex-1 items-start last:flex-none">
                <div className="flex w-[84px] flex-col items-center gap-2">
                  <span
                    className={cn(
                      'flex h-8 w-8 items-center justify-center rounded-full border transition-colors duration-300',
                      done && 'border-content/70 bg-content/70 text-canvas',
                      active && 'animate-pulse-ring border-accent/70 bg-accent/15 text-accent-fg',
                      !done && !active && 'border-content/15 text-ink-tertiary'
                    )}
                    aria-current={active ? 'step' : undefined}
                  >
                    {done ? <Check className="h-3.5 w-3.5" strokeWidth={3} /> : <Icon className="h-3.5 w-3.5" strokeWidth={1.75} />}
                  </span>
                  <span
                    className={cn(
                      'text-center text-meta font-medium',
                      active ? 'text-content' : done ? 'text-ink-secondary' : 'text-ink-tertiary'
                    )}
                  >
                    {step.label}
                  </span>
                </div>
                {i < STEPS.length - 1 && (
                  <div className="mt-[15px] h-0.5 flex-1 overflow-hidden rounded-full bg-content/10">
                    <div
                      className="h-full rounded-full bg-accent transition-[width] duration-700 ease-out"
                      style={{ width: done ? '100%' : '0%' }}
                    />
                  </div>
                )}
              </li>
            )
          })}
        </ol>
      </section>

      <div className="mt-4 flex items-center justify-between gap-4 px-1">
        <p className="max-w-[60ch] text-caption text-ink-secondary">
          {queued
            ? 'Starts automatically when a running job finishes.'
            : 'Long videos can take a while. Keep using Clipward or queue more videos; progress shows in the sidebar.'}
        </p>
        <Button onClick={onCancel}>{queued ? 'Remove from queue' : 'Cancel'}</Button>
      </div>
    </Page>
  )
}

export function JobFailure({ job, onRetry, leading }: { job: Job; onRetry: () => void; leading?: React.ReactNode }): React.JSX.Element {
  return (
    <Page width="focus">
      {leading && <div className="mb-3">{leading}</div>}
      <IconTile tone="danger" size="md">
        <AlertTriangle />
      </IconTile>
      <h1 className="mt-3 text-title text-ink">Clipping failed</h1>
      <p className="mt-1 truncate text-body text-ink-secondary" title={job.request.videoUrl}>
        {sourceLabel(job.request.videoUrl)}
      </p>

      <section className="card mt-5 overflow-hidden rounded-xl">
        <pre
          className="max-h-72 overflow-auto whitespace-pre-wrap break-words border-b border-stroke bg-danger/[0.04] px-4 py-3 font-mono text-caption text-danger"
          data-selectable
        >
          {job.error || 'The clipping engine stopped without an error message.'}
        </pre>
        {job.errorHint && (
          <p className="border-b border-stroke px-4 py-3 text-body text-content/70 last:border-b-0" data-selectable>
            {job.errorHint}
          </p>
        )}
        {job.failureCode && (
          <p className="px-4 py-2.5 font-mono text-meta text-ink-tertiary" data-selectable>
            Failure code: {job.failureCode}{job.httpStatus ? ` · HTTP ${job.httpStatus}` : ''}
          </p>
        )}
      </section>

      <div className="mt-4 flex flex-wrap items-center gap-1.5">
        <Button variant="primary" icon={<RotateCcw className="h-3.5 w-3.5" />} onClick={onRetry}>
          Run again
        </Button>
        <Button icon={<ScrollText className="h-3.5 w-3.5" />} onClick={() => getApi().diagnostics.openLogFolder()}>
          Show logs
        </Button>
        <Button variant="ghost" icon={<Github className="h-3.5 w-3.5" />} onClick={() => getApi().shell.openPath(ISSUES_URL)}>
          Report an issue
        </Button>
      </div>
    </Page>
  )
}
