import type { ReactNode } from 'react'
import { cn } from '../../lib/utils'

export type Tone = 'neutral' | 'accent' | 'success' | 'danger' | 'warning'

const TONES: Record<Tone, string> = {
  neutral: 'bg-content/5 text-ink-secondary border-content/10',
  accent: 'bg-accent/10 text-accent-fg border-accent/25',
  success: 'bg-success/10 text-success border-success/25',
  danger: 'bg-danger/10 text-danger border-danger/25',
  warning: 'bg-warning/10 text-warning border-warning/25'
}

interface BadgeProps {
  tone?: Tone
  icon?: ReactNode
  children: ReactNode
  className?: string
}

/** A compact status label: tinted fill, hairline border, 18px tall, `text-meta`. */
export function Badge({ tone = 'neutral', icon, children, className }: BadgeProps): React.JSX.Element {
  return (
    <span
      className={cn(
        'inline-flex min-h-[1.125rem] shrink-0 items-center gap-1 rounded border px-1.5 py-px text-meta font-medium',
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
