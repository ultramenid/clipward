import { cn } from '../../lib/utils'

interface SliderProps {
  /** Accessible name; sliders sit beside a visible title. */
  label: string
  value: number
  min: number
  max: number
  step?: number
  /** The value as shown to the right, e.g. "85%" or "24". */
  display: string
  onChange: (value: number) => void
  disabled?: boolean
  className?: string
}

/**
 * A range input on a thin content-tinted track with its value to the right,
 * 224px wide, as in MonoCode's settings. Changes stream while dragging.
 */
export function Slider({ label, value, min, max, step = 1, display, onChange, disabled, className }: SliderProps): React.JSX.Element {
  return (
    <div className={cn('flex w-56 max-w-full shrink-0 items-center gap-3', disabled && 'opacity-40', className)}>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        aria-label={label}
        aria-valuetext={display}
        disabled={disabled}
        onChange={(event) => onChange(Number(event.target.value))}
        className={cn(
          'h-1 min-w-0 flex-1 cursor-default appearance-none rounded-full bg-content/15 outline-offset-4',
          '[&::-webkit-slider-thumb]:h-3 [&::-webkit-slider-thumb]:w-3 [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-content',
          '[&::-webkit-slider-thumb]:transition-transform [&::-webkit-slider-thumb]:duration-feedback active:[&::-webkit-slider-thumb]:scale-110',
          'disabled:[&::-webkit-slider-thumb]:bg-content/60'
        )}
      />
      <span className="w-10 shrink-0 text-right font-mono text-xs tabular text-content">{display}</span>
    </div>
  )
}
