export const DURATION_OPTIONS = [
  { id: 'xshort', label: 'Extra short', range: '10–30s' },
  { id: 'short', label: 'Short', range: '30–60s' },
  { id: 'medium', label: 'Medium', range: '1–2m' },
  { id: 'long', label: 'Long', range: '2–5m' },
  { id: 'xlong', label: 'Extra long', range: '5–10m' },
  { id: 'extended', label: 'Extended', range: '10–15m' },
  { id: 'feature', label: 'Feature', range: '15–30m' }
] as const

/** Increment when the desktop bridge and bundled Clipward engine job contract change. */
export const BRIDGE_CONTRACT_VERSION = 3

/** Longest campaign brief the engine accepts (mirrors bridge_runner.validate_config). */
export const MAX_REQUIREMENTS_CHARS = 4000

export const VIDEO_SPEED_OPTIONS = [1, 1.1, 1.25, 1.5, 1.75, 2] as const

export function isVideoSpeed(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 1 && value <= 2
}

export type DurationId = (typeof DURATION_OPTIONS)[number]['id']
export const DURATION_IDS: readonly string[] = DURATION_OPTIONS.map((option) => option.id)

/** Manual clip mode limits (mirrors crates/engine/src/manual.rs and src-tauri validation). */
export const MAX_MANUAL_CLIPS = 50
export const MIN_MANUAL_CLIP_SECONDS = 3
export const MAX_MANUAL_CLIP_SECONDS = 15 * 60
export const MAX_MANUAL_TITLE_CHARS = 120
