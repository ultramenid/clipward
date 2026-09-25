import type { ReactNode } from 'react'
import { cn } from '../../lib/utils'
import type { Tone } from './Badge'

type Size = 'sm' | 'md' | 'lg' | 'xl'

const SIZES: Record<Size, string> = {
  sm: 'h-6 w-6 rounded-md [&_svg]:h-3.5 [&_svg]:w-3.5',
  md: 'h-8 w-8 rounded-lg [&_svg]:h-4 [&_svg]:w-4',
  lg: 'h-10 w-10 rounded-lg [&_svg]:h-[18px] [&_svg]:w-[18px]',
  xl: 'h-12 w-12 rounded-xl [&_svg]:h-5 [&_svg]:w-5'
}

const TONES: Record<Tone, string> = {
  neutral: 'text-content/70 bg-content/5 border border-content/10',
  accent: 'text-accent-fg bg-accent/10 border border-accent/25',
  success: 'text-success bg-success/10 border border-success/25',
  warning: 'text-warning bg-warning/10 border border-warning/25',
  danger: 'text-danger bg-danger/10 border border-danger/25'
}

/** A small tinted square holding an icon. */
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
