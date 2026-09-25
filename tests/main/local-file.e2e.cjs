'use strict'
const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { buildApp, launchApp } = require('../support/electron-app.cjs')

test('Electron serves only library media and supports bounded byte ranges', async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'clipward-media-e2e-'))
  t.after(() => fs.rmSync(root, { recursive: true, force: true }))
  const userDataDir = path.join(root, 'user-data')
  const library = path.join(userDataDir, 'Clipward')
  fs.mkdirSync(library, { recursive: true })
  const inside = path.join(library, 'clip.mp4')
  const image = path.join(library, 'frame.png')
  const outside = path.join(root, 'outside.mp4')
  fs.writeFileSync(inside, '0123456789')
  fs.writeFileSync(image, Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jR9sAAAAASUVORK5CYII=', 'base64'))
  fs.writeFileSync(outside, 'private')
  const appDir = buildApp(path.join(root, 'app'))
  const session = await launchApp({ appDir, userDataDir })
  t.after(() => session.close())
  const request = (file, range) => session.app.evaluate(async ({ net }, { file, range }) => {
    const headers = range ? { Range: range } : {}
    const response = await net.fetch(`local-file://${encodeURIComponent(file)}`, { headers })
    return { status: response.status, body: await response.text(), range: response.headers.get('content-range') }
  }, { file, range })
  assert.deepEqual(await request(inside, 'bytes=2-5'), { status: 206, body: '2345', range: 'bytes 2-5/10' })
  assert.equal((await request(inside, 'bytes=11-12')).status, 416)
  assert.equal((await request(outside)).status, 403)
  assert.equal(await session.page.evaluate((file) => new Promise((resolve) => {
    const image = new Image()
    image.onload = () => resolve(true)
    image.onerror = () => resolve(false)
    image.src = `local-file://${encodeURIComponent(file)}`
  }), image), true)
})
