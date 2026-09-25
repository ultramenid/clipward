'use strict'
// Bundles main-process TypeScript with esbuild and runs it as a fresh CommonJS
// module, with `electron` (and any other ids) replaced by test doubles. Each
// call gets its own copy of module-level state.

const Module = require('node:module')
const path = require('node:path')
const fs = require('node:fs')
const os = require('node:os')
const { buildSync } = require('esbuild')

const ROOT = path.resolve(__dirname, '../..')

/**
 * @param {string} source ES module source whose imports resolve from the repo root,
 *   e.g. `export * from './src/main/settings-store'`.
 * @param {Record<string, unknown>} mocks Module ids to replace (e.g. { electron }).
 */
function loadMain(source, mocks = {}) {
  const code = buildSync({
    stdin: { contents: source, resolveDir: ROOT, loader: 'ts', sourcefile: 'entry.ts' },
    bundle: true,
    platform: 'node',
    format: 'cjs',
    target: 'node20',
    external: ['electron', ...Object.keys(mocks)],
    write: false,
    logLevel: 'silent'
  }).outputFiles[0].text
  const filename = path.join(ROOT, 'tests/support/.bundle.cjs')
  const mod = new Module(filename, module)
  mod.filename = filename
  mod.paths = Module._nodeModulePaths(ROOT)
  const realRequire = mod.require.bind(mod)
  mod.require = (id) => (Object.prototype.hasOwnProperty.call(mocks, id) ? mocks[id] : realRequire(id))
  mod._compile(code, filename)
  return mod.exports
}

/** A temp directory removed by the returned `cleanup`. */
function tempDir(prefix = 'clipward-test-') {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), prefix))
  return { dir, cleanup: () => fs.rmSync(dir, { recursive: true, force: true }) }
}

/**
 * Enough of Electron for main-process modules: paths under `dir`, a
 * reversible fake safeStorage, and spies for shell/app/window calls.
 */
function fakeElectron(dir, { isPackaged = false } = {}) {
  const calls = { openExternal: [], focus: [], sent: [], shown: 0, restored: 0, focused: 0 }
  const window = {
    destroyed: false,
    minimized: false,
    isDestroyed: () => window.destroyed,
    isMinimized: () => window.minimized,
    restore: () => { calls.restored += 1 },
    show: () => { calls.shown += 1 },
    focus: () => { calls.focused += 1 },
    webContents: { send: (channel, payload) => calls.sent.push({ channel, payload }) }
  }
  const electron = {
    app: {
      isPackaged,
      isReady: () => true,
      getPath: (name) => {
        const target = path.join(dir, name)
        fs.mkdirSync(target, { recursive: true })
        return target
      },
      focus: (options) => calls.focus.push(options)
    },
    safeStorage: {
      isEncryptionAvailable: () => true,
      getSelectedStorageBackend: () => 'keychain',
      encryptString: (value) => Buffer.from(`enc:${value}`),
      decryptString: (buffer) => buffer.toString().replace(/^enc:/, '')
    },
    shell: {
      openExternal: async (url) => { calls.openExternal.push(url) }
    }
  }
  return { electron, window, calls }
}

module.exports = { loadMain, tempDir, fakeElectron, ROOT }
