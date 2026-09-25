import type { ClipwardAPI } from '../../shared/api'
import { bridge } from './bridge'

declare global {
  interface Window {
    /** Test seam: renderer tests inject a stand-in API here. The app itself uses the Tauri bridge. */
    clipward?: ClipwardAPI
  }
}

export function getApi(): ClipwardAPI {
  return window.clipward ?? bridge
}
