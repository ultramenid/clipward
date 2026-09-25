const assert = require('node:assert/strict')
const { test } = require('node:test')
const path = require('node:path')
const { buildSync } = require('esbuild')
const React = require('react')
const { renderToStaticMarkup } = require('react-dom/server')

const bundled = buildSync({
  stdin: {
    contents: `export { FormatStep, ClipsStep, BriefStep, JobForm, buildJobRequest, parseTrimRange, WIZARD_STEPS, wizardSteps } from './src/renderer/components/JobForm';
      export { parseClipTime, formatClipTime, formatClipLength, checkManualRow, checkManualClips, newManualRow } from './src/renderer/lib/manual-clips';
      export { setupBlockedReason } from './src/renderer/store/use-settings-store';
      export { ClipCard } from './src/renderer/components/ClipCard';
      export { isManualRun } from './src/shared/job-output';
      export { SetupCard } from './src/renderer/components/SetupCard';
      export { LocalAiSetup } from './src/renderer/components/LocalAiSetup';
      export { useSettingsStore, isProviderReady } from './src/renderer/store/use-settings-store';
      export { useDraftStore } from './src/renderer/store/use-draft-store';
      export { SourcePicker, isValidSourceLink } from './src/renderer/components/SourcePicker';
      export { framingProblem, sourceAnalysisNotice } from './src/renderer/components/ClipList';
      export { parseJobOutput } from './src/shared/job-output';
      export { twitchVodId, normalizeVideoSource } from './src/shared/video-source';`,
    resolveDir: path.resolve(__dirname, '..'),
    loader: 'ts'
  },
  bundle: true,
  platform: 'node',
  format: 'cjs',
  packages: 'external',
  define: { __APP_VERSION__: JSON.stringify(require('../package.json').version) },
  jsx: 'automatic',
  write: false
}).outputFiles[0].text

const form = { exports: {} }
new Function('module', 'exports', 'require', bundled)(form, form.exports, require)
const { FormatStep, JobForm, isValidSourceLink, parseTrimRange, framingProblem, sourceAnalysisNotice, parseJobOutput } = form.exports

test('setup asks for an AI provider and the offline Whisper model until both are ready', () => {
  const { SetupCard, isProviderReady } = form.exports
  // Server rendering reads the store's initial state: nothing is set up yet.
  const setup = renderToStaticMarkup(React.createElement(SetupCard, { onOpenSettings() {} }))
  assert.match(setup, /AI that picks the clips/)
  assert.match(setup, /Offline transcription/)
  assert.match(setup, /aria-label="Whisper model"/)

  const clis = [{ id: 'claude', label: 'Claude Code', path: '/usr/local/bin/claude' }, { id: 'codex', label: 'Codex', path: null }]
  const keys = { openrouterConfigured: false, anthropicConfigured: true, openaiConfigured: false }
  // A logged-in CLI needs no API key; one that isn't installed doesn't count.
  assert.equal(isProviderReady({ ...keys, clis, aiProvider: 'cli:claude' }), true)
  assert.equal(isProviderReady({ ...keys, clis, aiProvider: 'cli:codex' }), false)
  assert.equal(isProviderReady({ ...keys, clis: null, aiProvider: 'cli:claude' }), false)
  // An API provider is ready only with its own key.
  assert.equal(isProviderReady({ ...keys, clis, aiProvider: 'anthropic' }), true)
  assert.equal(isProviderReady({ ...keys, clis, aiProvider: 'openrouter' }), false)
  assert.equal(isProviderReady({ ...keys, clis, aiProvider: 'unknown' }), false)
  // Local AI needs a server URL and a model, but no key.
  const local = { ...keys, clis, aiProvider: 'local', localBaseUrl: 'http://localhost:11434/v1' }
  assert.equal(isProviderReady({ ...local, localModel: 'qwen2.5:14b' }), true)
  assert.equal(isProviderReady({ ...local, localModel: ' ' }), false)
  assert.equal(isProviderReady({ ...local, localBaseUrl: '', localModel: 'qwen2.5:14b' }), false)
})

test('setup offers Local AI, whose form asks for the server, model, images and an optional key', () => {
  const { SetupCard, LocalAiSetup } = form.exports
  const offered = renderToStaticMarkup(React.createElement(SetupCard, { onOpenSettings() {} }))
  assert.match(offered, /Local AI \(Ollama, LM Studio…\)/)
  assert.doesNotMatch(offered, /Server URL/)
  const keys = { drafts: { localApiKey: '' }, setDraft() {}, persist: async () => {}, remove: async () => {}, savedAt: null, error: null }
  const html = renderToStaticMarkup(React.createElement(LocalAiSetup, { keys }))
  assert.match(html, /Server URL/)
  assert.match(html, /value="http:\/\/localhost:11434\/v1"/)
  assert.match(html, /ollama pull qwen2\.5:14b/)
  assert.match(html, /Detect models/)
  assert.match(html, /Model accepts images/)
  assert.match(html, /role="switch" aria-checked="false"/)
  assert.match(html, /API key \(optional\)/)
})

test('source picker accepts full HTTP(S) links and rejects malformed or credentialed links', () => {
  assert.equal(isValidSourceLink(' https://www.youtube.com/watch?v=abc '), true)
  assert.equal(isValidSourceLink('http://example.com/video.mp4'), true)
  assert.equal(isValidSourceLink('https://'), false)
  assert.equal(isValidSourceLink('https://user:pass@example.com/video'), false)
  assert.equal(isValidSourceLink('file:///tmp/video.mp4'), false)
})

test('trim validation matches main process bounds, including an end at zero', () => {
  assert.equal(parseTrimRange(true, '', '0').error, 'End must be after the start.')
  assert.equal(parseTrimRange(true, '1:30', '90').error, 'End must be after the start.')
  assert.deepEqual(parseTrimRange(true, '', '1:30'), { start: null, end: 90, error: null })
  assert.deepEqual(parseTrimRange(false, 'oops', '0'), { start: null, end: null, error: null })
})

test('the wizard opens on the video step with the steps listed in order', () => {
  const html = renderToStaticMarkup(React.createElement(JobForm, { onSubmit() {} }))
  const labels = [...html.matchAll(/aria-label="Create steps">(.*?)<\/nav>/gs)][0]?.[1] ?? ''
  const order = ['Video', 'Brief', 'Format', 'Clips', 'Captions', 'Review'].map((label) => labels.indexOf(label))
  assert.ok(order.every((index, i) => index > -1 && (i === 0 || index > order[i - 1])))
  assert.match(html, /aria-current="step"[^>]*>.*?Video/s)
  assert.match(html, /Choose a video/)
})

test('the campaign brief travels with the job, trimmed, and stays for the next video', () => {
  const { useDraftStore, buildJobRequest, WIZARD_STEPS } = form.exports
  const original = useDraftStore.getState()
  try {
    assert.equal(WIZARD_STEPS[1].id, 'brief')
    original.update({ source: 'https://example.com/video', requirements: '  Mention Acme.\nNo swearing.\n' })
    assert.equal(buildJobRequest(useDraftStore.getState(), { start: null, end: null }).requirements, 'Mention Acme.\nNo swearing.')
    assert.equal('clippingMode' in buildJobRequest(useDraftStore.getState(), { start: null, end: null }), false)
    useDraftStore.getState().startAnother()
    assert.equal(useDraftStore.getState().source, '')
    assert.equal(useDraftStore.getState().requirements, '  Mention Acme.\nNo swearing.\n')
    const { BriefStep } = form.exports
    const html = renderToStaticMarkup(React.createElement(BriefStep, { draft: useDraftStore.getState(), update() {} }))
    assert.match(html, /aria-label="Campaign requirements"/)
    assert.match(html, /Mention Acme/)
    assert.match(html, /\/4000/)
  } finally { useDraftStore.setState(original) }
})

test('format and framing radio groups each expose one keyboard tab stop', () => {
  const draft = { aspectRatio: '9:16', layoutStyle: 'auto', layoutVision: true, pacing: 'tight' }
  const html = renderToStaticMarkup(React.createElement(FormatStep, { draft, update() {} }))
  for (const label of ['Format', 'Framing', 'Video speed']) {
    const group = html.match(new RegExp(`role="radiogroup" aria-label="${label}"[^>]*>(.*?)<\\/div>`, 's'))?.[1]
    assert.ok(group)
    assert.equal((group.match(/tabindex="0"/g) ?? []).length, 1)
  }
})

test('speed survives navigation and another job, and appears in the submitted request', () => {
  const { useDraftStore, buildJobRequest, ClipsStep } = form.exports
  const original = useDraftStore.getState()
  try {
    assert.equal(original.videoSpeed, 1)
    original.update({ source: 'https://example.com/video', videoSpeed: 1.5 })
    original.setStep('review')
    assert.equal(useDraftStore.getState().step, 'review')
    const lengths = renderToStaticMarkup(React.createElement(ClipsStep, { draft: useDraftStore.getState(), update() {} }))
    assert.match(lengths, /60 seconds becomes about 40 seconds/)
    assert.equal(buildJobRequest(useDraftStore.getState(), { start: 10, end: 70 }).videoSpeed, 1.5)
    original.startAnother()
    assert.equal(useDraftStore.getState().videoSpeed, 1.5)
    assert.equal(useDraftStore.getState().step, 'video')
  } finally { useDraftStore.setState(original) }
})

test('saved run speed is retained while invalid speed metadata is discarded', () => {
  assert.equal(parseJobOutput({ clips: [], metrics: { requested_settings: { video_speed: 1.5 } } }).metrics.requested_settings.video_speed, 1.5)
  for (const video_speed of ['2', null, Infinity, 0, 3]) {
    assert.equal(parseJobOutput({ clips: [], metrics: { requested_settings: { video_speed } } }).metrics.requested_settings.video_speed, undefined)
  }
})

test('clip list explains when smart framing intentionally keeps the whole frame', () => {
  const clip = (index) => ({
    clip_index: index, s3_url: `/tmp/clip-${index}.mp4`, duration_ms: 5000,
    start_time_ms: index * 5000, end_time_ms: (index + 1) * 5000, virality_score: 0.5
  })
  const output = parseJobOutput({
    clips: [clip(0), clip(1)],
    metrics: { clip_layouts: [
      { clip_index: 0, framing_status: 'whole_frame_auto' },
      { clip_index: 0, framing_status: 'whole_frame_auto' },
      { clip_index: 9, framing_status: 'whole_frame_auto' }
    ] }
  })
  assert.ok(output)
  assert.match(framingProblem(output, true), /whole frame for clip 1/)
  assert.equal(framingProblem(output, false), null)
  const classic = parseJobOutput({ clips: [clip(0)], metrics: {
    smart_framing_available: false, requested_settings: { aspect_ratio: '9:16', layout_style: 'fit' }
  } })
  assert.ok(classic)
  assert.equal(framingProblem(classic, true), null)
})

test('visual-only runs disclose unavailable captions and preserve analysis status', () => {
  const output = parseJobOutput({ clips: [], metrics: {
    transcription_status: 'no_speech', planning_source: 'visual', visual_frame_count: 12,
    captions_status: 'unavailable_without_transcript'
  } })
  assert.ok(output)
  assert.equal(output.metrics.visual_frame_count, 12)
  assert.equal(output.metrics.captions_status, 'unavailable_without_transcript')
  assert.match(sourceAnalysisNotice(output), /No speech was detected/)
})

test('Twitch VOD links canonicalize while other Twitch pages are rejected', () => {
  const { normalizeVideoSource, twitchVodId, SourcePicker } = form.exports
  for (const host of ['twitch.tv', 'www.twitch.tv', 'm.twitch.tv', 'go.twitch.tv']) {
    const source = `https://${host}/videos/12345/?t=1h&tracking=secret`
    assert.equal(isValidSourceLink(source), true)
    assert.equal(normalizeVideoSource(source), 'https://www.twitch.tv/videos/12345')
    assert.equal(twitchVodId(source), '12345')
  }
  for (const source of ['https://twitch.tv/channel', 'https://clips.twitch.tv/Clip', 'https://player.twitch.tv/?video=123', 'https://twitch.tv/videos/nope', 'https://twitch.tv:8443/videos/123']) assert.equal(isValidSourceLink(source), false)
  assert.equal(twitchVodId('https://twitch.tv.evil.test/videos/123'), null)
  const html = renderToStaticMarkup(React.createElement(SourcePicker, { value: 'https://www.twitch.tv/videos/12345', onChange() {} }))
  assert.match(html, /Twitch VOD/)
  assert.match(html, /Public, completed videos only/)
  assert.doesNotMatch(html, /<img/)
})

test('manual clip times accept seconds, m:ss and h:mm:ss', () => {
  const { parseClipTime, formatClipTime, formatClipLength } = form.exports
  assert.equal(parseClipTime('90'), 90)
  assert.equal(parseClipTime('75.5'), 75.5)
  assert.equal(parseClipTime('1:30'), 90)
  assert.equal(parseClipTime(' 1:02:03 '), 3723)
  assert.equal(parseClipTime(''), null)
  for (const bad of ['1:75', 'abc', '1:2:3:4', '-5']) assert.ok(Number.isNaN(parseClipTime(bad)), bad)
  assert.equal(formatClipTime(90), '1:30')
  assert.equal(formatClipTime(3723.4), '1:02:03.4')
  assert.equal(formatClipLength(45), '45s')
  assert.equal(formatClipLength(3.5), '3.5s')
  assert.equal(formatClipLength(90), '1m 30s')
  assert.equal(formatClipLength(900), '15m')
})

test('manual clips are checked inline against the shell limits and the known video length', () => {
  const { checkManualRow, checkManualClips, newManualRow } = form.exports
  const row = (start, end, title = '') => ({ id: 'r', start, end, title })
  assert.deepEqual(checkManualRow(row('0:10', '0:40'), null), { start: 10, end: 40, length: 30, error: null, field: null })
  assert.equal(checkManualRow(row('', '10'), null).error, 'Enter a start time.')
  assert.equal(checkManualRow(row('5', ''), null).error, 'Enter an end time.')
  assert.match(checkManualRow(row('1:99', '2:00'), null).error, /m:ss/)
  assert.equal(checkManualRow(row('40', '30'), null).error, 'End must be after the start.')
  assert.equal(checkManualRow(row('10', '12'), null).error, 'Clips must be at least 3 seconds.')
  assert.equal(checkManualRow(row('10', '13'), null).error, null)
  assert.equal(checkManualRow(row('0', '15:00'), null).error, null)
  assert.equal(checkManualRow(row('0', '15:01'), null).error, 'Clips can be at most 15 minutes.')
  assert.equal(checkManualRow(row('0', '10', 'x'.repeat(121)), null).field, 'title')
  assert.equal(checkManualRow(row('0', '10', 'two\nlines'), null).error, 'Titles must be a single line.')
  // A known video length catches clips past the end; unknown lengths wait for the engine.
  assert.equal(checkManualRow(row('1:50', '2:10'), 120).error, 'Ends after the video ends (2:00).')
  assert.equal(checkManualRow(row('2:00', '2:10'), 120).error, 'Starts after the video ends (2:00).')
  assert.equal(checkManualRow(row('1:50', '2:10'), null).error, null)
  // Overlaps are allowed; the list needs one to fifty clips.
  assert.equal(checkManualClips([row('0', '30'), row('10', '40')], 60).valid, true)
  assert.equal(checkManualClips([], null).listError, 'Add at least one clip.')
  const many = Array.from({ length: 51 }, () => newManualRow({ start: '0', end: '10' }))
  assert.equal(checkManualClips(many, null).listError, 'Add at most 50 clips.')
  assert.equal(checkManualClips(many.slice(0, 50), null).valid, true)
  assert.equal(new Set(many.map((r) => r.id)).size, 51)
})

test('manual mode sends the typed clips and drops the brief, trim, counts and AI vision', () => {
  const { useDraftStore, buildJobRequest, wizardSteps, newManualRow } = form.exports
  const original = useDraftStore.getState()
  try {
    original.update({
      source: 'https://example.com/video', requirements: 'Mention Acme', trimOpen: true, trimStart: '10', trimEnd: '20',
      autoClipCount: false, maxClips: 3, layoutVision: true, clipMode: 'manual',
      manualClips: [newManualRow({ start: '1:00', end: '1:30', title: '  Hook ' }), newManualRow({ start: '5', end: '20' })]
    })
    const request = buildJobRequest(useDraftStore.getState(), { start: 10, end: 20 })
    assert.deepEqual(request.manualClips, [
      { startSeconds: 60, endSeconds: 90, title: 'Hook' },
      { startSeconds: 5, endSeconds: 20, title: null }
    ])
    assert.equal(request.requirements, '')
    assert.equal(request.startTimeSeconds, null)
    assert.equal(request.endTimeSeconds, null)
    assert.equal(request.maxClips, null)
    assert.equal(request.layoutVision, false)
    assert.deepEqual(wizardSteps('manual').map((s) => s.id), ['video', 'format', 'clips', 'captions', 'review'])
    // AI mode keeps the trim and sends no manual clips.
    useDraftStore.getState().update({ clipMode: 'ai' })
    const ai = buildJobRequest(useDraftStore.getState(), { start: 10, end: 20 })
    assert.equal('manualClips' in ai, false)
    assert.equal(ai.startTimeSeconds, 10)
    assert.equal(wizardSteps('ai').length, 6)
    // The mode survives the next video; the typed moments belong to the last one.
    useDraftStore.getState().update({ clipMode: 'manual' })
    useDraftStore.getState().startAnother()
    assert.equal(useDraftStore.getState().clipMode, 'manual')
    assert.equal(useDraftStore.getState().manualClips.length, 1)
    assert.equal(useDraftStore.getState().manualClips[0].start, '')
  } finally { useDraftStore.setState(original) }
})

test('the Clips step offers both modes and shows an editable list in manual mode', () => {
  const { useDraftStore, ClipsStep, newManualRow } = form.exports
  const original = useDraftStore.getState()
  try {
    const ai = renderToStaticMarkup(React.createElement(ClipsStep, { draft: useDraftStore.getState(), update() {} }))
    assert.match(ai, /role="radiogroup" aria-label="How clips are chosen"/)
    assert.match(ai, /AI picks the moments/)
    assert.match(ai, /Clip length/)
    original.update({ clipMode: 'manual', manualClips: [newManualRow({ start: '0:10', end: '0:40', title: 'Intro' }), newManualRow({ start: '50', end: '45' })] })
    const html = renderToStaticMarkup(React.createElement(ClipsStep, { draft: useDraftStore.getState(), update() {} }))
    assert.doesNotMatch(html, /Clip length options/)
    assert.doesNotMatch(html, /Let AI decide/)
    assert.match(html, /aria-label="Clip 1 start"/)
    assert.match(html, /aria-label="Clip 2 end" aria-invalid="true"/)
    assert.match(html, /aria-label="Duplicate clip 1"/)
    assert.match(html, /aria-label="Remove clip 2"/)
    assert.match(html, />30s</)
    assert.match(html, /End must be after the start\./)
    assert.match(html, /checked against the video once it downloads/)
  } finally { useDraftStore.setState(original) }
})

test('manual mode starts without an AI provider but still needs Whisper and the tools', () => {
  const { setupBlockedReason, SetupCard } = form.exports
  const noProvider = { ready: false, providerReady: false, whisperReady: true, toolsOk: true, manualReady: true }
  assert.equal(setupBlockedReason(noProvider, 'ai'), 'Choose an AI provider above to start.')
  assert.equal(setupBlockedReason(noProvider, 'manual'), undefined)
  assert.equal(setupBlockedReason({ ...noProvider, whisperReady: false, manualReady: false }, 'manual'), 'Download the Whisper model above to start.')
  assert.equal(setupBlockedReason({ ...noProvider, toolsOk: false, manualReady: false }, 'manual'), 'Fix the missing tools in Settings to start.')
  assert.equal(setupBlockedReason({ ...noProvider, toolsOk: null, manualReady: false }, 'manual'), 'Checking required tools…')
  assert.equal(setupBlockedReason({ ...noProvider, ready: true, providerReady: true }, 'ai'), undefined)
  // Server rendering reads the stores' initial state (AI mode): the card points to manual mode.
  const setup = renderToStaticMarkup(React.createElement(SetupCard, { onOpenSettings() {} }))
  assert.match(setup, /I’ll choose the moments/)
})

test('manual runs keep their planning source and hide scores and the top pick', () => {
  const { isManualRun, ClipCard } = form.exports
  const clip = { clip_index: 0, s3_url: '/tmp/clip-0.mp4', duration_ms: 5000, start_time_ms: 0, end_time_ms: 5000, virality_score: 0 }
  const output = parseJobOutput({ clips: [clip], metrics: { planning_source: 'manual', requested_settings: { ai_provider: 'none' } } })
  assert.ok(output)
  assert.equal(output.metrics.planning_source, 'manual')
  assert.equal(output.metrics.requested_settings.ai_provider, 'none')
  assert.equal(isManualRun(output), true)
  assert.equal(isManualRun(parseJobOutput({ clips: [], metrics: { planning_source: 'transcript' } })), false)
  assert.equal(parseJobOutput({ clips: [], metrics: { planning_source: 'other' } }).metrics.planning_source, undefined)
  const props = { clip: output.clips[0], vertical: true, selected: false, selecting: false, onToggleSelect() {} }
  assert.match(renderToStaticMarkup(React.createElement(ClipCard, props)), /title="Virality score"/)
  const manual = renderToStaticMarkup(React.createElement(ClipCard, { ...props, showScore: false, topPick: false }))
  assert.doesNotMatch(manual, /Virality score/)
  assert.doesNotMatch(manual, /Top pick/)
})
