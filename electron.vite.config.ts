import { readFileSync, readdirSync, statSync } from 'fs'
import { resolve } from 'path'
import { defineConfig, externalizeDepsPlugin } from 'electron-vite'
import react from '@vitejs/plugin-react'
import type { Plugin } from 'vite'

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
        const files = readdirSync(directory).filter((name) => /^(licen[sc]e|copying|notice)([.-]|$)/i.test(name) && statSync(resolve(directory, name)).isFile()).sort()
        if (!files.length) this.error(`Missing bundled license text: ${pkg.name}@${pkg.version}`)
        notices.push(`${pkg.name}@${pkg.version}\nDeclared license: ${pkg.license ?? 'See notice'}\n\n${files.map((name) => readFileSync(resolve(directory, name), 'utf8')).join('\n\n')}`)
      }
      this.emitFile({ type: 'asset', fileName: 'THIRD_PARTY_LICENSES.txt', source: notices.join('\n\n----------------------------------------\n\n') })
    }
  }
}

export default defineConfig({
  main: {
    plugins: [externalizeDepsPlugin()],
    build: {
      rollupOptions: {
        input: {
          index: resolve(__dirname, 'src/main/index.ts')
        }
      }
    }
  },
  preload: {
    plugins: [externalizeDepsPlugin()],
    build: {
      rollupOptions: {
        input: {
          index: resolve(__dirname, 'src/preload/index.ts')
        }
      }
    }
  },
  renderer: {
    root: resolve(__dirname, 'src/renderer'),
    build: {
      rollupOptions: {
        input: {
          index: resolve(__dirname, 'src/renderer/index.html')
        }
      }
    },
    plugins: [
      react(),
      bundledLicenseNotices(),
      {
        name: 'development-content-security-policy',
        apply: 'serve',
        transformIndexHtml(html) {
          // React Refresh injects an inline bootstrap; HMR uses a local socket.
          // This transform never runs in packaged builds.
          return html
            .replace("script-src 'self'", "script-src 'self' 'unsafe-inline'")
            .replace("connect-src 'none'", "connect-src 'self' ws://localhost:* ws://127.0.0.1:*")
        }
      }
    ],
    define: {
      __APP_VERSION__: JSON.stringify(version)
    },
    resolve: {
      alias: {
        '@': resolve(__dirname, 'src/renderer')
      }
    }
  }
})
