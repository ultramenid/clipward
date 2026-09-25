// Settings → Appearance: values, ranges and the store's live-preview saves.
// The value tests are ported from MonoCode's appearance.test.ts and
// uiScale.test.ts (MIT, Copyright (c) 2026 Nick; see THIRD_PARTY_NOTICES.md).
const assert = require('node:assert/strict')
const { test } = require('node:test')
const vm = require('node:vm')
const path = require('node:path')
const { buildSync } = require('esbuild')

const bundle = buildSync({
  stdin: {
    contents: `export * from './src/shared/appearance';
      export { useSettingsStore } from './src/renderer/store/use-settings-store';
      export { hexToHsv, hsvToHex, hexToChannels, mixChannels } from './src/renderer/lib/color';`,
    resolveDir: path.resolve(__dirname, '..'),
    loader: 'ts'
  },
  bundle: true, platform: 'node', format: 'cjs', packages: 'external', write: false
}).outputFiles[0].text

function load(api = {}) {
  const module = { exports: {} }
  const defaults = { ai: { detect: async () => [] }, whisper: { list: async () => [] } }
  vm.runInNewContext(bundle, { module, exports: module.exports, require, window: { clipward: { ...defaults, ...api } } })
  return module.exports
}

const look = load()

test('defaults match MonoCode', () => {
  const d = look.APPEARANCE_DEFAULTS
  assert.equal(d.theme, 'dark')
  assert.equal(d.accentColor, null)
  assert.deepEqual([d.hue, d.saturation, d.darkLightness], [240, 0, 9])
  assert.deepEqual([d.sidebarOpacity, d.blurRadius, d.mainPaneGlass], [0.85, 24, true])
  assert.equal(d.uiScale, 1)
  assert.equal(d.collapsedRail, false)
  assert.deepEqual([...look.ACCENT_COLOR_PRESETS], ['#4da3f5', '#8b5cf6', '#ec4899', '#ef4444', '#f59e0b', '#10b981'])
})

test('accent colors are normalized hex, anything else is the default', () => {
  assert.equal(look.normalizeAccentColor('#AABBCC'), '#aabbcc')
  assert.equal(look.normalizeAccentColor('tomato'), null)
  assert.equal(look.normalizeAccentColor('#abc'), null)
  assert.equal(look.normalizeAccentColor(null), null)
})

test('theme preference: unknown values fall back, system follows the OS', () => {
  assert.equal(look.normalizeAppearance({ theme: 'solarized' }).theme, 'dark')
  for (const theme of ['system', 'light', 'dark']) assert.equal(look.normalizeAppearance({ theme }).theme, theme)
  assert.equal(look.resolveColorScheme('system', true), 'light')
  assert.equal(look.resolveColorScheme('system', false), 'dark')
  assert.equal(look.resolveColorScheme('dark', true), 'dark')
  assert.equal(look.resolveColorScheme('light', false), 'light')
  // Without matchMedia, dark.
  assert.equal(look.resolveColorScheme('system', null), 'dark')
})

test('tint, lightness, glass and blur are clamped and rounded', () => {
  const a = look.normalizeAppearance({ hue: 400.4, saturation: -3, darkLightness: 100, sidebarOpacity: 0, blurRadius: 99 })
  assert.deepEqual([a.hue, a.saturation, a.darkLightness, a.sidebarOpacity, a.blurRadius], [360, 0, 30, 0.15, 64])
  const b = look.normalizeAppearance({ darkLightness: 0, sidebarOpacity: 0.456, blurRadius: 0 })
  assert.deepEqual([b.darkLightness, b.sidebarOpacity, b.blurRadius], [0, 0.46, 1])
  // Missing or foreign input takes the defaults.
  const plain = (value) => JSON.parse(JSON.stringify(value))
  assert.deepEqual(plain(look.normalizeAppearance(undefined)), plain(look.APPEARANCE_DEFAULTS))
  assert.deepEqual(plain(look.normalizeAppearance({ hue: 'junk', mainPaneGlass: 'yes' })), plain(look.APPEARANCE_DEFAULTS))
})

test('ui scale clamps to the supported range and rounds to one decimal', () => {
  assert.equal(look.normalizeUiScale(1), 1)
  assert.equal(look.normalizeUiScale(1.05), 1.1)
  assert.equal(look.normalizeUiScale(0), look.UI_SCALE_MIN)
  assert.equal(look.normalizeUiScale(99), look.UI_SCALE_MAX)
  assert.equal(look.normalizeUiScale(Number.NaN), look.UI_SCALE_DEFAULT)
  assert.equal(look.normalizeUiScale('junk'), look.UI_SCALE_DEFAULT)
})

test('ui scale steps in and out without float drift', () => {
  assert.equal(look.zoomInUiScale(1), 1.1)
  assert.equal(look.zoomOutUiScale(1.1), 1)
  let scale = 1
  for (let i = 0; i < 10; i += 1) scale = look.zoomInUiScale(scale)
  assert.equal(scale, look.UI_SCALE_MAX)
  assert.equal(look.zoomInUiScale(look.UI_SCALE_MAX), look.UI_SCALE_MAX)
  assert.equal(look.zoomOutUiScale(look.UI_SCALE_MIN), look.UI_SCALE_MIN)
})

test('accent text is black on light fills and white on dark ones', () => {
  assert.equal(look.accentForeground('#f59e0b'), '#000000')
  assert.equal(look.accentForeground('#1e3a8a'), '#ffffff')
  assert.equal(look.hexToChannels('#4da3f5'), '77 163 245')
  assert.equal(look.mixChannels('#000000', 255, 0.5), '128 128 128')
})

test('the color picker round-trips hex through HSV', () => {
  for (const hex of ['#4da3f5', '#ec4899', '#10b981', '#000000', '#ffffff']) {
    const { h, s, v } = look.hexToHsv(hex)
    assert.equal(look.hsvToHex(h, s, v), hex)
  }
  assert.equal(look.hsvToHex(0, 100, 100), '#ff0000')
})

function deferred() {
  let resolve
  const promise = new Promise((yes) => { resolve = yes })
  return { promise, resolve }
}

const settings = { openrouterConfigured: false, outputDirectory: '/clips' }

test('a shell without appearance settings reads as the defaults, scaled by the page', async () => {
  const { useSettingsStore, APPEARANCE_DEFAULTS } = load({ settings: { load: async () => settings } })
  await useSettingsStore.getState().load()
  assert.deepEqual(JSON.parse(JSON.stringify(useSettingsStore.getState().appearance)), JSON.parse(JSON.stringify(APPEARANCE_DEFAULTS)))
  assert.equal(useSettingsStore.getState().nativeUiScale, false)
  assert.equal(useSettingsStore.getState().windowBlurSupported, false)
})

test('appearance changes show at once and a burst saves the latest value once', async () => {
  const first = deferred()
  const sent = []
  const { useSettingsStore } = load({ settings: {
    load: async () => ({ ...settings, nativeUiScale: true }),
    setAppearance: async (appearance) => {
      sent.push(appearance.hue)
      if (sent.length === 1) await first.promise
      return { ...settings, nativeUiScale: true, appearance }
    }
  } })
  await useSettingsStore.getState().load()
  const a = useSettingsStore.getState().setAppearance({ hue: 10 })
  await Promise.resolve()
  // In flight: these queue behind it and collapse into one write.
  const b = useSettingsStore.getState().setAppearance({ hue: 20 })
  const c = useSettingsStore.getState().setAppearance({ hue: 999 })
  assert.equal(useSettingsStore.getState().appearance.hue, 360)
  first.resolve()
  await Promise.all([a, b, c])
  assert.deepEqual(sent, [10, 360])
  assert.equal(useSettingsStore.getState().appearance.hue, 360)
  assert.equal(useSettingsStore.getState().saving, false)
})

test('the menu zoom reaches the store through receive', async () => {
  const { useSettingsStore } = load({ settings: { load: async () => settings } })
  await useSettingsStore.getState().load()
  useSettingsStore.getState().receive({ ...settings, nativeUiScale: true, appearance: { uiScale: 1.3 } })
  assert.equal(useSettingsStore.getState().appearance.uiScale, 1.3)
  assert.equal(useSettingsStore.getState().nativeUiScale, true)
})
