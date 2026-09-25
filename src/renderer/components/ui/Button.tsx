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
  /** Round button with only an icon; pass `aria-label`. */
  iconOnly?: boolean
}

// Capsules throughout: liquid glass controls are pill-shaped.
const VARIANTS: Record<Variant, string> = {
  primary: 'btn-primary',
  secondary: 'btn-glass text-ink disabled:text-ink-subtle disabled:opacity-60',
  ghost: 'text-ink-muted hover:bg-white/[0.07] hover:text-ink disabled:text-ink-faint disabled:hover:bg-transparent',
  danger:
    'text-danger bg-danger/[0.08] shadow-[inset_0_0_0_1px_rgb(var(--danger)/0.28),inset_0_1px_0_rgb(255_255_255/0.06)] hover:bg-danger/[0.15] hover:shadow-[inset_0_0_0_1px_rgb(var(--danger)/0.45),inset_0_1px_0_rgb(255_255_255/0.08)]'
}

const SIZES: Record<Size, string> = {
  sm: 'h-7 px-2.5 text-xs gap-1.5',
  md: 'h-8 px-3.5 text-sm gap-1.5',
  lg: 'h-10 px-5 text-sm gap-2'
}

const ICON_ONLY: Record<Size, string> = {
  sm: 'h-7 w-7',
  md: 'h-8 w-8',
  lg: 'h-10 w-10'
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
        'inline-flex shrink-0 select-none items-center justify-center whitespace-nowrap rounded-full font-medium',
        'transition-[background,box-shadow,color,filter,transform] duration-200 ease-out',
        'active:scale-[0.97] disabled:active:scale-100',
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
