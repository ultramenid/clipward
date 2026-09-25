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
        'flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-[6px] transition-[background,box-shadow] duration-150',
        on
          ? 'bg-accent text-accent-ink shadow-[inset_0_1px_0_rgb(255_255_255/0.35),0_0_0_1px_rgb(var(--accent)/0.7)]'
          : variant === 'overlay'
            ? 'bg-black/35 text-transparent shadow-[inset_0_0_0_1.5px_rgb(255_255_255/0.55)] backdrop-blur-md hover:shadow-[inset_0_0_0_1.5px_rgb(255_255_255/0.85)]'
            : 'bg-black/25 text-transparent shadow-[inset_0_0_0_1px_rgb(255_255_255/0.22),inset_0_1px_2px_rgb(0_0_0/0.3)] hover:shadow-[inset_0_0_0_1px_rgb(255_255_255/0.4),inset_0_1px_2px_rgb(0_0_0/0.3)]',
        disabled && 'cursor-not-allowed opacity-40',
        className
      )}
    >
      {indeterminate ? <Minus className="h-3 w-3" strokeWidth={3} /> : <Check className="h-3 w-3" strokeWidth={3} />}
    </button>
  )
}
