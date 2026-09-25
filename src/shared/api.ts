/**
 * The API the renderer uses to reach the app shell. In the Tauri app it is
 * implemented by src/renderer/lib/bridge.ts over `invoke`/`listen`; the shape is
 * the one the Electron preload exposed as `window.clipward`.
 */
import type { ClipJobRequest, JobSnapshot } from './jobs'
import type { ApiKeyName, DetectedCli, ProviderTestResult } from './ai-providers'
import type { WhisperModelStatus } from './whisper-models'
import type { Appearance } from './appearance'

export type { ClipJobRequest, JobSnapshot } from './jobs'

export interface ClipSettings {
  openrouterConfigured: boolean
  anthropicConfigured: boolean
  openaiConfigured: boolean
  /** 'openrouter' | 'anthropic' | 'openai' | 'cli:<id>' */
  aiProvider: string
  aiModel: string
  whisperModel: string
  outputDirectory: string
  pythonPath: string
  customVocabulary: string
  /** Native window glass (macOS vibrancy, Windows Mica/acrylic). Change it with settings.setWindowGlass. */
  windowGlass: boolean
  /** Whether this platform draws window glass; Linux stays opaque. */
  windowGlassSupported: boolean
  /** Settings → Appearance (src/shared/appearance.ts). Change it with settings.setAppearance. */
  appearance: Appearance
  /** Whether the shell can change the glass blur radius (macOS). */
  windowBlurSupported: boolean
  /** The shell applies appearance.uiScale as the webview zoom; otherwise the renderer scales itself. */
  nativeUiScale: boolean
}

export interface HistoryEntry {
  jobId: string
  date: string
  videoTitle: string
  clipCount: number
  status: 'completed' | 'failed' | 'cancelled' | 'running' | 'interrupted' | 'incomplete'
  outputDir: string
  totalCostUsd: number | null
  finishedAt: string | null
  durationMs: number | null
  errorMessage: string | null
}

/**
 * Result of Settings → System check. The clipping engine is built into the app,
 * so `python` is always true, `pythonDeps` reports the smart framing model and
 * `engine`/`enginePath` the bundled fonts and models; `bridgeRunner` is always true.
 */
export interface ToolStatus {
  python: boolean
  pythonDeps: boolean
  pythonPath: string
  pythonError: string | null
  ffmpeg: boolean
  ffmpegCaptions: boolean
  ffprobe: boolean
  ytdlp: boolean
  engine: boolean
  enginePath: string
  bridgeRunner: boolean
  bridgePath: string
}

export interface ClipwardAPI {
  ai: {
    /** Supported CLIs and where each is installed. */
    detect: () => Promise<DetectedCli[]>
    /** One tiny request through the selected provider. */
    test: () => Promise<ProviderTestResult>
  }
  whisper: {
    list: () => Promise<WhisperModelStatus[]>
    /** Resolves when the download finishes; progress arrives via onProgress. */
    download: (id: string) => Promise<WhisperModelStatus[]>
    cancel: (id: string) => Promise<void>
    remove: (id: string) => Promise<WhisperModelStatus[]>
    onProgress: (callback: (status: WhisperModelStatus) => void) => () => void
  }
  settings: {
    load: () => Promise<ClipSettings>
    save: (settings: ClipSettings) => Promise<ClipSettings>
    replaceApiKey: (key: ApiKeyName, value: string) => Promise<ClipSettings>
    selectOutputDir: () => Promise<string | null>
    /** Switches native window glass now and remembers the choice. */
    setWindowGlass: (enabled: boolean) => Promise<ClipSettings>
    /** Saves the appearance settings; the shell applies interface scale and blur radius. */
    setAppearance: (appearance: Appearance) => Promise<ClipSettings>
    /** Settings changed outside the renderer (the View menu's zoom items). */
    onChanged: (callback: (settings: ClipSettings) => void) => () => void
  }
  job: {
    /** Queues a clipping run; it starts right away when a slot is free (`queued: false`). */
    start: (config: ClipJobRequest) => Promise<{ jobId?: string; queued?: boolean; error?: string }>
    cancel: (jobId: string) => Promise<boolean>
    /** Every job the app shell knows about this session, newest first. */
    list: () => Promise<JobSnapshot[]>
    /** Forget a finished job for this session; its run folder stays in the library. */
    dismiss: (jobId: string) => Promise<boolean>
    /** A fresh snapshot each time any job changes. */
    onUpdate: (callback: (job: JobSnapshot) => void) => () => void
  }
  history: {
    list: () => Promise<HistoryEntry[]>
    getJob: (outputDir: string) => Promise<Record<string, unknown> | null>
  }
  thumbnails: {
    generate: (videoPath: string, seekSeconds?: number) => Promise<string | null>
  }
  shell: {
    /** Opens a local path with its default app, or an http(s) URL in the browser. */
    openPath: (path: string) => Promise<boolean>
    showItemInFolder: (path: string) => Promise<boolean>
  }
  dialog: {
    selectVideo: () => Promise<string | null>
  }
  clips: {
    bulkExport: (clips: { path: string; name: string }[]) => Promise<{ success: boolean; count: number; failedCount: number; destDir?: string }>
  }
  system: {
    isPackaged: () => Promise<boolean>
    checkTools: () => Promise<ToolStatus>
  }
  diagnostics: {
    getLogPath: () => Promise<string>
    openLogFolder: () => Promise<boolean>
  }
  update: {
    onAvailable: (cb: (info: { version: string; releaseNotes?: string; releaseDate?: string }) => void) => () => void
    onProgress: (cb: (info: { percent: number; bytesPerSecond: number; transferred: number; total: number }) => void) => () => void
    onDownloaded: (cb: () => void) => () => void
    onError: (cb: (info: { message: string }) => void) => () => void
    download: () => Promise<void>
    install: () => Promise<void>
    check: () => Promise<void>
  }
}
