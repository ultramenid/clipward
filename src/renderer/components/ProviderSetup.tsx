import { useState } from 'react'
import { CheckCircle2, KeyRound, RefreshCw, TerminalSquare, XCircle, Zap } from 'lucide-react'
import { AI_CLIS, API_PROVIDERS } from '../../shared/ai-providers'
import type { ProviderTestResult } from '../../shared/ai-providers'
import { useSettingsStore } from '../store/use-settings-store'
import { useApiKeyDrafts } from '../hooks/use-api-key-drafts'
import { getApi } from '../lib/ipc'
import { cn, errorMessage } from '../lib/utils'
import { onRadioKeyDown } from './ui/Segmented'
import { ApiKeyInput } from './ApiKeyInput'
import { Field, TextInput } from './ui/Field'
import { Button } from './ui/Button'
import { Badge } from './ui/Badge'

/**
 * Pick who plans the clips: a coding CLI the user is already logged in to
 * (billed to that plan) or their own API key. Transcription never uses this;
 * it runs offline with Whisper.
 */
export function ProviderSetup({ compact = false }: { compact?: boolean }): React.JSX.Element {
  const settings = useSettingsStore()
  const { clis, aiProvider, aiModel, save, detectClis } = settings
  const keys = useApiKeyDrafts()
  const [test, setTest] = useState<ProviderTestResult | null>(null)
  const [testing, setTesting] = useState(false)
  const [detecting, setDetecting] = useState(false)
  const [modelDraft, setModelDraft] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const choose = (provider: string): void => {
    setTest(null)
    setModelDraft(null)
    // A model name only means something to the provider it was written for.
    save({ aiProvider: provider, aiModel: '' }).catch((err) => setError(errorMessage(err, 'Could not save the provider')))
  }
  const runTest = async (): Promise<void> => {
    setTesting(true)
    setTest(null)
    try {
      await keys.persist()
      setTest(await getApi().ai.test())
    } catch (err) {
      setTest({ ok: false, message: errorMessage(err, 'The test could not run') })
    } finally {
      setTesting(false)
    }
  }
  const redetect = async (): Promise<void> => {
    setDetecting(true)
    try { await detectClis() } finally { setDetecting(false) }
  }

  const api = API_PROVIDERS.find((provider) => provider.id === aiProvider)
  const installed = (clis ?? []).filter((cli) => cli.path)
  const visibleClis = compact ? installed : clis ?? []
  const configured = { openrouterApiKey: settings.openrouterConfigured, anthropicApiKey: settings.anthropicConfigured, openaiApiKey: settings.openaiConfigured }

  return (
    <div className="space-y-4">
      <div>
        <div className="mb-2 flex items-center justify-between">
          <span className="eyebrow">Use a CLI you're logged in to</span>
          <Button size="sm" variant="ghost" onClick={() => void redetect()} loading={detecting}
            icon={<RefreshCw className="h-3.5 w-3.5" />}>Detect again</Button>
        </div>
        {visibleClis.length === 0 ? (
          <p className="text-xs leading-relaxed text-content/45">
            No supported CLI found. Install and log in to one of: {AI_CLIS.map((cli) => cli.label).join(', ')}. Or use an API key below.
          </p>
        ) : (
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-3" role="radiogroup" aria-label="AI CLI">
            {visibleClis.map((cli) => {
              const id = `cli:${cli.id}`
              const selected = aiProvider === id
              const meta = AI_CLIS.find((item) => item.id === cli.id)
              return (
                <button key={cli.id} type="button" role="radio" aria-checked={selected} tabIndex={selected ? 0 : -1}
                  onKeyDown={onRadioKeyDown} disabled={!cli.path} onClick={() => choose(id)}
                  className={cn('tile tile-hover flex items-start gap-2.5 rounded-lg px-3 py-2.5 text-left disabled:opacity-45',
                    selected && 'tile-selected')}>
                  <TerminalSquare className={cn('mt-0.5 h-4 w-4 shrink-0', selected ? 'text-accent-fg' : 'text-content/40')} strokeWidth={1.75} />
                  <span className="min-w-0">
                    <span className="block text-[13px] font-medium text-ink">{cli.label}</span>
                    <span className="block truncate text-2xs text-content/40">{cli.path ? meta?.hint : 'Not installed'}</span>
                  </span>
                </button>
              )
            })}
          </div>
        )}
      </div>

      <div>
        <span className="eyebrow mb-2 block">Or bring your own API key</span>
        <div className="grid grid-cols-3 gap-2" role="radiogroup" aria-label="API provider">
          {API_PROVIDERS.map((provider) => {
            const selected = aiProvider === provider.id
            return (
              <button key={provider.id} type="button" role="radio" aria-checked={selected} tabIndex={selected ? 0 : -1}
                onKeyDown={onRadioKeyDown} onClick={() => choose(provider.id)}
                className={cn('tile tile-hover flex items-center gap-2.5 rounded-lg px-3 py-2.5 text-left', selected && 'tile-selected')}>
                <KeyRound className={cn('h-4 w-4 shrink-0', selected ? 'text-accent-fg' : 'text-content/40')} strokeWidth={1.75} />
                <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-ink">{provider.label}</span>
                {configured[provider.key] && <Badge tone="success">Key</Badge>}
              </button>
            )
          })}
        </div>
        {api && (
          <div className="mt-3 animate-fade-in">
            <ApiKeyInput
              label={`${api.label} API key`}
              value={keys.drafts[api.key]}
              configured={configured[api.key]}
              onChange={(value) => keys.setDraft(api.key, value)}
              onRemove={() => void keys.remove(api.key)}
              onBlur={() => void keys.persist()}
              placeholder={api.placeholder}
              getKeyUrl={api.url}
              description="Encrypted with your system keychain. Only sent to this provider."
            />
          </div>
        )}
      </div>

      {!compact && (
        <Field label="Model (optional)" htmlFor="ai-model"
          hint={api ? `Empty uses ${api.defaultModel}.` : 'Empty uses the CLI’s default model.'}>
          <TextInput id="ai-model" mono value={modelDraft ?? aiModel}
            placeholder={api?.defaultModel ?? 'CLI default'}
            onChange={(e) => setModelDraft(e.target.value)}
            onBlur={() => {
              if (modelDraft === null || modelDraft.trim() === aiModel) return
              save({ aiModel: modelDraft.trim() }).then(() => setModelDraft(null))
                .catch((err) => setError(errorMessage(err, 'Invalid model name')))
            }} />
        </Field>
      )}

      <div className="flex flex-wrap items-center gap-3">
        <Button onClick={() => void runTest()} loading={testing} icon={<Zap className="h-3.5 w-3.5" />}>Test connection</Button>
        {test && (
          <p role="status" className={cn('flex min-w-0 items-center gap-1.5 text-xs', test.ok ? 'text-success' : 'text-danger')}>
            {test.ok ? <CheckCircle2 className="h-3.5 w-3.5 shrink-0" /> : <XCircle className="h-3.5 w-3.5 shrink-0" />}
            <span className="truncate">{test.message}</span>
          </p>
        )}
      </div>
      {(error ?? keys.error) && <p role="alert" className="text-xs text-danger">{error ?? keys.error}</p>}
    </div>
  )
}
