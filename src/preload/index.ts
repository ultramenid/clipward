import { contextBridge, ipcRenderer } from 'electron'
import type { ClipJobRequest, JobSnapshot } from '../shared/jobs'
import type { ApiKeyName, DetectedCli, ProviderTestResult } from '../shared/ai-providers'
import type { WhisperModelStatus } from '../shared/whisper-models'

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
}

export type { ClipJobRequest, JobSnapshot } from '../shared/jobs'

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
  }
  job: {
    /** Queues a clipping run; it starts right away when a slot is free (`queued: false`). */
    start: (config: ClipJobRequest) => Promise<{ jobId?: string; queued?: boolean; error?: string }>
    cancel: (jobId: string) => Promise<boolean>
    /** Every job the main process knows about this session, newest first. */
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

function subscribe<T>(channel: string, callback: (data: T) => void): () => void {
  const handler = (_event: Electron.IpcRendererEvent, data: T): void => callback(data)
  ipcRenderer.on(channel, handler)
  return () => ipcRenderer.removeListener(channel, handler)
}

const api: ClipwardAPI = {
  ai: {
    detect: () => ipcRenderer.invoke('ai:detect'),
    test: () => ipcRenderer.invoke('ai:test')
  },
  whisper: {
    list: () => ipcRenderer.invoke('whisper:list'),
    download: (id) => ipcRenderer.invoke('whisper:download', id),
    cancel: (id) => ipcRenderer.invoke('whisper:cancel', id),
    remove: (id) => ipcRenderer.invoke('whisper:delete', id),
    onProgress: (callback) => subscribe('whisper:progress', callback)
  },
  settings: {
    load: () => ipcRenderer.invoke('settings:load'),
    save: (settings) => ipcRenderer.invoke('settings:save', settings),
    replaceApiKey: (key, value) => ipcRenderer.invoke('settings:replaceApiKey', key, value),
    selectOutputDir: () => ipcRenderer.invoke('settings:selectOutputDir')
  },
  job: {
    start: (config) => ipcRenderer.invoke('job:start', config),
    cancel: (jobId) => ipcRenderer.invoke('job:cancel', jobId),
    list: () => ipcRenderer.invoke('jobs:list'),
    dismiss: (jobId) => ipcRenderer.invoke('jobs:dismiss', jobId),
    onUpdate: (callback) => subscribe('jobs:update', callback)
  },
  history: {
    list: () => ipcRenderer.invoke('history:list'),
    getJob: (outputDir) => ipcRenderer.invoke('history:getJob', outputDir)
  },
  thumbnails: {
    generate: (videoPath, seekSeconds) => ipcRenderer.invoke('thumbnails:generate', videoPath, seekSeconds)
  },
  shell: {
    openPath: (path) => ipcRenderer.invoke('shell:openPath', path),
    showItemInFolder: (path) => ipcRenderer.invoke('shell:showItemInFolder', path)
  },
  dialog: {
    selectVideo: () => ipcRenderer.invoke('dialog:selectVideo')
  },
  clips: {
    bulkExport: (clips) => ipcRenderer.invoke('clips:bulkExport', clips)
  },
  system: {
    isPackaged: () => ipcRenderer.invoke('system:isPackaged'),
    checkTools: () => ipcRenderer.invoke('system:checkTools')
  },
  diagnostics: {
    getLogPath: () => ipcRenderer.invoke('diagnostics:getLogPath'),
    openLogFolder: () => ipcRenderer.invoke('diagnostics:openLogFolder')
  },
  update: {
    onAvailable: (callback) => subscribe('update:available', callback),
    onProgress: (callback) => subscribe('update:progress', callback),
    onDownloaded: (callback) => subscribe('update:downloaded', () => callback()),
    onError: (callback) => subscribe('update:error', callback),
    download: () => ipcRenderer.invoke('update:download'),
    install: () => ipcRenderer.invoke('update:install'),
    check: () => ipcRenderer.invoke('update:check')
  }
}

contextBridge.exposeInMainWorld('clipward', api)
