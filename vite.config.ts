// Vite config for the Tauri app's renderer (src/renderer → dist/).
// `npm run dev` (tauri dev) starts this dev server through beforeDevCommand;
// `npm run build` (tauri build) runs `vite build` first. electron.vite.config.ts
// remains only for the legacy Electron shell until it is removed.
import { readFileSync, readdirSync, statSync } from 'fs'
import { resolve } from 'path'
import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'

const { version } = JSON.parse(readFileSync(resolve(__dirname, 'package.json'), 'utf-8')) as { version: string }

/** Renderer dependencies are bundled from devDependencies, outside npm's runtime inventory. */
function bundledLicenseNotices(): Plugin {
  return {
    name: 'bundled-license-notices',
    apply: 'build',
    generateBundle() {
      const directories = new Set<string>()
      for (const id of this.getModuleIds()) {
        const normalized = id.replace(/\\/g, '/')
        const marker = '/node_modules/'
        const at = normalized.lastIndexOf(marker)
        if (at < 0 || normalized.startsWith('\0')) continue
        const parts = normalized.slice(at + marker.length).split('/')
        const name = parts.slice(0, parts[0].startsWith('@') ? 2 : 1).join('/')
        directories.add(`${normalized.slice(0, at)}${marker}${name}`)
      }
      const notices: string[] = []
      for (const directory of [...directories].sort()) {
        const pkg = JSON.parse(readFileSync(resolve(directory, 'package.json'), 'utf8'))
        const files = readdirSync(directory).filter((name) => /^(licen[sc]e|copying|notice)([._-]|$)/i.test(name) && statSync(resolve(directory, name)).isFile()).sort()
        if (!files.length) this.error(`Missing bundled license text: ${pkg.name}@${pkg.version}`)
        notices.push(`${pkg.name}@${pkg.version}\nDeclared license: ${pkg.license ?? 'See notice'}\n\n${files.map((name) => readFileSync(resolve(directory, name), 'utf8')).join('\n\n')}`)
      }
      this.emitFile({ type: 'asset', fileName: 'THIRD_PARTY_LICENSES.txt', source: notices.join('\n\n----------------------------------------\n\n') })
    }
  }
}

const host = process.env.TAURI_DEV_HOST

export default defineConfig({
  root: resolve(__dirname, 'src/renderer'),
  // Relative asset URLs work from the bundled tauri:// origin.
  base: './',
  clearScreen: false,
  plugins: [react(), bundledLicenseNotices()],
  define: {
    __APP_VERSION__: JSON.stringify(version)
  },
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src/renderer')
    }
  },
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**', '**/crates/**', '**/target/**'] }
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    outDir: resolve(__dirname, 'dist'),
    emptyOutDir: true,
    // Tauri's webviews: WebView2 (Chromium) on Windows, WebKit on macOS and Linux.
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari15',
    minify: process.env.TAURI_ENV_DEBUG ? false : 'esbuild',
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
    rollupOptions: {
      input: { index: resolve(__dirname, 'src/renderer/index.html') }
    }
  }
})
