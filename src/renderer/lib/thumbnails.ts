import { getApi } from './ipc'

/**
 * Thumbnails are extracted with a synchronous ffmpeg call in the main process,
 * so firing one IPC per card at once stalls the main thread for the whole
 * batch. Run them one at a time instead, newest request last, and memoise the
 * result for the session (the files are cached on disk as *_thumb.jpg too).
 */
const cache = new Map<string, Promise<string | null>>()
let tail: Promise<unknown> = Promise.resolve()

export function loadThumbnail(videoPath: string, seekSeconds?: number): Promise<string | null> {
  const cached = cache.get(videoPath)
  if (cached) return cached

  const task = tail
    .then(() => getApi().thumbnails.generate(videoPath, seekSeconds))
    .catch(() => null)
    .then((result) => {
      // A missing source or temporary ffmpeg failure can recover later.
      if (!result) cache.delete(videoPath)
      return result
    })
  tail = task
  cache.set(videoPath, task)
  return task
}

/** file:///a/b.mp4 → /a/b.mp4; plain paths pass through. Clipward engine writes the
 *  raw path after the scheme (not percent-encoded), so don't decode it. */
export function clipFilePath(url: string): string {
  return url.startsWith('file://') ? url.slice('file://'.length) : url
}
