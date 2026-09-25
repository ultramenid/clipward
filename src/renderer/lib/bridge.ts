/**
 * The renderer's API to the Tauri app shell (src-tauri/src/commands.rs) over
 * `invoke`/`listen`. Components reach it through `getApi()`.
 */
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { ClipwardAPI } from '../../shared/api'

/** Subscribe to an app-shell event; the returned function unsubscribes. */
function subscribe<T>(event: string, callback: (payload: T) => void): () => void {
  let unlisten: (() => void) | null = null
  let disposed = false
  listen<T>(event, (message) => callback(message.payload))
    .then((stop) => {
      if (disposed) stop()
      else unlisten = stop
    })
    .catch(() => { /* No event bridge (e.g. in a plain browser); nothing to unsubscribe. */ })
  return () => {
    disposed = true
    unlisten?.()
    unlisten = null
  }
}

export const bridge: ClipwardAPI = {
  ai: {
    detect: () => invoke('ai_detect'),
    test: () => invoke('ai_test'),
    localModels: () => invoke('ai_local_models')
  },
  whisper: {
    list: () => invoke('whisper_list'),
    download: (id) => invoke('whisper_download', { id }),
    cancel: (id) => invoke('whisper_cancel', { id }),
    remove: (id) => invoke('whisper_delete', { id }),
    onProgress: (callback) => subscribe('whisper:progress', callback)
  },
  settings: {
    load: () => invoke('settings_load'),
    save: (settings) => invoke('settings_save', { settings }),
    replaceApiKey: (key, value) => invoke('settings_replace_api_key', { key, value }),
    selectOutputDir: () => invoke('settings_select_output_dir'),
    setWindowGlass: (enabled) => invoke('window_set_glass', { enabled }),
    setAppearance: (appearance) => invoke('settings_set_appearance', { appearance }),
    onChanged: (callback) => subscribe('settings:changed', callback)
  },
  job: {
    start: (config) => invoke('job_start', { config }),
    cancel: (jobId) => invoke('job_cancel', { jobId }),
    list: () => invoke('jobs_list'),
    dismiss: (jobId) => invoke('jobs_dismiss', { jobId }),
    onUpdate: (callback) => subscribe('jobs:update', callback)
  },
  history: {
    list: () => invoke('history_list'),
    getJob: (outputDir) => invoke('history_get_job', { outputDir })
  },
  thumbnails: {
    generate: (videoPath, seekSeconds) => invoke('thumbnails_generate', { videoPath, seekSeconds: seekSeconds ?? null })
  },
  shell: {
    openPath: (path) => invoke('shell_open_path', { path }),
    showItemInFolder: (path) => invoke('shell_show_item_in_folder', { path })
  },
  dialog: {
    selectVideo: () => invoke('dialog_select_video')
  },
  clips: {
    bulkExport: (clips) => invoke('clips_bulk_export', { clips })
  },
  system: {
    isPackaged: () => invoke('system_is_packaged'),
    checkTools: () => invoke('system_check_tools')
  },
  diagnostics: {
    getLogPath: () => invoke('diagnostics_get_log_path'),
    openLogFolder: () => invoke('diagnostics_open_log_folder')
  },
  update: {
    // TODO(updater): the shell emits these once tauri-plugin-updater is wired.
    onAvailable: (callback) => subscribe('update:available', callback),
    onProgress: (callback) => subscribe('update:progress', callback),
    onDownloaded: (callback) => subscribe('update:downloaded', () => callback()),
    onError: (callback) => subscribe('update:error', callback),
    download: () => invoke('update_download'),
    install: () => invoke('update_install'),
    check: () => invoke('update_check')
  }
}
