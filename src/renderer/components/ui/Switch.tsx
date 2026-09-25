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
        'relative inline-flex h-6 w-[42px] shrink-0 items-center rounded-full transition-[background,box-shadow] duration-200 ease-out',
        checked
          ? 'bg-accent shadow-[inset_0_1px_0_rgb(255_255_255/0.3),0_0_0_1px_rgb(var(--accent)/0.6)]'
          : 'bg-black/30 shadow-[inset_0_1px_2px_rgb(0_0_0/0.4),inset_0_0_0_1px_rgb(255_255_255/0.12)]',
        disabled && 'opacity-40',
        className
      )}
    >
      <span
        className={cn(
          'block h-5 w-5 rounded-full bg-white shadow-[0_1px_3px_rgb(0_0_0/0.45),0_0_0_0.5px_rgb(0_0_0/0.1)] transition-transform duration-300 ease-spring',
          checked ? 'translate-x-5' : 'translate-x-0.5'
        )}
      />
    </button>
  )
}
