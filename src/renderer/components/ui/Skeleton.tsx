import { cn } from '../../lib/utils'

/** Loading placeholder: a faint flat block that pulses. */
export function Skeleton({ className }: { className?: string }): React.JSX.Element {
  return <span aria-hidden className={cn('block animate-pulse rounded-lg bg-white/[0.05]', className)} />
}
