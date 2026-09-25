import type { ManualClip } from '../../shared/jobs'
import {
  MAX_MANUAL_CLIPS,
  MAX_MANUAL_CLIP_SECONDS,
  MAX_MANUAL_TITLE_CHARS,
  MIN_MANUAL_CLIP_SECONDS
} from '../../shared/job-contract'
import { parseTimecode } from './utils'

/** One editable row of the manual clip list, as typed. */
export interface ManualClipRow {
  id: string
  start: string
  end: string
  title: string
}

export interface ManualRowCheck {
  start: number | null
  end: number | null
  /** Seconds between start and end once both parse. */
  length: number | null
  /** First problem with the row, shown under it. */
  error: string | null
  /** Which field the error points at, for aria-invalid. */
  field: 'start' | 'end' | 'title' | null
}

export interface ManualClipsCheck {
  rows: ManualRowCheck[]
  /** Problem with the list as a whole (none, or too many clips). */
  listError: string | null
  valid: boolean
}

let nextRowId = 0

export function newManualRow(patch: Partial<Omit<ManualClipRow, 'id'>> = {}): ManualClipRow {
  nextRowId += 1
  return { id: `clip-${Date.now().toString(36)}-${nextRowId}`, start: '', end: '', title: '', ...patch }
}

/** "90", "1:30", "1:02:03" or "75.5" in seconds; null when empty, NaN when unreadable. */
export function parseClipTime(text: string): number | null {
  return parseTimecode(text)
}

/** Seconds as m:ss or h:mm:ss, keeping a tenth of a second when there is one. */
export function formatClipTime(seconds: number): string {
  const tenths = Math.round(seconds * 10)
  const whole = Math.floor(tenths / 10)
  const h = Math.floor(whole / 3600)
  const m = Math.floor((whole % 3600) / 60)
  const s = whole % 60
  const fraction = tenths % 10 ? `.${tenths % 10}` : ''
  const ss = `${String(s).padStart(2, '0')}${fraction}`
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`
}

/** A clip's length for the Duration column: "45s", "1m 30s", "15m". */
export function formatClipLength(seconds: number): string {
  const rounded = Math.round(seconds * 10) / 10
  if (rounded < 60) return `${Number.isInteger(rounded) ? rounded : rounded.toFixed(1)}s`
  const whole = Math.round(rounded)
  const m = Math.floor(whole / 60)
  const s = whole % 60
  return s ? `${m}m ${s}s` : `${m}m`
}

const TIME_HINT = 'Use seconds (90), m:ss (1:30) or h:mm:ss.'

/**
 * Checks one row against the limits the app shell enforces (end after start,
 * 3 s to 15 min, a short one-line title) and, when the video length is known,
 * that the clip fits inside the video.
 */
export function checkManualRow(row: ManualClipRow, videoSeconds: number | null): ManualRowCheck {
  const start = parseClipTime(row.start)
  const end = parseClipTime(row.end)
  const length = start != null && end != null && !Number.isNaN(start) && !Number.isNaN(end) ? end - start : null
  const fail = (error: string, field: ManualRowCheck['field']): ManualRowCheck => ({ start, end, length, error, field })
  if (start == null) return fail('Enter a start time.', 'start')
  if (Number.isNaN(start)) return fail(TIME_HINT, 'start')
  if (end == null) return fail('Enter an end time.', 'end')
  if (Number.isNaN(end)) return fail(TIME_HINT, 'end')
  if (end <= start) return fail('End must be after the start.', 'end')
  const known = videoSeconds != null && Number.isFinite(videoSeconds) && videoSeconds > 0
  if (known && start >= videoSeconds) return fail(`Starts after the video ends (${formatClipTime(videoSeconds)}).`, 'start')
  if (known && end > videoSeconds + 0.05) return fail(`Ends after the video ends (${formatClipTime(videoSeconds)}).`, 'end')
  if (length != null && length < MIN_MANUAL_CLIP_SECONDS) return fail(`Clips must be at least ${MIN_MANUAL_CLIP_SECONDS} seconds.`, 'end')
  if (length != null && length > MAX_MANUAL_CLIP_SECONDS) return fail('Clips can be at most 15 minutes.', 'end')
  if (row.title.trim().length > MAX_MANUAL_TITLE_CHARS) return fail(`Titles can be at most ${MAX_MANUAL_TITLE_CHARS} characters.`, 'title')
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(row.title)) return fail('Titles must be a single line.', 'title')
  return { start, end, length, error: null, field: null }
}

export function checkManualClips(rows: ManualClipRow[], videoSeconds: number | null): ManualClipsCheck {
  const checks = rows.map((row) => checkManualRow(row, videoSeconds))
  const listError = rows.length === 0
    ? 'Add at least one clip.'
    : rows.length > MAX_MANUAL_CLIPS ? `Add at most ${MAX_MANUAL_CLIPS} clips.` : null
  return { rows: checks, listError, valid: !listError && checks.every((check) => !check.error) }
}

/** The request's manualClips for valid rows (titles trimmed, empty titles dropped). */
export function toManualClips(rows: ManualClipRow[]): ManualClip[] {
  return rows.map((row) => {
    const title = row.title.trim()
    return {
      startSeconds: parseClipTime(row.start) ?? 0,
      endSeconds: parseClipTime(row.end) ?? 0,
      title: title || null
    }
  })
}
