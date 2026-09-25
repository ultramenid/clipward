'use strict'
const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { execFileSync, spawnSync } = require('node:child_process')
const yaml = require('js-yaml')
const workflow = yaml.load(fs.readFileSync(path.join(__dirname, '../.github/workflows/release.yml'), 'utf8'))
const resolver = workflow.jobs['resolve-release']
const validate = resolver.steps.find(step => step.id === 'validate').run
const resolve = resolver.steps.find(step => step.id === 'resolve').run

test('release jobs consume one immutable commit and explicit tag checkout', () => {
  const checkout = resolver.steps.find(step => step.uses?.startsWith('actions/checkout@'))
  assert.equal(checkout.with.ref, 'refs/tags/${{ steps.validate.outputs.tag }}')
  for (const name of ['build-macos', 'publish']) {
    const job = workflow.jobs[name]
    assert.ok([].concat(job.needs).includes('resolve-release'))
    assert.equal(job.steps.find(step => step.uses?.startsWith('actions/checkout@')).with.ref,
      '${{ needs.resolve-release.outputs.sha }}')
  }
})

test('tag validation rejects ref syntax and shell input', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'clipward-tag-validation-'))
  try {
    for (const tag of ['main', 'refs/heads/v1.2.3', 'v1.2.3\nsha=bad', 'v1.2.3;exit 0', 'v1.2.3^{commit}']) {
      const result = spawnSync('bash', ['-e', '-c', validate], {
        env: { ...process.env, TAG_NAME: tag, GITHUB_OUTPUT: path.join(dir, 'out') }
      })
      assert.notEqual(result.status, 0, tag)
    }
  } finally { fs.rmSync(dir, { recursive: true, force: true }) }
})

test('resolver handles annotated tags, branch collisions, missing tags and moved push events', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'clipward-release-ref-'))
  const git = (...args) => execFileSync('git', args, { cwd: dir, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim()
  try {
    git('init'); git('config', 'user.name', 'Local test'); git('config', 'user.email', 'test@example.invalid')
    git('commit', '--allow-empty', '-m', 'reviewed')
    const reviewed = git('rev-parse', 'HEAD')
    git('tag', '-a', 'v1.2.3', '-m', 'reviewed tag')
    git('checkout', '-b', 'v1.2.3')
    git('commit', '--allow-empty', '-m', 'unreviewed branch')
    const branch = git('rev-parse', 'HEAD')
    git('branch', 'v9.9.9', branch)
    git('checkout', '--detach', 'refs/tags/v1.2.3')
    const output = path.join(dir, 'output')
    const run = (tag, event = 'workflow_dispatch', sha = reviewed) => spawnSync('bash', ['-e', '-c', resolve], {
      cwd: dir, encoding: 'utf8', env: { ...process.env, TAG_NAME: tag, EVENT_NAME: event, EVENT_SHA: sha, GITHUB_OUTPUT: output }
    })
    assert.equal(run('v1.2.3').status, 0)
    assert.equal(fs.readFileSync(output, 'utf8').trim(), `sha=${reviewed}`)
    assert.equal(run('v1.2.3', 'push', git('rev-parse', 'refs/tags/v1.2.3')).status, 0,
      'annotated tag object is peeled to its commit')
    git('tag', 'v2.0.0', reviewed)
    assert.equal(run('v2.0.0').status, 0, 'lightweight tag resolves to its commit')
    assert.notEqual(run('v9.9.9').status, 0, 'a branch without a tag must fail')
    assert.notEqual(run('v1.2.3', 'push', branch).status, 0, 'moved push tag must fail')
    git('checkout', '--detach', branch)
    assert.notEqual(run('v1.2.3').status, 0, 'checkout of shadowing branch must fail')
  } finally { fs.rmSync(dir, { recursive: true, force: true }) }
})
