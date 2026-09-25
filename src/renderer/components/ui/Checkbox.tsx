import { Check, Minus } from 'lucide-react'
import { cn } from '../../lib/utils'

interface CheckboxProps {
  checked: boolean
  indeterminate?: boolean
  onChange: (checked: boolean) => void
  label: string
  disabled?: boolean
  /** `overlay` sits on video and needs its own frosted backing. */
  variant?: 'default' | 'overlay'
  className?: string
}

export function Checkbox({ checked, indeterminate, onChange, label, disabled, variant = 'default', className }: CheckboxProps): React.JSX.Element {
  const on = checked || indeterminate
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={indeterminate ? 'mixed' : checked}
      aria-label={label}
      disabled={disabled}
      onClick={(e) => {
        e.stopPropagation()
        onChange(!checked)
      }}
      className={cn(
        'flex h-4 w-4 shrink-0 items-center justify-center rounded border transition-colors duration-feedback',
        on
          ? 'border-accent bg-accent text-accent-ink'
          : variant === 'overlay'
            ? 'border-white/60 bg-black/40 text-transparent backdrop-blur-md hover:border-white/90'
            : 'border-content/25 bg-transparent text-transparent hover:border-content/45',
        disabled && 'opacity-40',
        className
      )}
    >
      {indeterminate ? <Minus className="h-2.5 w-2.5" strokeWidth={3} /> : <Check className="h-2.5 w-2.5" strokeWidth={3} />}
    </button>
  )
}
