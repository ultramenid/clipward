import { useEffect, useRef, useState } from 'react'
import { Search } from 'lucide-react'
import { LOCAL_PROVIDER } from '../../shared/ai-providers'
import type { LocalModelsResult } from '../../shared/ai-providers'
import { useSettingsStore } from '../store/use-settings-store'
import type { useApiKeyDrafts } from '../hooks/use-api-key-drafts'
import { getApi } from '../lib/ipc'
import { cn, errorMessage } from '../lib/utils'
import { ApiKeyInput } from './ApiKeyInput'
import { Field, TextInput } from './ui/Field'
import { Button } from './ui/Button'
import { SettingRow } from './ui/SettingRow'
import { Switch } from './ui/Switch'

/**
 * Local AI: the user's own OpenAI-compatible server (Ollama, LM Studio,
 * llama.cpp server, vLLM). Server URL, model (detected list or free text),
 * whether the model takes images, and an optional key.
 */
export function LocalAiSetup({ keys, onChange }: {
  keys: ReturnType<typeof useApiKeyDrafts>
  /** Something that affects the connection changed (clears a stale test result). */
  onChange?: () => void
}): React.JSX.Element {
  const { localBaseUrl, localModel, localImages, localConfigured, save } = useSettingsStore()
  const [urlDraft, setUrlDraft] = useState<string | null>(null)
  const [modelDraft, setModelDraft] = useState<string | null>(null)
  const [detected, setDetected] = useState<LocalModelsResult | null>(null)
  const [detecting, setDetecting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const urlRef = useRef(urlDraft)
  urlRef.current = urlDraft

  const saveUrl = async (): Promise<boolean> => {
    const draft = urlRef.current
    if (draft === null || draft.trim() === localBaseUrl) return true
    try {
      await save({ localBaseUrl: draft.trim() })
      setUrlDraft(null)
      setError(null)
      onChange?.()
      return true
    } catch (err) {
      setError(errorMessage(err, 'Invalid server URL'))
      return false
    }
  }
  const saveModel = (value: string): void => {
    const model = value.trim()
    if (model === localModel) { setModelDraft(null); return }
    save({ localModel: model }).then(() => { setModelDraft(null); setError(null); onChange?.() })
      .catch((err) => setError(errorMessage(err, 'Invalid model name')))
  }
  const detect = async (): Promise<void> => {
    setDetecting(true)
    try {
      if (!(await saveUrl())) return
      await keys.persist()
      setDetected(await getApi().ai.localModels())
    } catch (err) {
      setDetected({ ok: false, models: [], message: errorMessage(err, 'Could not reach the server') })
    } finally {
      setDetecting(false)
    }
  }

  // List the server's models once when Local AI is picked (mount only).
  const detectRef = useRef(detect)
  detectRef.current = detect
  useEffect(() => { void detectRef.current() }, [])

  const models = detected?.models ?? []
  const currentModel = modelDraft ?? localModel

  return (
    <div className="mt-3 space-y-3 animate-fade-in">
      <p className="max-w-[70ch] text-caption text-ink-secondary">
        Plans clips on your own machine with Ollama, LM Studio, llama.cpp or vLLM. For Ollama, install it, run{' '}
        <code className="font-mono text-ink">ollama pull {LOCAL_PROVIDER.exampleModel}</code> and keep it open; the URL below
        is its default (LM Studio uses <code className="font-mono text-ink">http://localhost:1234/v1</code>). Planning requests go
        only to this server, and nothing is billed.
      </p>

      <Field label="Server URL" htmlFor="local-base-url" hint="The server's OpenAI-compatible address, usually ending in /v1.">
        <TextInput id="local-base-url" mono value={urlDraft ?? localBaseUrl}
          placeholder={LOCAL_PROVIDER.defaultBaseUrl}
          onChange={(e) => setUrlDraft(e.target.value)}
          onBlur={() => void saveUrl()} />
      </Field>

      <Field label="Model" htmlFor="local-model"
        aside={
          <Button size="sm" variant="ghost" onClick={() => void detect()} loading={detecting}
            icon={<Search className="h-3.5 w-3.5" />}>Detect models</Button>
        }
        hint={detected && !detected.ok
          ? <span className="text-danger">{detected.message}</span>
          : detected?.ok && models.length === 0 ? detected.message : 'Pick a detected model or type its name.'}>
        <TextInput id="local-model" mono value={currentModel}
          placeholder={LOCAL_PROVIDER.exampleModel}
          onChange={(e) => setModelDraft(e.target.value)}
          onBlur={() => { if (modelDraft !== null) saveModel(modelDraft) }} />
        {models.length > 0 && (
          <div className="flex flex-wrap gap-1.5 pt-1" role="group" aria-label="Detected models">
            {models.map((model) => {
              const selected = model === localModel
              return (
                <button key={model} type="button" aria-pressed={selected} onClick={() => saveModel(model)}
                  className={cn('tile tile-hover max-w-full truncate rounded-md px-2 py-0.5 font-mono text-caption text-ink',
                    selected && 'tile-selected')}>
                  {model}
                </button>
              )
            })}
          </div>
        )}
      </Field>

      <SettingRow
        title="Model accepts images"
        description="Turn on for vision models (for example qwen2.5vl or gemma3) so planning can look at video frames. Leave off for text-only models."
        control={<Switch checked={localImages} label="Model accepts images"
          onChange={(checked) => {
            save({ localImages: checked }).then(() => onChange?.())
              .catch((err) => setError(errorMessage(err, 'Could not save the setting')))
          }} />}
      />

      <ApiKeyInput
        label="API key (optional)"
        value={keys.drafts[LOCAL_PROVIDER.key]}
        configured={localConfigured}
        onChange={(value) => keys.setDraft(LOCAL_PROVIDER.key, value)}
        onRemove={() => void keys.remove(LOCAL_PROVIDER.key)}
        onBlur={() => void keys.persist()}
        placeholder="Only if your server asks for one"
        description="Encrypted with your system keychain. Only sent to this server."
      />

      {error && <p role="alert" className="text-caption text-danger">{error}</p>}
    </div>
  )
}
