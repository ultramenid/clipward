// Single source for product identity, shared by the main process (menu,
// window title) and the renderer.
export const APP_NAME = 'Clipward'
export const APP_TAGLINE = 'AI clipping for content-reward campaigns'

export const REPO_URL = 'https://github.com/clipward/clipward'
export const ISSUES_URL = `${REPO_URL}/issues`
export const LICENSE_NAME = 'MIT'
export const CLIPWARD_URL = 'https://www.clipward.ai'
export const DISCORD_URL = 'https://www.clipward.ai/discord'

export const PROVIDER_LINKS = {
  openrouter: 'https://openrouter.ai/keys',
  anthropic: 'https://console.anthropic.com/settings/keys',
  openai: 'https://platform.openai.com/api-keys'
} as const
