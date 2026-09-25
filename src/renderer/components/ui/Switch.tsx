import { cn } from '../../lib/utils'

interface SwitchProps {
  checked: boolean
  onChange: (checked: boolean) => void
  disabled?: boolean
  label?: string
  className?: string
}

export function Switch({ checked, onChange, disabled, label, className }: SwitchProps): React.JSX.Element {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        'relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors duration-feedback ease-out',
        checked ? 'bg-accent' : 'bg-content/20',
        disabled && 'opacity-40',
        className
      )}
    >
      <span
        className={cn(
          'block h-4 w-4 rounded-full bg-white shadow-sm transition-transform duration-150 ease-out',
          checked ? 'translate-x-[18px]' : 'translate-x-0.5'
        )}
      />
    </button>
  )
}
