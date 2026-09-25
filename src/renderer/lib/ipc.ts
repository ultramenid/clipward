import type { ClipwardAPI } from '../../preload/index'

declare global {
  interface Window {
    clipward: ClipwardAPI
  }
}

export function getApi(): ClipwardAPI {
  return window.clipward
}
