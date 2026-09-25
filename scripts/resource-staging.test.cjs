'use strict'
const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawnSync } = require('node:child_process')

test('resource staging preserves existing tools when the in-repo engine is incomplete', { skip: process.platform !== 'darwin' }, () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'clipward-stage-'))
  try {
    const scripts = path.join(dir, 'scripts')
    fs.mkdirSync(scripts)
    fs.copyFileSync(path.join(__dirname, 'prepare-resources.sh'), path.join(scripts, 'prepare-resources.sh'))
    fs.mkdirSync(path.join(dir, 'engine-bin'))
    const sentinel = path.join(dir, 'engine-bin', 'keep')
    fs.writeFileSync(sentinel, 'keep')
    const result = spawnSync('bash', [path.join(scripts, 'prepare-resources.sh'), process.arch === 'arm64' ? 'arm64' : 'x64'], { encoding: 'utf8' })
    assert.notEqual(result.status, 0)
    assert.match(result.stderr, /in-repo Clipward clipping engine is incomplete/)
    assert.equal(fs.readFileSync(sentinel, 'utf8'), 'keep')
  } finally { fs.rmSync(dir, { recursive: true, force: true }) }
})
