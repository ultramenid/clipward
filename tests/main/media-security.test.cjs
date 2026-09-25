'use strict'
const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const { execFileSync } = require('node:child_process')
const { loadMain, tempDir, fakeElectron, ROOT } = require('../support/load-main.cjs')
const FFMPEG = fs.existsSync(path.join(ROOT, 'engine-bin/ffmpeg')) ? path.join(ROOT, 'engine-bin/ffmpeg') : 'ffmpeg'

function clip(file) {
  execFileSync(FFMPEG, ['-v', 'error', '-f', 'lavfi', '-i', 'color=s=64x64:d=0.3', '-f', 'lavfi', '-i', 'sine=duration=0.3', '-shortest', '-c:v', 'mpeg4', '-c:a', 'aac', file])
}

test('a playlist disguised as video cannot generate a thumbnail from sibling media', async () => {
  const { dir, cleanup } = tempDir()
  try {
    clip(path.join(dir, 'private.mp4'))
    const disguised = path.join(dir, 'playlist.mp4')
    fs.writeFileSync(disguised, "ffconcat version 1.0\nfile 'private.mp4'\n")
    const { generateThumbnail } = loadMain("export { generateThumbnail } from './src/main/file-manager'", { electron: fakeElectron(dir).electron })
    assert.equal(await generateThumbnail(disguised), null)
  } finally { cleanup() }
})

function whisper(dir, fetch) {
  const { electron } = fakeElectron(dir)
  return loadMain("export * from './src/main/whisper-models'", { electron: { ...electron, net: { fetch } }, './logger': { logger: { info() {}, warn() {} } } })
}

test('a Whisper download is kept only when complete, and unknown models are rejected', async () => {
  const { dir, cleanup } = tempDir()
  try {
    const body = (bytes) => new ReadableStream({ start(controller) { controller.enqueue(new Uint8Array(bytes)); controller.close() } })
    // The server promises 10 bytes and sends 4: the model must not appear downloaded.
    const truncated = whisper(dir, async () => new Response(body(4), { headers: { 'content-length': '10' } }))
    await assert.rejects(truncated.downloadWhisperModel('base', () => {}), /could not be downloaded/)
    assert.equal(truncated.downloadedWhisperModel('base'), null)
    assert.deepEqual(fs.readdirSync(path.join(dir, 'userData', 'whisper-models')), [])

    const requested = []
    const complete = whisper(dir, async (url) => { requested.push(url); return new Response(body(10), { headers: { 'content-length': '10' } }) })
    const progress = []
    await complete.downloadWhisperModel('base', (status) => progress.push(status))
    assert.deepEqual(requested, ['https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin'])
    assert.equal(complete.downloadedWhisperModel('base'), path.join(dir, 'userData', 'whisper-models', 'ggml-base.bin'))
    assert.equal(progress.at(-1).downloaded, true)
    assert.equal(complete.listWhisperModels().find((model) => model.id === 'base').downloaded, true)

    for (const id of ['../../etc/passwd', 'large', null]) {
      await assert.rejects(complete.downloadWhisperModel(id, () => {}), /Unknown Whisper model/)
      assert.throws(() => complete.deleteWhisperModel(id), /Unknown Whisper model/)
    }
    complete.deleteWhisperModel('base')
    assert.equal(complete.downloadedWhisperModel('base'), null)
  } finally { cleanup() }
})

test('packaged tools never fall back to PATH or a user-selected Python', () => {
  const { dir, cleanup } = tempDir()
  const previous = Object.getOwnPropertyDescriptor(process, 'resourcesPath')
  Object.defineProperty(process, 'resourcesPath', { value: dir, configurable: true })
  try {
    const api = loadMain("export { resolveBinary } from './src/main/tools'; export { resolvePythonPath, preflightCheck } from './src/main/pipeline-runner'", { electron: fakeElectron(dir, { isPackaged: true }).electron })
    for (const name of ['ffmpeg', 'ffprobe', 'yt-dlp']) {
      assert.equal(api.resolveBinary(name), path.join(dir, 'engine-bin', name + (process.platform === 'win32' ? '.exe' : '')))
    }
    assert.equal(api.resolvePythonPath(dir, '/untrusted/python'), path.join(dir, 'engine-venv', ...(process.platform === 'win32' ? ['Scripts', 'python.exe'] : ['bin', 'python3'])))
    const check = api.preflightCheck({ pythonPath: '/untrusted/python', bridgePath: dir, enginePath: dir })
    assert.equal(check.ok, false)
    assert.match(check.error, /Bundled ffmpeg is missing/)
  } finally {
    if (previous) Object.defineProperty(process, 'resourcesPath', previous)
    else delete process.resourcesPath
    cleanup()
  }
})
