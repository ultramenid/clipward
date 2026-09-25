import { app, net } from 'electron'
import { createWriteStream, existsSync, mkdirSync, renameSync, rmSync, statSync } from 'fs'
import { join } from 'path'
import { Readable } from 'stream'
import { pipeline } from 'stream/promises'
import { isWhisperModelId, WHISPER_MODELS, type WhisperModelStatus } from '../shared/whisper-models'
import { logger } from './logger'

const BASE_URL = 'https://huggingface.co/ggerganov/whisper.cpp/resolve/main'
const downloads = new Map<string, { controller: AbortController; received: number; total: number | null }>()

function modelsDir(): string {
  const dir = join(app.getPath('userData'), 'whisper-models')
  mkdirSync(dir, { recursive: true, mode: 0o700 })
  return dir
}

export function whisperModelPath(id: string): string {
  if (!isWhisperModelId(id)) throw new Error('Unknown Whisper model')
  return join(modelsDir(), `ggml-${id}.bin`)
}

/** The downloaded file for `id`, or null when it has not been downloaded yet. */
export function downloadedWhisperModel(id: string): string | null {
  if (!isWhisperModelId(id)) return null
  const path = whisperModelPath(id)
  return existsSync(path) ? path : null
}

export function listWhisperModels(): WhisperModelStatus[] {
  return WHISPER_MODELS.map(({ id }) => {
    const active = downloads.get(id)
    return { id, downloaded: downloadedWhisperModel(id) !== null, receivedBytes: active?.received ?? null, totalBytes: active?.total ?? null }
  })
}

/**
 * Downloads a model to a `.part` file and renames it only when complete, so a
 * crash or cancel never leaves a truncated model that looks usable.
 */
export async function downloadWhisperModel(id: unknown, onProgress: (status: WhisperModelStatus) => void): Promise<WhisperModelStatus[]> {
  if (!isWhisperModelId(id)) throw new Error('Unknown Whisper model')
  if (downloads.has(id) || downloadedWhisperModel(id)) return listWhisperModels()
  const target = whisperModelPath(id)
  const part = `${target}.part`
  const controller = new AbortController()
  const state = { controller, received: 0, total: null as number | null }
  downloads.set(id, state)
  let lastReport = 0
  try {
    const response = await net.fetch(`${BASE_URL}/ggml-${id}.bin`, { signal: controller.signal })
    if (!response.ok || !response.body) throw new Error(`Download failed (${response.status})`)
    state.total = Number(response.headers.get('content-length')) || null
    const body = Readable.fromWeb(response.body as import('stream/web').ReadableStream)
    body.on('data', (chunk: Buffer) => {
      state.received += chunk.length
      const now = Date.now()
      if (now - lastReport > 250) {
        lastReport = now
        onProgress({ id, downloaded: false, receivedBytes: state.received, totalBytes: state.total })
      }
    })
    await pipeline(body, createWriteStream(part, { mode: 0o600 }))
    if (state.total !== null && statSync(part).size !== state.total) throw new Error('Download was incomplete')
    renameSync(part, target)
    logger.info('whisper.download.done', { id })
  } catch (error) {
    rmSync(part, { force: true })
    if (!controller.signal.aborted) {
      logger.warn('whisper.download.failed', { id })
      throw new Error('The Whisper model could not be downloaded. Check your connection and retry.', { cause: error })
    }
  } finally {
    downloads.delete(id)
    onProgress({ id, downloaded: downloadedWhisperModel(id) !== null, receivedBytes: null, totalBytes: null })
  }
  return listWhisperModels()
}

export function cancelWhisperDownload(id: unknown): void {
  if (typeof id === 'string') downloads.get(id)?.controller.abort()
}

export function deleteWhisperModel(id: unknown): WhisperModelStatus[] {
  if (!isWhisperModelId(id)) throw new Error('Unknown Whisper model')
  rmSync(whisperModelPath(id), { force: true })
  return listWhisperModels()
}
