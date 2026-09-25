import { autoUpdater } from 'electron-updater'
import { BrowserWindow, ipcMain } from 'electron'
import { assertTrustedSender } from './security'
import { is } from '@electron-toolkit/utils'

export function initAutoUpdater(
  getMainWindow: () => BrowserWindow | null,
): void {
  if (is.dev) return

  autoUpdater.autoDownload = false
  autoUpdater.autoInstallOnAppQuit = true

  const send = (channel: string, data?: unknown): void => {
    const win = getMainWindow()
    if (win && !win.isDestroyed()) {
      win.webContents.send(channel, data)
    }
  }

  autoUpdater.on('update-available', (info) => {
    send('update:available', {
      version: info.version,
      releaseNotes:
        typeof info.releaseNotes === 'string'
          ? info.releaseNotes
          : undefined,
      releaseDate: info.releaseDate,
    })
  })

  autoUpdater.on('download-progress', (progress) => {
    send('update:progress', {
      percent: progress.percent,
      bytesPerSecond: progress.bytesPerSecond,
      transferred: progress.transferred,
      total: progress.total,
    })
  })

  autoUpdater.on('update-downloaded', () => {
    send('update:downloaded')
  })

  autoUpdater.on('error', (err) => {
    console.error('Auto-updater error:', err.name)
    send('update:error', { message: 'Could not check or install the update. Please retry from Settings.' })
  })

  ipcMain.handle('update:download', async (event) => { assertTrustedSender(event, getMainWindow()); await autoUpdater.downloadUpdate() })
  ipcMain.handle('update:install', (event) => { assertTrustedSender(event, getMainWindow()); autoUpdater.quitAndInstall(false, true) })
  ipcMain.handle('update:check', async (event) => { assertTrustedSender(event, getMainWindow()); await autoUpdater.checkForUpdates() })

  setTimeout(() => {
    autoUpdater.checkForUpdates().catch(() => {})
  }, 5000)
}
