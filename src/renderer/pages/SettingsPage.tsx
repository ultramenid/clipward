import { useEffect, useRef, useState, type ReactNode } from 'react'
import { AppWindow, ArrowUpRight, AudioLines, BookA, Check, ChevronDown, Cpu, FolderOpen, Github, Info, Loader2, RefreshCw, RotateCcw, ScrollText, Sparkles } from 'lucide-react'
import { useSettingsStore, useSetupState } from '../store/use-settings-store'
import { getApi } from '../lib/ipc'
import { cn, errorMessage, MOD_KEY } from '../lib/utils'
import { APP_NAME, APP_VERSION, CLIPWARD_URL, ISSUES_URL, LICENSE_NAME, REPO_URL } from '../config/brand'
import { providerLabel } from '../../shared/ai-providers'
import { WHISPER_MODELS } from '../../shared/whisper-models'
import type { ClipSettings, ToolStatus } from '../../preload/index'
import { ProviderSetup } from '../components/ProviderSetup'
import { WhisperModels } from '../components/WhisperModels'
import { ClipwardLogo } from '../components/brand/ClipwardLogo'
import { Page } from '../components/ui/Page'
import { PageHeader } from '../components/ui/PageHeader'
import { Panel, PanelHeader } from '../components/ui/Panel'
import { Button } from '../components/ui/Button'
import { Field, TextArea, TextInput } from '../components/ui/Field'
import { Badge, StatusDot } from '../components/ui/Badge'
import { IconTile } from '../components/ui/IconTile'
import { Callout } from '../components/ui/Callout'
import { SettingRow } from '../components/ui/SettingRow'
import { Switch } from '../components/ui/Switch'
import { Segmented } from '../components/ui/Segmented'
import { Slider } from '../components/ui/Slider'
import { ColorPicker, ColorSwatchRow } from '../components/ui/ColorSwatches'
import { glassActive, useReducedTransparency } from '../lib/glass'
import { useColorScheme } from '../lib/appearance'
import { LAYER_CLASS } from '../lib/layers'
import { SIDEBAR_SHORTCUT_KEY } from '../components/Sidebar'
import {
  ACCENT_COLOR_LABELS,
  ACCENT_COLOR_PRESETS,
  APPEARANCE_DEFAULTS,
  SIDEBAR_BLUR_MAX,
  SIDEBAR_BLUR_MIN,
  SIDEBAR_OPACITY_MAX,
  SIDEBAR_OPACITY_MIN,
  THEME_DARK_LIGHTNESS_MAX,
  THEME_DARK_LIGHTNESS_MIN,
  THEME_HUE_MAX,
  THEME_HUE_MIN,
  THEME_SATURATION_MAX,
  THEME_SATURATION_MIN,
  UI_SCALE_MAX,
  UI_SCALE_MIN,
  type Appearance,
  type ThemePreference
} from '../../shared/appearance'

type SectionId = 'ai' | 'transcription' | 'vocabulary' | 'output' | 'appearance' | 'system' | 'about'
type SectionTone = 'success' | 'warning' | 'danger' | 'idle'

export function SettingsPage(): React.JSX.Element {
  const { outputDirectory, pythonPath, customVocabulary, aiProvider, whisperModel, saving, save, toolStatus, toolError, checkTools, checkingTools } =
    useSettingsStore()
  const { providerReady, whisperReady } = useSetupState()
  const [isPackaged, setIsPackaged] = useState(true)
  const [savedAt, setSavedAt] = useState<number | null>(null)
  const [saveError, setSaveError] = useState<string | null>(null)

  useEffect(() => {
    getApi().system.isPackaged().then(setIsPackaged).catch(() => {})
  }, [])

  const commit = async (patch: Partial<ClipSettings>, recheck = false): Promise<void> => {
    try {
      await save(patch)
      setSaveError(null)
      setSavedAt(Date.now())
      if (recheck) checkTools()
    } catch (err) {
      setSaveError(errorMessage(err, 'Could not save settings'))
    }
  }

  const lastSaved = savedAt ?? 0
  const error = saveError
  const tools = toolRows(toolStatus)
  const toolsChecked = tools.every((row) => row.ok != null)
  const toolsMissing = tools.filter((row) => !row.optional && row.ok === false).length
  const vocabularyTerms = customVocabulary.split('\n').filter((line) => line.trim()).length

  const sections: { id: SectionId; label: string; icon: ReactNode; tone: SectionTone }[] = [
    { id: 'ai', label: 'AI provider', icon: <Sparkles />, tone: providerReady ? 'success' : 'warning' },
    { id: 'transcription', label: 'Transcription', icon: <AudioLines />, tone: whisperReady ? 'success' : 'warning' },
    { id: 'vocabulary', label: 'Vocabulary', icon: <BookA />, tone: 'idle' },
    { id: 'output', label: 'Output', icon: <FolderOpen />, tone: 'idle' },
    { id: 'appearance', label: 'Appearance', icon: <AppWindow />, tone: 'idle' },
    { id: 'system', label: 'System check', icon: <Cpu />, tone: !toolsChecked ? 'idle' : toolsMissing ? 'danger' : 'success' },
    { id: 'about', label: 'About', icon: <Info />, tone: 'idle' }
  ]
  const [active, jump] = useActiveSection(sections.map((section) => section.id))

  const checks: { label: string; ok: boolean | null; detail: string; section: SectionId; optional?: boolean; tone?: 'danger' }[] = [
    { label: 'AI provider', ok: providerReady, detail: providerReady ? providerLabel(aiProvider) : 'Pick a CLI or add a key', section: 'ai' },
    { label: 'Whisper', ok: whisperReady, detail: whisperReady ? WHISPER_MODELS.find((m) => m.id === whisperModel)?.label ?? whisperModel : 'Download a model', section: 'transcription' },
    { label: 'Tools', ok: toolsChecked ? toolsMissing === 0 : null, detail: !toolsChecked ? (checkingTools ? 'Checking…' : 'Not checked') : toolsMissing ? `${toolsMissing} missing` : 'All installed', section: 'system', tone: 'danger' },
  ]
  const blocking = checks.filter((check) => !check.optional && check.ok === false).length

  return (
    <Page width="default" className="max-w-[1080px]">
      <PageHeader
        title="Settings"
        description="Changes save automatically."
        actions={<SaveIndicator saving={saving} savedAt={lastSaved || null} error={error} />}
      />

      <div className="mt-5 grid items-start gap-6 lg:grid-cols-[176px_minmax(0,1fr)]">
        <nav aria-label="Settings sections" className="hidden flex-col gap-px lg:sticky lg:top-6 lg:flex">
          <p aria-hidden className="px-2 pb-1 text-caption font-semibold text-ink-tertiary">Sections</p>
          {sections.map((section) => (
            <button
              key={section.id}
              type="button"
              onClick={() => jump(section.id)}
              aria-current={active === section.id ? 'true' : undefined}
              className={cn(
                'flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-label transition-colors duration-feedback [&_svg]:h-4 [&_svg]:w-4 [&_svg]:shrink-0 [&_svg]:opacity-70',
                active === section.id ? 'bg-selection text-content' : 'text-ink-secondary hover:bg-content/5 hover:text-content'
              )}
            >
              {section.icon}
              <span className="min-w-0 flex-1 truncate">{section.label}</span>
              {section.tone !== 'idle' && <StatusDot tone={section.tone} />}
            </button>
          ))}
        </nav>

        <div className="min-w-0 space-y-4">
          <Panel>
            <div className="flex items-center justify-between gap-3">
              <div className="flex items-center gap-3">
                <IconTile tone={blocking ? 'warning' : 'success'} size="md">{blocking ? <Sparkles /> : <Check strokeWidth={3} />}</IconTile>
                <div>
                  <h2 className="text-section text-ink">{blocking ? `${blocking} thing${blocking === 1 ? '' : 's'} to set up before clipping` : 'Ready to clip'}</h2>
                  <p className="mt-0.5 text-caption text-ink-secondary">{APP_NAME} runs on this computer. Whisper transcribes offline; your CLI plan or API key picks the clips.</p>
                </div>
              </div>
            </div>
            <div className="mt-3 grid grid-cols-2 gap-2 sm:grid-cols-3">
              {checks.map((check) => (
                <button
                  key={check.label}
                  type="button"
                  onClick={() => jump(check.section)}
                  className="tile tile-hover flex items-start gap-2.5 rounded-lg px-3 py-2 text-left"
                >
                  <StatusDot className="mt-1.5" tone={check.ok == null || check.optional && !check.ok ? 'idle' : check.ok ? 'success' : check.tone ?? 'warning'} />
                  <span className="min-w-0">
                    <span className="block text-label text-ink">{check.label}</span>
                    <span className={cn('block truncate text-meta', check.ok === false && !check.optional ? (check.tone === 'danger' ? 'text-danger' : 'text-warning') : 'text-ink-secondary')}>{check.detail}</span>
                  </span>
                </button>
              ))}
            </div>
          </Panel>

          <Section id="ai">
            <PanelHeader
              icon={<IconTile tone="accent" size="sm"><Sparkles /></IconTile>}
              title="AI provider"
              description="Reads the transcript and picks the clips that fit your brief. Use a CLI you're already logged in to, or your own API key."
            />
            <div className="mt-4"><ProviderSetup /></div>
          </Section>

          <Section id="transcription">
            <PanelHeader
              icon={<IconTile size="sm"><AudioLines /></IconTile>}
              title="Transcription"
              description="Whisper runs on this computer with whisper.cpp; audio never leaves it. Larger models are more accurate and slower."
            />
            <div className="mt-4"><WhisperModels /></div>
          </Section>

          <Section id="vocabulary">
            <PanelHeader
              icon={<IconTile size="sm"><BookA /></IconTile>}
              title="Custom vocabulary"
              description="Names, products and jargon Whisper should spell exactly, one per line. Put the campaign's brand and creator names here."
              action={vocabularyTerms > 0 && <Badge className="tabular-nums">{vocabularyTerms} term{vocabularyTerms === 1 ? '' : 's'}</Badge>}
            />
            <VocabularyField value={customVocabulary} onCommit={(value) => commit({ customVocabulary: value })} />
          </Section>

          <Section id="output">
            <PanelHeader
              icon={<IconTile size="sm"><FolderOpen /></IconTile>}
              title="Output"
              description="Every run gets its own folder of clips, transcript and plan."
            />
            <OutputFolder value={outputDirectory} onCommit={(dir) => commit({ outputDirectory: dir })} />
          </Section>

          <Section id="appearance">
            <AppearanceSettings onError={setSaveError} onSaved={() => { setSaveError(null); setSavedAt(Date.now()) }} />
          </Section>

          <Section id="system">
            <PanelHeader
              icon={<IconTile size="sm" tone={toolsChecked && toolsMissing ? 'danger' : 'neutral'}><Cpu /></IconTile>}
              title="System check"
              description="Tools Clipward needs to download, transcribe and cut video."
              action={
                <Button
                  size="sm"
                  onClick={checkTools}
                  disabled={checkingTools}
                  icon={<RefreshCw className={cn('h-3.5 w-3.5', checkingTools && 'animate-spin')} />}
                >
                  Re-check
                </Button>
              }
            />
            <ToolList rows={tools} checking={checkingTools} />
            {toolError && <Callout tone="danger" className="mt-3">{toolError}</Callout>}
            {!isPackaged && (
              <div className="mt-4 grid gap-4 border-t border-stroke pt-4">
                <DevPathField
                  label="Python path"
                  value={pythonPath}
                  placeholder="python3"
                  onCommit={(v) => commit({ pythonPath: v }, true)}
                />
              </div>
            )}
          </Section>

          <Section id="about" className="overflow-hidden">
            <div className="flex flex-wrap items-center gap-4">
              <ClipwardLogo variant="icon" className="-m-1 h-10" />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2.5">
                  <h2 className="text-heading text-ink">{APP_NAME}</h2>
                  <Badge className="font-mono tabular-nums">v{APP_VERSION}</Badge>
                </div>
                <p className="mt-1 text-caption text-ink-secondary">
                  Released under the {LICENSE_NAME} license.{' '}
                  <button
                    type="button"
                    onClick={() => getApi().shell.openPath(CLIPWARD_URL)}
                    className="text-accent-fg underline decoration-accent/30 underline-offset-2 transition-colors hover:decoration-accent-fg"
                  >
                    clipward.ai
                  </button>
                </p>
              </div>
              <div className="flex flex-wrap gap-1.5">
                <Button size="sm" icon={<Github className="h-3.5 w-3.5" />} onClick={() => getApi().shell.openPath(REPO_URL)}>
                  GitHub
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  trailingIcon={<ArrowUpRight className="h-3.5 w-3.5" />}
                  onClick={() => getApi().shell.openPath(ISSUES_URL)}
                >
                  Report an issue
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<ScrollText className="h-3.5 w-3.5" />}
                  onClick={() => getApi().diagnostics.openLogFolder()}
                >
                  Show logs
                </Button>
              </div>
            </div>
          </Section>
        </div>
      </div>
    </Page>
  )
}

function Section({ id, className, children }: { id: SectionId; className?: string; children: ReactNode }): React.JSX.Element {
  return (
    <section id={`settings-${id}`} className="scroll-mt-6">
      <Panel className={className}>{children}</Panel>
    </section>
  )
}

/** How far below the top of the scroll area a section counts as reached. */
const ACTIVE_OFFSET = 48

/** The section nearest the top of the scroll area, for the side nav, and a jump that selects it. */
function useActiveSection(ids: SectionId[]): [SectionId, (id: SectionId) => void] {
  const [active, setActive] = useState<SectionId>(ids[0])
  // A jumped-to section stays selected through its smooth scroll, even if it
  // can't reach the top, until the user scrolls on their own.
  const pinned = useRef(false)
  const key = ids.join(',')
  useEffect(() => {
    const root = document.getElementById('page-scroll')
    if (!root) return
    const update = (): void => {
      if (pinned.current) return
      // Sections near the end can't scroll up to the offset line, so over the
      // last stretch the line slides down to the bottom edge. Every section
      // takes its turn, and the last is selected at the bottom.
      const max = root.scrollHeight - root.clientHeight
      const tail = Math.min(max, root.clientHeight - ACTIVE_OFFSET)
      const progress = tail > 0 ? Math.min(1, Math.max(0, root.scrollTop - (max - tail)) / tail) : 0
      const line = root.getBoundingClientRect().top + ACTIVE_OFFSET + progress * (root.clientHeight - ACTIVE_OFFSET)
      let current = ids[0]
      for (const id of ids) {
        const el = document.getElementById(`settings-${id}`)
        if (el && el.getBoundingClientRect().top <= line) current = id
      }
      setActive(current)
    }
    const release = (): void => { pinned.current = false }
    const inputs = ['wheel', 'touchstart', 'pointerdown', 'keydown'] as const
    update()
    root.addEventListener('scroll', update, { passive: true })
    for (const type of inputs) window.addEventListener(type, release, { passive: true })
    return () => {
      root.removeEventListener('scroll', update)
      for (const type of inputs) window.removeEventListener(type, release)
    }
  }, [key])
  const jump = (id: SectionId): void => {
    pinned.current = true
    setActive(id)
    document.getElementById(`settings-${id}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }
  return [active, jump]
}

interface SaveCallbacks {
  onSaved: () => void
  onError: (message: string) => void
}

/**
 * Settings → Appearance, ported from MonoCode's appearance page (MIT,
 * Copyright (c) 2026 Nick; see THIRD_PARTY_NOTICES.md): theme and accent,
 * the tint of the neutral chrome, window glass strength, and layout. Every
 * change shows at once and saves in the background.
 */
function AppearanceSettings({ onSaved, onError }: SaveCallbacks): React.JSX.Element {
  const appearance = useSettingsStore((s) => s.appearance)
  const setAppearance = useSettingsStore((s) => s.setAppearance)
  const glassSupported = useSettingsStore((s) => s.windowGlassSupported)
  const glassEnabled = useSettingsStore((s) => s.windowGlass)
  const blurSupported = useSettingsStore((s) => s.windowBlurSupported)
  const setWindowGlass = useSettingsStore((s) => s.setWindowGlass)
  const reduced = useReducedTransparency()
  const light = useColorScheme() === 'light'
  const glassOn = glassActive(glassSupported, glassEnabled, reduced)
  const glassOff = !glassOn || light

  const change = (patch: Partial<Appearance>): void => {
    setAppearance(patch).then(onSaved, (err) => onError(errorMessage(err, 'Could not save appearance settings')))
  }

  const reset = (): void => {
    change({ ...APPEARANCE_DEFAULTS })
    if (glassSupported && !glassEnabled) {
      setWindowGlass(true).then(onSaved, (err) => onError(errorMessage(err, 'Could not change window transparency')))
    }
  }

  const opacity = Math.round(appearance.sidebarOpacity * 100)
  const scale = Math.round(appearance.uiScale * 100)
  const translucencyNote = !glassSupported
    ? 'Window transparency is available on macOS and Windows.'
    : light
      ? 'Light mode always uses an opaque window, so these are off. Your dark-mode values are preserved.'
      : !glassOn
        ? 'Turn on window transparency to adjust these. Your values are preserved.'
        : 'How much of the desktop shows through Clipward. Blur costs more to composite the higher it goes.'

  return (
    <>
      <PanelHeader
        icon={<IconTile size="sm"><AppWindow /></IconTile>}
        title="Appearance"
        description="How Clipward looks on this computer. Changes show as you make them."
        action={
          <Button size="sm" variant="ghost" icon={<RotateCcw className="h-3.5 w-3.5" />} onClick={reset}>
            Reset appearance
          </Button>
        }
      />

      <AppearanceGroup title="Theme" description="Dark and light share the same tint, so the color settings below apply to both.">
        <SettingRow
          title="Theme"
          description="System follows the OS appearance."
          control={
            <Segmented<ThemePreference>
              label="Theme"
              value={appearance.theme}
              options={[
                { value: 'system', label: 'System' },
                { value: 'dark', label: 'Dark' },
                { value: 'light', label: 'Light' }
              ]}
              onChange={(theme) => change({ theme })}
            />
          }
        />
        <SettingRow
          title="Accent color"
          description="Primary buttons, chosen options, progress and focus rings."
          control={<AccentColorPicker value={appearance.accentColor} light={light} onChange={(accentColor) => change({ accentColor })} />}
        />
      </AppearanceGroup>

      <AppearanceGroup title="Color" description="Hue and saturation tint every surface. Lightness only moves the dark theme.">
        <SettingRow
          title="Hue"
          description="Base hue for tinted surfaces."
          control={
            <Slider
              label="Hue"
              value={appearance.hue}
              display={`${appearance.hue}°`}
              min={THEME_HUE_MIN}
              max={THEME_HUE_MAX}
              onChange={(hue) => change({ hue })}
            />
          }
        />
        <SettingRow
          title="Saturation"
          description="How strongly the hue tints the interface. Zero keeps it neutral."
          control={
            <Slider
              label="Saturation"
              value={appearance.saturation}
              display={`${appearance.saturation}%`}
              min={THEME_SATURATION_MIN}
              max={THEME_SATURATION_MAX}
              onChange={(saturation) => change({ saturation })}
            />
          }
        />
        <SettingRow
          title="Dark-mode lightness"
          description={
            light
              ? 'This only affects dark mode. Your dark-mode value is preserved.'
              : 'Base brightness of the dark theme. Lower values are darker; zero is true black.'
          }
          control={
            <Slider
              label="Dark-mode lightness"
              value={appearance.darkLightness}
              display={`${appearance.darkLightness}%`}
              min={THEME_DARK_LIGHTNESS_MIN}
              max={THEME_DARK_LIGHTNESS_MAX}
              onChange={(darkLightness) => change({ darkLightness })}
              disabled={light}
            />
          }
        />
      </AppearanceGroup>

      <AppearanceGroup title="Translucency" description={translucencyNote}>
        <WindowGlassRow onSaved={onSaved} onError={onError} />
        <SettingRow
          title="Sidebar opacity"
          description="How solid the sidebar is over the glass. With main pane glass, the pages too."
          control={
            <Slider
              label="Sidebar opacity"
              value={opacity}
              display={`${opacity}%`}
              min={Math.round(SIDEBAR_OPACITY_MIN * 100)}
              max={Math.round(SIDEBAR_OPACITY_MAX * 100)}
              onChange={(percent) => change({ sidebarOpacity: percent / 100 })}
              disabled={glassOff}
            />
          }
        />
        <SettingRow
          title="Blur radius"
          description={blurSupported || !glassSupported ? 'Background blur behind the window.' : 'Windows draws Mica and acrylic with a fixed blur.'}
          control={
            <Slider
              label="Blur radius"
              value={appearance.blurRadius}
              display={String(appearance.blurRadius)}
              min={SIDEBAR_BLUR_MIN}
              max={SIDEBAR_BLUR_MAX}
              onChange={(blurRadius) => change({ blurRadius })}
              disabled={glassOff || !blurSupported}
            />
          }
        />
        <SettingRow
          title="Main pane glass"
          description="Extend the translucent treatment to the pages beside the sidebar."
          control={
            <Switch
              label="Main pane glass"
              checked={appearance.mainPaneGlass}
              disabled={glassOff}
              onChange={(mainPaneGlass) => change({ mainPaneGlass })}
            />
          }
        />
      </AppearanceGroup>

      <AppearanceGroup title="Layout">
        <SettingRow
          title="Collapsed rail"
          description={`Show the sidebar as a compact icon rail. ${MOD_KEY}${SIDEBAR_SHORTCUT_KEY} toggles it. Narrow windows always use the rail.`}
          control={
            <Switch label="Collapsed rail" checked={appearance.collapsedRail} onChange={(collapsedRail) => change({ collapsedRail })} />
          }
        />
        <SettingRow
          title="Interface scale"
          description={`Zoom the whole interface. You can also use ${MOD_KEY}=, ${MOD_KEY}- and ${MOD_KEY}0.`}
          control={
            <Slider
              label="Interface scale"
              value={scale}
              display={`${scale}%`}
              min={Math.round(UI_SCALE_MIN * 100)}
              max={Math.round(UI_SCALE_MAX * 100)}
              step={10}
              onChange={(percent) => change({ uiScale: percent / 100 })}
            />
          }
        />
      </AppearanceGroup>
    </>
  )
}

function AppearanceGroup({ title, description, children }: { title: string; description?: string; children: ReactNode }): React.JSX.Element {
  return (
    <div className="mt-5">
      <h3 className="eyebrow">{title}</h3>
      {description && <p className="mt-0.5 max-w-[70ch] text-caption text-ink-secondary">{description}</p>}
      <div className="mt-2 space-y-1.5">{children}</div>
    </div>
  )
}

/** Clipward's own accent in each scheme (globals.css `--accent`). */
const DEFAULT_ACCENT = { dark: 'rgb(69 155 247)', light: 'rgb(41 134 238)' } as const

/** Default, MonoCode's six presets, and a custom colour in a popover. */
function AccentColorPicker({ value, light, onChange }: { value: string | null; light: boolean; onChange: (value: string | null) => void }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const root = useRef<HTMLDivElement>(null)
  const presetIndex = value == null ? 0 : ACCENT_COLOR_PRESETS.indexOf(value as (typeof ACCENT_COLOR_PRESETS)[number]) + 1
  const custom = value != null && presetIndex === 0 ? value : null

  useEffect(() => {
    if (!open) return
    const onPointer = (event: PointerEvent): void => {
      if (!root.current?.contains(event.target as Node)) setOpen(false)
    }
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') setOpen(false)
    }
    window.addEventListener('pointerdown', onPointer)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('pointerdown', onPointer)
      window.removeEventListener('keydown', onKey)
    }
  }, [open])

  return (
    <div ref={root} className="relative shrink-0">
      <ColorSwatchRow
        colors={[DEFAULT_ACCENT[light ? 'light' : 'dark'], ...ACCENT_COLOR_PRESETS]}
        labels={['Default', ...ACCENT_COLOR_LABELS]}
        selectedIndex={custom ? null : presetIndex}
        customColor={custom}
        customOpen={open}
        onPick={(index) => {
          setOpen(false)
          onChange(index === 0 ? null : ACCENT_COLOR_PRESETS[index - 1])
        }}
        onToggleCustom={() => setOpen((current) => !current)}
      />
      {open && (
        <div className={cn('popover-surface popover-open absolute right-0 top-full mt-2 w-[248px] rounded-xl p-2', LAYER_CLASS.popover)}>
          <ColorPicker value={value ?? ACCENT_COLOR_PRESETS[0]} onChange={onChange} />
        </div>
      )}
    </div>
  )
}

/** Native window glass: on or off, applied at once by the app shell. */
function WindowGlassRow({ onSaved, onError }: SaveCallbacks): React.JSX.Element {
  const supported = useSettingsStore((s) => s.windowGlassSupported)
  const enabled = useSettingsStore((s) => s.windowGlass)
  const setWindowGlass = useSettingsStore((s) => s.setWindowGlass)
  const reduced = useReducedTransparency()
  const [pending, setPending] = useState(false)

  const toggle = async (next: boolean): Promise<void> => {
    setPending(true)
    try {
      await setWindowGlass(next)
      onSaved()
    } catch (err) {
      onError(errorMessage(err, 'Could not change window transparency'))
    } finally {
      setPending(false)
    }
  }

  const description = !supported
    ? 'Available on macOS and Windows.'
    : reduced
      ? 'Your system asks for reduced transparency, so the window stays opaque.'
      : 'The desktop shows softly through the sidebar, and the pages with main pane glass, in dark mode. Video, cards and fields stay solid.'

  return (
    <SettingRow
      title="Window transparency"
      description={description}
      control={
        <Switch
          label="Window transparency"
          checked={supported && enabled}
          disabled={!supported || pending}
          onChange={(next) => void toggle(next)}
        />
      }
    />
  )
}

function SaveIndicator({ saving, savedAt, error }: { saving: boolean; savedAt: number | null; error: string | null }): React.JSX.Element | null {
  const [visible, setVisible] = useState(false)
  useEffect(() => {
    if (!savedAt) return
    setVisible(true)
    const t = setTimeout(() => setVisible(false), 2200)
    return () => clearTimeout(t)
  }, [savedAt])

  if (error) return <Badge tone="danger">{error}</Badge>
  if (saving) {
    return (
      <Badge icon={<Loader2 className="h-3 w-3 animate-spin" />}>
        Saving…
      </Badge>
    )
  }
  return (
    <span
      className={cn('transition-opacity duration-300', visible ? 'opacity-100' : 'opacity-0')}
      aria-live="polite"
    >
      <Badge tone="success" icon={<Check className="h-3 w-3" strokeWidth={3} />}>
        Saved
      </Badge>
    </span>
  )
}

function OutputFolder({ value, onCommit }: { value: string; onCommit: (dir: string) => void }): React.JSX.Element {
  const [error, setError] = useState<string | null>(null)

  const choose = async (): Promise<void> => {
    try {
      const dir = await getApi().settings.selectOutputDir()
      if (dir) {
        setError(null)
        onCommit(dir)
      }
    } catch (err) {
      setError(errorMessage(err, 'Could not choose an output folder'))
    }
  }

  const open = async (): Promise<void> => {
    try {
      if (!(await getApi().shell.openPath(value))) setError('Could not open the output folder')
      else setError(null)
    } catch (err) {
      setError(errorMessage(err, 'Could not open the output folder'))
    }
  }

  return (
    <div className="mt-4">
      <div className="flex items-center gap-2">
        <TextInput
          className="flex-1"
          mono
          value={value}
          readOnly
          title="Use Change to select a folder"
          leading={<FolderOpen strokeWidth={1.75} />}
          aria-label="Output folder"
        />
        <Button onClick={() => void choose()}>Change…</Button>
        <Button variant="ghost" onClick={() => void open()} disabled={!value}>
          Open
        </Button>
      </div>
      {error && <p role="alert" className="mt-2 text-caption text-danger">{error}</p>}
    </div>
  )
}

function VocabularyField({ value, onCommit }: { value: string; onCommit: (value: string) => void }): React.JSX.Element {
  const [draft, setDraft] = useState(value)
  useEffect(() => setDraft(value), [value])
  return (
    <div className="mt-4">
      <TextArea
        rows={4}
        value={draft}
        placeholder={'Acme Energy\nMrBeast\nWhop'}
        aria-label="Custom vocabulary"
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => draft !== value && onCommit(draft)}
      />
      <p className="mt-2 text-caption text-ink-secondary">Up to five words per term. Applies to new transcriptions; generated metadata uses it right away.</p>
    </div>
  )
}

function DevPathField({
  label,
  value,
  placeholder,
  onCommit
}: {
  label: string
  value: string
  placeholder: string
  onCommit: (value: string) => void
}): React.JSX.Element {
  const [draft, setDraft] = useState(value)
  useEffect(() => setDraft(value), [value])
  return (
    <Field label={label} hint="Developer builds only">
      <TextInput
        mono
        value={draft}
        placeholder={placeholder}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => draft.trim() !== value && onCommit(draft.trim())}
        onKeyDown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
      />
    </Field>
  )
}

interface ToolRow { name: string; ok: boolean | null; detail?: ReactNode; optional?: boolean }

function toolRows(status: ToolStatus | null): ToolRow[] {
  return [
    { name: 'Python', ok: status?.python ?? null, detail: status?.pythonPath },
    {
      name: 'Clipping dependencies and smart framing',
      ok: status?.pythonDeps ?? null,
      detail: status && !status.pythonDeps ? status.pythonError : 'Includes OpenCV and the smart framing model'
    },
    { name: 'FFmpeg', ok: status?.ffmpeg ?? null },
    {
      name: 'FFmpeg captions',
      ok: status?.ffmpegCaptions ?? null,
      detail: 'Required when captions are enabled; FFmpeg must include the ass filter',
      optional: true
    },
    { name: 'FFprobe', ok: status?.ffprobe ?? null },
    { name: 'yt-dlp', ok: status?.ytdlp ?? null, detail: 'Downloads from YouTube, Twitch, Kick, X and other sites' },
    { name: 'Clipward clipping engine', ok: status?.engine ?? null, detail: status?.enginePath },
    { name: 'Bridge runner', ok: status?.bridgeRunner ?? null, detail: status?.bridgePath }
  ]
}

function ToolList({ rows, checking }: { rows: ToolRow[]; checking: boolean }): React.JSX.Element {
  const [expanded, setExpanded] = useState(false)
  const required = rows.filter((row) => !row.optional)
  const missing = required.filter((row) => row.ok === false).length
  const checked = rows.every((row) => row.ok != null)

  return (
    <div className="mt-4">
      {checked && (
        <div className="mb-2 flex items-center gap-2 text-caption">
          <StatusDot tone={missing > 0 ? 'danger' : 'success'} />
          <span className={cn('flex-1', missing > 0 ? 'text-danger' : 'text-ink-secondary')}>
            {missing > 0 ? `${missing} required tool${missing === 1 ? '' : 's'} missing` : `Everything Clipward needs is installed (${rows.filter((row) => row.ok).length} tools)`}
          </span>
          {missing === 0 && (
            <Button
              size="sm"
              variant="ghost"
              aria-expanded={expanded}
              trailingIcon={<ChevronDown className={cn('h-3.5 w-3.5 transition-transform', expanded && 'rotate-180')} />}
              onClick={() => setExpanded(!expanded)}
            >
              {expanded ? 'Hide details' : 'Show details'}
            </Button>
          )}
        </div>
      )}
      {(!checked || missing > 0 || expanded) && <div className="field divide-y divide-stroke overflow-hidden rounded-lg animate-fade-in">
        {rows.map((row) => {
          const tone = row.ok == null ? 'idle' : row.ok ? 'success' : row.optional ? 'warning' : 'danger'
          return (
            <div key={row.name} className="flex items-center gap-3 px-3 py-2">
              <StatusDot tone={tone} />
              <div className="min-w-0 flex-1">
                <p className="text-body text-ink">{row.name}</p>
                {row.detail && (
                  <p className="mt-0.5 break-words font-mono text-meta text-ink-tertiary" data-selectable>
                    {row.detail}
                  </p>
                )}
              </div>
              {row.ok == null ? (
                <span className="shrink-0 text-caption text-ink-tertiary">{checking ? 'Checking…' : 'Not checked'}</span>
              ) : (
                <Badge tone={row.ok ? 'neutral' : row.optional ? 'warning' : 'danger'} className="shrink-0">
                  {row.ok ? 'Found' : row.optional ? 'Unavailable' : 'Missing'}
                </Badge>
              )}
            </div>
          )
        })}
      </div>}
    </div>
  )
}
