import type { ReactNode } from 'react'
import { cn } from '../../lib/utils'

export type Tone = 'neutral' | 'accent' | 'success' | 'danger' | 'warning'

const TONES: Record<Tone, string> = {
  neutral: 'bg-white/[0.06] text-ink-muted shadow-[inset_0_0_0_1px_rgb(255_255_255/0.1),inset_0_1px_0_rgb(255_255_255/0.08)]',
  accent: 'bg-accent/[0.14] text-accent-hover shadow-[inset_0_0_0_1px_rgb(var(--accent)/0.32),inset_0_1px_0_rgb(255_255_255/0.08)]',
  success: 'bg-success/[0.12] text-success shadow-[inset_0_0_0_1px_rgb(var(--success)/0.28),inset_0_1px_0_rgb(255_255_255/0.06)]',
  danger: 'bg-danger/[0.12] text-danger shadow-[inset_0_0_0_1px_rgb(var(--danger)/0.3),inset_0_1px_0_rgb(255_255_255/0.06)]',
  warning: 'bg-warning/[0.12] text-warning shadow-[inset_0_0_0_1px_rgb(var(--warning)/0.28),inset_0_1px_0_rgb(255_255_255/0.06)]'
}

interface BadgeProps {
  tone?: Tone
  icon?: ReactNode
  children: ReactNode
  className?: string
}

export function Badge({ tone = 'neutral', icon, children, className }: BadgeProps): React.JSX.Element {
  return (
    <span
      className={cn(
        'inline-flex h-5 shrink-0 items-center gap-1 rounded-full px-2 text-2xs font-medium',
        TONES[tone],
        className
      )}
    >
      {icon}
      {children}
    </span>
  )
}

type DotTone = 'success' | 'danger' | 'warning' | 'accent' | 'idle'

const DOTS: Record<DotTone, string> = {
  success: 'bg-success',
  danger: 'bg-danger',
  warning: 'bg-warning',
  accent: 'bg-accent',
  idle: 'bg-ink-faint'
}

export function StatusDot({ tone, pulse, className }: { tone: DotTone; pulse?: boolean; className?: string }): React.JSX.Element {
  return (
    <span className={cn('relative inline-flex h-2 w-2 shrink-0', className)}>
      {pulse && <span className={cn('absolute inset-0 animate-ping rounded-full opacity-60', DOTS[tone])} />}
      <span className={cn('relative inline-flex h-2 w-2 rounded-full', DOTS[tone])} />
    </span>
  )
}
