// Electron preload, kept until the Tauri port replaces the Electron shell.
// The API types now live in src/shared/api.ts (the Tauri bridge implements them).
import { contextBridge, ipcRenderer } from 'electron'
import type { ClipwardAPI } from '../shared/api'

export type { ClipSettings, ClipJobRequest, ClipwardAPI, HistoryEntry, JobSnapshot, ToolStatus } from '../shared/api'

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
    selectOutputDir: () => ipcRenderer.invoke('settings:selectOutputDir'),
    // The Electron shell draws no window glass (settings leave windowGlassSupported unset).
    setWindowGlass: () => Promise.reject(new Error('Window transparency needs the Tauri app')),
    setAppearance: () => Promise.reject(new Error('Appearance settings need the Tauri app')),
    onChanged: () => () => {}
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
