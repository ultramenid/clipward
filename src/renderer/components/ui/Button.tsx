import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from 'react'
import { Loader2 } from 'lucide-react'
import { cn } from '../../lib/utils'

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger'
type Size = 'sm' | 'md' | 'lg'

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant
  size?: Size
  icon?: ReactNode
  trailingIcon?: ReactNode
  loading?: boolean
  /** Square button with only an icon; pass `aria-label`. */
  iconOnly?: boolean
}

// Dense, flat controls: small radius, hairline borders, content-tinted hovers.
const VARIANTS: Record<Variant, string> = {
  primary:
    'bg-accent text-accent-ink hover:brightness-110 disabled:bg-accent/30 disabled:text-accent-ink/60 disabled:hover:brightness-100',
  secondary:
    'border border-content/10 text-content/80 hover:bg-content/10 hover:text-content disabled:text-content/35 disabled:hover:bg-transparent',
  ghost: 'text-ink-secondary hover:bg-content/10 hover:text-content disabled:text-content/30 disabled:hover:bg-transparent',
  danger:
    'border border-danger/30 text-danger hover:border-danger/50 hover:bg-danger/10 disabled:opacity-40 disabled:hover:bg-transparent'
}

const SIZES: Record<Size, string> = {
  sm: 'h-6 px-2 text-caption gap-1.5',
  md: 'h-7 px-2.5 text-caption gap-1.5',
  lg: 'h-8 px-3 text-body gap-2'
}

const ICON_ONLY: Record<Size, string> = {
  sm: 'h-6 w-6',
  md: 'h-7 w-7',
  lg: 'h-8 w-8'
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  {
    variant = 'secondary',
    size = 'md',
    icon,
    trailingIcon,
    loading = false,
    iconOnly = false,
    className,
    children,
    disabled,
    type = 'button',
    ...props
  },
  ref
) {
  return (
    <button
      ref={ref}
      type={type}
      disabled={disabled || loading}
      className={cn(
        'inline-flex shrink-0 select-none items-center justify-center whitespace-nowrap rounded-md font-medium',
        'transition-[background-color,border-color,color,filter] duration-feedback ease-out',
        VARIANTS[variant],
        iconOnly ? ICON_ONLY[size] : SIZES[size],
        className
      )}
      {...props}
    >
      {loading ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : icon}
      {!iconOnly && children}
      {!loading && trailingIcon}
    </button>
  )
})
