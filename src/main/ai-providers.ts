import { execFile } from 'child_process'
import { existsSync, statSync } from 'fs'
import { homedir } from 'os'
import { delimiter, join } from 'path'
import { promisify } from 'util'
import { loadSettings, getSettingsForBridge, providerKeyMissing } from './settings-store'
import { getBridgeRunnerPath, getEnginePath, resolvePythonPath, runtimeEnvironment } from './pipeline-runner'
import { AI_CLIS, type DetectedCli, type ProviderTestResult } from '../shared/ai-providers'

const execFileAsync = promisify(execFile)

/**
 * Directories a GUI-launched app misses: macOS apps start with a minimal PATH
 * that excludes Homebrew and per-user installers.
 */
function searchDirs(): string[] {
  const home = homedir()
  const extra = process.platform === 'win32'
    ? [join(process.env.APPDATA ?? '', 'npm'), join(home, '.local', 'bin'), join(home, '.bun', 'bin')]
    : [join(home, '.local', 'bin'), '/opt/homebrew/bin', '/usr/local/bin', join(home, '.npm-global', 'bin'),
        join(home, '.bun', 'bin'), join(home, '.opencode', 'bin'), join(home, '.cursor', 'bin'), join(home, '.grok', 'bin')]
  return [...new Set([...(process.env.PATH ?? '').split(delimiter), ...extra].filter(Boolean))]
}

// ponytail: Windows npm shims (.cmd) are found but Python can't exec them without a shell;
// native .exe installs work. Add a cmd.exe wrapper if Windows users need npm-installed CLIs.
const EXTENSIONS = process.platform === 'win32' ? ['.exe', '.cmd'] : ['']

function findBinary(names: readonly string[]): string | null {
  for (const dir of searchDirs()) {
    for (const name of names) {
      for (const ext of EXTENSIONS) {
        const candidate = join(dir, name + ext)
        try { if (statSync(candidate).isFile()) return candidate } catch { /* Not here. */ }
      }
    }
  }
  return null
}

export function detectClis(): DetectedCli[] {
  return AI_CLIS.map((cli) => ({ id: cli.id, label: cli.label, path: findBinary(cli.binaries) }))
}

export function resolveCliPath(provider: string): string | null {
  if (!provider.startsWith('cli:')) return null
  const cli = AI_CLIS.find((item) => `cli:${item.id}` === provider)
  return cli ? findBinary(cli.binaries) : null
}

/** Why the selected provider can't run a job, or null when it can. */
export function providerProblem(): string | null {
  const settings = loadSettings()
  if (settings.aiProvider.startsWith('cli:')) {
    return resolveCliPath(settings.aiProvider) ? null : 'The selected AI CLI is not installed. Choose another provider in Settings → AI provider.'
  }
  return providerKeyMissing(settings) ? 'Add an API key for the selected AI provider in Settings → AI provider.' : null
}

/**
 * One tiny structured-output request through the engine's own gateway, so the
 * test exercises exactly what a clipping job will run.
 */
export async function testProvider(): Promise<ProviderTestResult> {
  const problem = providerProblem()
  if (problem) return { ok: false, message: problem }
  const settings = loadSettings()
  const enginePath = getEnginePath()
  const script = join(getBridgeRunnerPath(), '..', 'provider_check.py')
  if (!existsSync(script)) return { ok: false, message: 'The provider check script is missing from this installation.' }
  const started = Date.now()
  try {
    const { stdout } = await execFileAsync(resolvePythonPath(enginePath, settings.pythonPath), [script], {
      cwd: enginePath,
      timeout: 180_000,
      maxBuffer: 1024 * 1024,
      env: {
        ...runtimeEnvironment(),
        ...getSettingsForBridge(settings),
        PYTHONPATH: enginePath,
        AI_PROVIDER: settings.aiProvider,
        AI_MODEL: settings.aiModel,
        AI_CLI_PATH: resolveCliPath(settings.aiProvider) ?? '',
        PATH: searchDirs().join(delimiter)
      },
      windowsHide: true
    })
    const result = JSON.parse(stdout.trim().split('\n').pop() ?? '{}') as { ok?: boolean; message?: string }
    const seconds = ((Date.now() - started) / 1000).toFixed(1)
    return result.ok
      ? { ok: true, message: `Connected in ${seconds}s${result.message ? ` · ${result.message}` : ''}` }
      : { ok: false, message: typeof result.message === 'string' ? result.message.slice(0, 300) : 'The provider did not answer.' }
  } catch {
    return { ok: false, message: 'The provider check did not finish. Make sure the CLI is logged in, or check the key.' }
  }
}

/** PATH for the engine, so CLIs installed per user can find their own helpers (node, etc.). */
export function providerPath(): string {
  return searchDirs().join(delimiter)
}
