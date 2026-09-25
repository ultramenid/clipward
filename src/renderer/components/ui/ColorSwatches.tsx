import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'
import { Pipette } from 'lucide-react'
import { hexToHsv, hsvToHex, normalizeHex, type Hsv } from '../../lib/color'
import { cn } from '../../lib/utils'

/*
 * A row of colour swatches with a custom-colour button, and the picker it
 * opens. Ported from MonoCode's `src/shared/ui/ColorPickerPopover.tsx` (MIT,
 * Copyright (c) 2026 Nick; see THIRD_PARTY_NOTICES.md).
 */

const RING = 'ring-2 ring-content/80 ring-offset-1 ring-offset-transparent'

interface ColorSwatchRowProps {
  /** CSS colours, first to last. */
  colors: readonly string[]
  labels: readonly string[]
  /** The chosen swatch, or null when a custom colour is chosen. */
  selectedIndex: number | null
  /** The custom colour, shown in its button once chosen. */
  customColor: string | null
  customOpen: boolean
  onPick: (index: number) => void
  onToggleCustom: () => void
}

export function ColorSwatchRow({ colors, labels, selectedIndex, customColor, customOpen, onPick, onToggleCustom }: ColorSwatchRowProps): React.JSX.Element {
  return (
    <div className="flex items-center gap-1" role="group" aria-label="Colors">
      {colors.map((color, index) => {
        const selected = customColor == null && selectedIndex === index
        return (
          <button
            key={`${color}-${index}`}
            type="button"
            title={labels[index]}
            aria-label={labels[index]}
            aria-pressed={selected}
            onClick={() => onPick(index)}
            className="grid h-6 w-6 place-items-center rounded-full"
          >
            <span className={cn('h-4 w-4 rounded-full', selected && RING)} style={{ background: color }} />
          </button>
        )
      })}
      <button
        type="button"
        title="Custom color"
        aria-label="Custom color"
        aria-expanded={customOpen}
        aria-pressed={customColor != null}
        onClick={onToggleCustom}
        className="grid h-6 w-6 place-items-center rounded-full"
      >
        <span
          className={cn('grid h-4 w-4 place-items-center overflow-hidden rounded-full', (customColor != null || customOpen) && RING)}
          style={{ background: customColor ?? 'conic-gradient(red, yellow, lime, aqua, blue, magenta, red)' }}
        >
          {!customColor && <Pipette className="h-2.5 w-2.5 text-white drop-shadow-sm" strokeWidth={2.25} />}
        </span>
      </button>
    </div>
  )
}

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value))
}

/** Drag on `element`, reporting the pointer's position as 0–1 fractions. */
function dragFraction(event: ReactPointerEvent<HTMLDivElement>, element: HTMLDivElement | null, update: (x: number, y: number) => void): void {
  if (!element) return
  event.preventDefault()
  event.currentTarget.setPointerCapture(event.pointerId)
  const report = (clientX: number, clientY: number): void => {
    const rect = element.getBoundingClientRect()
    update(clamp01((clientX - rect.left) / rect.width), clamp01((clientY - rect.top) / rect.height))
  }
  report(event.clientX, event.clientY)
  const onMove = (e: PointerEvent): void => report(e.clientX, e.clientY)
  const onUp = (): void => {
    window.removeEventListener('pointermove', onMove)
    window.removeEventListener('pointerup', onUp)
    window.removeEventListener('pointercancel', onUp)
  }
  window.addEventListener('pointermove', onMove)
  window.addEventListener('pointerup', onUp)
  window.addEventListener('pointercancel', onUp)
}

/**
 * Saturation/brightness square, hue strip and hex field. Arrow keys move the
 * square (brightness up and down, saturation left and right) and the strip.
 */
export function ColorPicker({ value, onChange }: { value: string; onChange: (hex: string) => void }): React.JSX.Element {
  const [hsv, setHsv] = useState<Hsv>(() => hexToHsv(value))
  const [draft, setDraft] = useState<string | null>(null)
  const square = useRef<HTMLDivElement>(null)
  const strip = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const hex = normalizeHex(value)
    setHsv((prev) => (hsvToHex(prev.h, prev.s, prev.v) === hex ? prev : hexToHsv(hex)))
  }, [value])

  const apply = useCallback(
    (updater: (prev: Hsv) => Hsv) => {
      setHsv((prev) => {
        const next = updater(prev)
        onChange(hsvToHex(next.h, next.s, next.v))
        return next
      })
    },
    [onChange]
  )

  const onHexInput = (raw: string): void => {
    setDraft(raw)
    const trimmed = raw.trim()
    if (!/^#?[0-9a-fA-F]{6}$/.test(trimmed)) return
    const hex = normalizeHex(trimmed.startsWith('#') ? trimmed : `#${trimmed}`)
    setHsv(hexToHsv(hex))
    onChange(hex)
  }

  const nudge = (key: string, axis: 'sv' | 'h'): boolean => {
    const step = 2
    if (axis === 'h') {
      if (key !== 'ArrowLeft' && key !== 'ArrowRight') return false
      apply((prev) => ({ ...prev, h: Math.min(360, Math.max(0, prev.h + (key === 'ArrowRight' ? step : -step))) }))
      return true
    }
    const delta = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, step], ArrowDown: [0, -step] }[key]
    if (!delta) return false
    apply((prev) => ({ ...prev, s: Math.min(100, Math.max(0, prev.s + delta[0])), v: Math.min(100, Math.max(0, prev.v + delta[1])) }))
    return true
  }

  const preview = hsvToHex(hsv.h, hsv.s, hsv.v)
  const hueColor = hsvToHex(hsv.h, 100, 100)
  const handle = 'pointer-events-none absolute h-3.5 w-3.5 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow-md'

  return (
    <div className="space-y-2">
      <div
        ref={square}
        role="slider"
        tabIndex={0}
        aria-label="Saturation and brightness"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(hsv.s)}
        aria-valuetext={`Saturation ${Math.round(hsv.s)}%, brightness ${Math.round(hsv.v)}%`}
        className="relative h-28 w-full touch-none rounded-md"
        style={{ background: `linear-gradient(to top, #000, transparent), linear-gradient(to right, #fff, ${hueColor})` }}
        onPointerDown={(event) => dragFraction(event, square.current, (x, y) => apply((prev) => ({ ...prev, s: x * 100, v: (1 - y) * 100 })))}
        onKeyDown={(event) => nudge(event.key, 'sv') && event.preventDefault()}
      >
        <span className={handle} style={{ left: `${hsv.s}%`, top: `${100 - hsv.v}%`, background: preview }} />
      </div>
      <div
        ref={strip}
        role="slider"
        tabIndex={0}
        aria-label="Hue"
        aria-valuemin={0}
        aria-valuemax={360}
        aria-valuenow={Math.round(hsv.h)}
        className="relative h-3 w-full touch-none rounded-full"
        style={{ background: 'linear-gradient(to right, #f00, #ff0, #0f0, #0ff, #00f, #f0f, #f00)' }}
        onPointerDown={(event) => dragFraction(event, strip.current, (x) => apply((prev) => ({ ...prev, h: x * 360 })))}
        onKeyDown={(event) => nudge(event.key, 'h') && event.preventDefault()}
      >
        <span className={cn(handle, 'top-1/2')} style={{ left: `${(hsv.h / 360) * 100}%`, background: hueColor }} />
      </div>
      <div className="flex items-center gap-2">
        <span className="h-7 w-7 shrink-0 rounded-md border border-content/10" style={{ background: preview }} aria-hidden />
        <input
          type="text"
          value={draft ?? preview}
          spellCheck={false}
          aria-label="Hex color"
          onChange={(event) => onHexInput(event.target.value)}
          onBlur={() => setDraft(null)}
          className="field h-7 min-w-0 flex-1 rounded-md px-2 font-mono text-caption text-content outline-none focus:border-accent"
        />
      </div>
    </div>
  )
}
