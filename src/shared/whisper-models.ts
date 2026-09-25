/** Offline transcription models (whisper.cpp ggml files from huggingface.co/ggerganov/whisper.cpp). */
export const WHISPER_MODELS = [
  { id: 'base', label: 'Base', sizeMb: 148, note: 'Fastest. Rough on names and noisy audio.' },
  { id: 'small', label: 'Small', sizeMb: 488, note: 'Fast. Good for clear speech.' },
  { id: 'large-v3-turbo-q5_0', label: 'Large v3 Turbo (compact)', sizeMb: 574, note: 'Recommended. Near-best accuracy, quick.' },
  { id: 'medium', label: 'Medium', sizeMb: 1533, note: 'Slower. Good accuracy.' },
  { id: 'large-v3-turbo', label: 'Large v3 Turbo', sizeMb: 1624, note: 'Best accuracy for its speed.' }
] as const

export type WhisperModelId = (typeof WHISPER_MODELS)[number]['id']

export interface WhisperModelStatus {
  id: string
  downloaded: boolean
  /** Bytes received while a download is running; null otherwise. */
  receivedBytes: number | null
  totalBytes: number | null
}

export function isWhisperModelId(value: unknown): value is WhisperModelId {
  return WHISPER_MODELS.some((model) => model.id === value)
}
