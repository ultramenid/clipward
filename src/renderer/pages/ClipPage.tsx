import { useCallback, useRef, useState } from 'react'
import { useSetupState } from '../store/use-settings-store'
import { useDraftStore } from '../store/use-draft-store'
import { useJobStore } from '../store/use-job-store'
import { errorMessage } from '../lib/utils'
import { getApi } from '../lib/ipc'
import type { ClipJobRequest } from '../../shared/jobs'
import { JobForm } from '../components/JobForm'
import { SetupCard } from '../components/SetupCard'
import { PageHeader } from '../components/ui/PageHeader'
import { Page } from '../components/ui/Page'
import { Callout } from '../components/ui/Callout'
import type { Page as PageId } from '../components/Sidebar'

/**
 * Create: the setup card (until the OpenRouter key exists) and the clipping wizard. Runs go
 * to the main process's queue and are followed on the Jobs page, so this page
 * is always ready for the next video.
 */
export function ClipPage({ onNavigate }: { onNavigate: (page: PageId) => void }): React.JSX.Element {
  const setup = useSetupState()
  const startingRef = useRef(false)
  const [starting, setStarting] = useState(false)
  const [startError, setStartError] = useState<string | null>(null)

  const handleSubmit = useCallback(async (config: ClipJobRequest) => {
    if (startingRef.current) return
    startingRef.current = true
    setStarting(true)
    setStartError(null)
    try {
      const result = await getApi().job.start(config)
      if (result.error) setStartError(result.error)
      else if (result.jobId) useDraftStore.getState().markStarted({ jobId: result.jobId, source: config.videoUrl, queued: Boolean(result.queued) })
    } catch (err) {
      setStartError(errorMessage(err, 'Could not start the job. Please try again.'))
    } finally {
      startingRef.current = false
      setStarting(false)
    }
  }, [])

  const viewJob = useCallback((jobId: string) => {
    useJobStore.getState().focusJob(jobId)
    onNavigate('jobs')
  }, [onNavigate])

  const blockedReason =
    !setup.providerReady
      ? 'Choose an AI provider above to start.'
      : !setup.whisperReady
        ? 'Download the Whisper model above to start.'
        : setup.toolsOk === false
        ? 'Fix the missing tools in Settings to start.'
        : !setup.ready ? 'Checking required tools…' : undefined

  return (
    <Page width="narrow">
      <PageHeader
        eyebrow="Studio"
        title="Create clips"
        description="Paste a YouTube, Twitch or other video link, or upload a file. Add the campaign brief, and Clipward cuts captioned clips that follow it."
      />

      <SetupCard className="mt-4" onOpenSettings={() => onNavigate('settings')} />

      {startError && (
        <Callout tone="danger" className="mt-3" onDismiss={() => setStartError(null)}>
          <span className="whitespace-pre-line">{startError}</span>
        </Callout>
      )}

      <JobForm className="mt-4" onSubmit={handleSubmit} onViewJob={viewJob} blockedReason={blockedReason} submitting={starting} />
    </Page>
  )
}
