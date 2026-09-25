/** Coding CLIs Clipward can plan with, using the user's own login. Mirrors CLI_PROVIDERS in engine/clip_engine/services/llm_gateway.py. */
export const AI_CLIS = [
  { id: 'claude', label: 'Claude Code', binaries: ['claude'], hint: 'Claude Pro or Max plan' },
  { id: 'codex', label: 'Codex', binaries: ['codex'], hint: 'ChatGPT plan' },
  { id: 'agy', label: 'Antigravity', binaries: ['agy'], hint: 'Google account' },
  { id: 'grok', label: 'Grok', binaries: ['grok'], hint: 'xAI / SuperGrok' },
  { id: 'cursor-agent', label: 'Cursor', binaries: ['cursor-agent', 'agent'], hint: 'Cursor plan' },
  { id: 'opencode', label: 'OpenCode', binaries: ['opencode'], hint: 'Your OpenCode providers' },
  { id: 'gemini', label: 'Gemini CLI', binaries: ['gemini'], hint: 'Google account' }
] as const

export const API_PROVIDERS = [
  { id: 'openrouter', label: 'OpenRouter', key: 'openrouterApiKey', placeholder: 'sk-or-…', defaultModel: 'anthropic/claude-opus-5.5', url: 'https://openrouter.ai/keys' },
  { id: 'anthropic', label: 'Anthropic', key: 'anthropicApiKey', placeholder: 'sk-ant-…', defaultModel: 'claude-opus-5', url: 'https://console.anthropic.com/settings/keys' },
  { id: 'openai', label: 'OpenAI', key: 'openaiApiKey', placeholder: 'sk-…', defaultModel: 'gpt-6-sol', url: 'https://platform.openai.com/api-keys' }
] as const

export type ApiKeyName = (typeof API_PROVIDERS)[number]['key']

export interface DetectedCli {
  id: string
  label: string
  /** Absolute path when installed, else null. */
  path: string | null
}

export interface ProviderTestResult {
  ok: boolean
  message: string
}

export function providerLabel(provider: string): string {
  const cli = AI_CLIS.find((item) => `cli:${item.id}` === provider)
  if (cli) return `${cli.label} CLI`
  return API_PROVIDERS.find((item) => item.id === provider)?.label ?? provider
}
