import type { ReactNode } from 'react'
import { cn } from '../../lib/utils'
import type { Tone } from './Badge'

type Size = 'sm' | 'md' | 'lg' | 'xl'

const SIZES: Record<Size, string> = {
  sm: 'h-7 w-7 rounded-lg [&_svg]:h-3.5 [&_svg]:w-3.5',
  md: 'h-8 w-8 rounded-[10px] [&_svg]:h-4 [&_svg]:w-4',
  lg: 'h-10 w-10 rounded-xl [&_svg]:h-[18px] [&_svg]:w-[18px]',
  xl: 'h-12 w-12 rounded-[14px] [&_svg]:h-[22px] [&_svg]:w-[22px]'
}

const TONES: Record<Tone, string> = {
  neutral:
    'text-ink bg-white/[0.08] shadow-[inset_0_1px_0_rgb(255_255_255/0.18),inset_0_0_0_1px_rgb(255_255_255/0.1),0_6px_16px_-8px_rgb(0_0_0/0.6)]',
  accent:
    'text-accent-hover bg-accent/20 shadow-[inset_0_1px_0_rgb(255_255_255/0.2),inset_0_0_0_1px_rgb(var(--accent)/0.4)]',
  success:
    'text-success bg-success/15 shadow-[inset_0_1px_0_rgb(255_255_255/0.16),inset_0_0_0_1px_rgb(var(--success)/0.35)]',
  warning:
    'text-warning bg-warning/15 shadow-[inset_0_1px_0_rgb(255_255_255/0.16),inset_0_0_0_1px_rgb(var(--warning)/0.35)]',
  danger:
    'text-danger bg-danger/15 shadow-[inset_0_1px_0_rgb(255_255_255/0.16),inset_0_0_0_1px_rgb(var(--danger)/0.38)]'
}

/** A small glass lens holding an icon. */
export function IconTile({ children, tone = 'neutral', size = 'md', className }: {
  children: ReactNode
  tone?: Tone
  size?: Size
  className?: string
}): React.JSX.Element {
  return (
    <span aria-hidden className={cn('flex shrink-0 items-center justify-center', SIZES[size], TONES[tone], className)}>
      {children}
    </span>
  )
}
