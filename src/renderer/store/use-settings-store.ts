import { create } from 'zustand'
import { errorMessage } from '../lib/utils'
import { getApi } from '../lib/ipc'
import type { ClipSettings, ToolStatus } from '../../shared/api'
import type { ApiKeyName, DetectedCli } from '../../shared/ai-providers'
import type { WhisperModelStatus } from '../../shared/whisper-models'
import { APPEARANCE_DEFAULTS, normalizeAppearance, sameAppearance, type Appearance } from '../../shared/appearance'

interface SettingsState extends ClipSettings {
  loaded: boolean
  saving: boolean
  toolStatus: ToolStatus | null
  checkingTools: boolean
  toolError: string | null
  /** Supported CLIs and where each is installed; null until detected. */
  clis: DetectedCli[] | null
  whisperModels: WhisperModelStatus[]
  load: () => Promise<void>
  save: (settings: Partial<ClipSettings>) => Promise<void>
  replaceApiKey: (key: ApiKeyName, value: string) => Promise<void>
  setWindowGlass: (enabled: boolean) => Promise<void>
  /**
   * Change appearance settings. The store (and so the page) updates at once,
   * for live preview while a slider drags; saves are coalesced, so a burst of
   * changes writes the latest value once the previous write finishes.
   */
  setAppearance: (patch: Partial<Appearance>) => Promise<void>
  /** Take settings the shell changed on its own (the View menu's zoom items). */
  receive: (settings: ClipSettings) => void
  checkTools: () => Promise<void>
  detectClis: () => Promise<void>
  loadWhisperModels: () => Promise<void>
  setWhisperStatus: (status: WhisperModelStatus) => void
}

// Queue writes so each partial update merges with the last successful save.
let saveQueue: Promise<void> = Promise.resolve()
let pendingSaves = 0
let latestToolCheck = 0
// An appearance save that is queued but has not started: later changes ride on it.
let queuedAppearanceSave: Promise<void> | null = null

export const useSettingsStore = create<SettingsState>((set, get) => ({
  openrouterConfigured: false,
  anthropicConfigured: false,
  openaiConfigured: false,
  aiProvider: 'openrouter',
  aiModel: '',
  whisperModel: 'large-v3-turbo-q5_0',
  outputDirectory: '',
  customVocabulary: '',
  windowGlass: false,
  windowGlassSupported: false,
  appearance: { ...APPEARANCE_DEFAULTS },
  windowBlurSupported: false,
  nativeUiScale: false,
  loaded: false,
  saving: false,
  toolStatus: null,
  checkingTools: false,
  toolError: null,
  clis: null,
  whisperModels: [],

  load: async () => {
    const api = getApi()
    const [settings, clis, whisperModels] = await Promise.all([api.settings.load(), api.ai.detect(), api.whisper.list()])
    set({ ...pickSettings(settings), clis, whisperModels, loaded: true })
  },

  detectClis: async () => set({ clis: await getApi().ai.detect() }),
  loadWhisperModels: async () => set({ whisperModels: await getApi().whisper.list() }),
  setWhisperStatus: (status) => set({ whisperModels: get().whisperModels.map((model) => model.id === status.id ? status : model) }),

  save: (updates) => {
    const patch = { ...updates }
    pendingSaves += 1
    set({ saving: true })
    const task = saveQueue.then(async () => {
      const merged: ClipSettings = { ...pickSettings(get()), ...patch }
      const saved = await getApi().settings.save(merged)
      set({ ...pickSettings(saved), loaded: true })
    })
    saveQueue = task.catch(() => {})
    return task.finally(() => {
      pendingSaves -= 1
      set({ saving: pendingSaves > 0 })
    })
  },

  replaceApiKey: (key, value) => {
    pendingSaves += 1
    set({ saving: true })
    const task = saveQueue.then(async () => {
      const saved = await getApi().settings.replaceApiKey(key, value)
      set({ ...pickSettings(saved), loaded: true })
    })
    saveQueue = task.catch(() => {})
    return task.finally(() => {
      pendingSaves -= 1
      set({ saving: pendingSaves > 0 })
    })
  },

  setWindowGlass: (enabled) => {
    pendingSaves += 1
    set({ saving: true })
    const task = saveQueue.then(async () => {
      const saved = await getApi().settings.setWindowGlass(enabled)
      set({ ...pickSettings(saved), loaded: true })
    })
    saveQueue = task.catch(() => {})
    return task.finally(() => {
      pendingSaves -= 1
      set({ saving: pendingSaves > 0 })
    })
  },

  setAppearance: (patch) => {
    set({ appearance: normalizeAppearance({ ...get().appearance, ...patch }) })
    if (queuedAppearanceSave) return queuedAppearanceSave
    pendingSaves += 1
    set({ saving: true })
    const task = saveQueue.then(async () => {
      queuedAppearanceSave = null
      const sent = get().appearance
      const saved = pickSettings(await getApi().settings.setAppearance(sent))
      // Changes made while this write was in flight win; the next save sends them.
      const current = get().appearance
      set({ ...saved, appearance: sameAppearance(current, sent) ? saved.appearance : current, loaded: true })
    })
    queuedAppearanceSave = task
    saveQueue = task.catch(() => {})
    return task.finally(() => {
      pendingSaves -= 1
      set({ saving: pendingSaves > 0 })
    })
  },

  receive: (settings) => set(pickSettings(settings)),

  checkTools: async () => {
    const request = ++latestToolCheck
    set({ checkingTools: true, toolError: null })
    try {
      const status = await getApi().system.checkTools()
      if (request === latestToolCheck) set({ toolStatus: status })
    } catch (err) {
      if (request === latestToolCheck) {
        set({ toolStatus: null, toolError: errorMessage(err, 'Could not check required tools. Try again in Settings.') })
      }
    } finally {
      if (request === latestToolCheck) set({ checkingTools: false })
    }
  }
}))

function pickSettings(s: ClipSettings): ClipSettings {
  return {
    openrouterConfigured: s.openrouterConfigured,
    anthropicConfigured: s.anthropicConfigured,
    openaiConfigured: s.openaiConfigured,
    aiProvider: s.aiProvider,
    aiModel: s.aiModel,
    whisperModel: s.whisperModel,
    outputDirectory: s.outputDirectory,
    customVocabulary: s.customVocabulary,
    windowGlass: s.windowGlass ?? false,
    windowGlassSupported: s.windowGlassSupported ?? false,
    // A shell that predates appearance settings reports none: use the defaults.
    appearance: normalizeAppearance(s.appearance),
    windowBlurSupported: s.windowBlurSupported ?? false,
    nativeUiScale: s.nativeUiScale ?? false
  }
}

export type SetupState = {
  ready: boolean
  /** The selected AI provider can run: its CLI is installed or its key is saved. */
  providerReady: boolean
  /** The selected Whisper model is on disk. */
  whisperReady: boolean
  toolsOk: boolean | null
}

/** Whether the selected provider is usable, from settings alone (no network). */
export function isProviderReady(s: Pick<SettingsState, 'aiProvider' | 'clis' | 'openrouterConfigured' | 'anthropicConfigured' | 'openaiConfigured'>): boolean {
  if (s.aiProvider.startsWith('cli:')) return Boolean(s.clis?.find((cli) => `cli:${cli.id}` === s.aiProvider)?.path)
  return { openrouter: s.openrouterConfigured, anthropic: s.anthropicConfigured, openai: s.openaiConfigured }[s.aiProvider] ?? false
}

/** Whether a clip job can start: a usable AI provider, a downloaded Whisper
 *  model and, once the system check has run, every required tool found. */
export function useSetupState(): SetupState {
  const providerReady = useSettingsStore(isProviderReady)
  const whisperReady = useSettingsStore((s) => s.whisperModels.some((model) => model.id === s.whisperModel && model.downloaded))
  const tools = useSettingsStore((s) => s.toolStatus)
  const toolError = useSettingsStore((s) => s.toolError)
  const checkingTools = useSettingsStore((s) => s.checkingTools)
  const toolsOk = toolError ? false : tools
    ? tools.smartFraming && tools.ffmpeg && tools.ffprobe && tools.ytdlp && tools.engine
    : null
  return { ready: providerReady && whisperReady && toolsOk === true && !checkingTools, providerReady, whisperReady, toolsOk }
}
