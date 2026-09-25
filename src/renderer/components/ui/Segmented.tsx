import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from 'react'
import { cn } from '../../lib/utils'

export interface SegmentedOption<T extends string> {
  value: T
  label: ReactNode
  /** Why the option is unavailable; disables it and becomes its tooltip. */
  disabled?: string
}

interface SegmentedProps<T extends string> {
  label: string
  value: T
  options: SegmentedOption<T>[]
  onChange: (value: T) => void
  size?: 'sm' | 'md'
  className?: string
}

/** Arrow keys, Home and End move between the radios of a group (roving tab stop). */
export function onRadioKeyDown(event: ReactKeyboardEvent<HTMLButtonElement>): void {
  const radios = Array.from(
    event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="radio"]:not(:disabled)') ?? []
  )
  const current = radios.indexOf(event.currentTarget)
  if (current < 0) return
  let next = current
  if (event.key === 'ArrowRight' || event.key === 'ArrowDown') next = (current + 1) % radios.length
  else if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') next = (current - 1 + radios.length) % radios.length
  else if (event.key === 'Home') next = 0
  else if (event.key === 'End') next = radios.length - 1
  else return
  event.preventDefault()
  radios[next].focus()
  radios[next].click()
}

/** Single choice in a hairline track; the chosen segment gets the selection fill. */
export function Segmented<T extends string>({ label, value, options, onChange, size = 'md', className }: SegmentedProps<T>): React.JSX.Element {
  return (
    <div
      className={cn(
        'inline-flex items-center gap-0.5 rounded-md border border-content/10 p-0.5',
        className
      )}
      role="radiogroup"
      aria-label={label}
    >
      {options.map((option) => {
        const selected = value === option.value
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected ? 0 : -1}
            disabled={Boolean(option.disabled)}
            title={option.disabled}
            onKeyDown={onRadioKeyDown}
            onClick={() => onChange(option.value)}
            className={cn(
              'rounded-[4px] transition-[background-color,color] duration-feedback ease-out',
              size === 'sm' ? 'h-5 px-2 text-2xs' : 'h-6 px-2.5 text-xs',
              selected ? 'bg-selection text-content' : 'text-content/50 hover:text-content',
              option.disabled && 'text-content/25 hover:text-content/25'
            )}
          >
            {option.label}
          </button>
        )
      })}
    </div>
  )
}
