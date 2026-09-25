import { cn } from '../../lib/utils'

interface ProgressBarProps {
  value: number
  failed?: boolean
  className?: string
}

export function ProgressBar({ value, failed, className }: ProgressBarProps): React.JSX.Element {
  const pct = Math.max(0, Math.min(100, value))
  return (
    <div
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(pct)}
      className={cn(
        'relative h-1 w-full overflow-hidden rounded-full bg-content/10',
        className
      )}
    >
      <div
        className={cn(
          'h-full rounded-full transition-[width] duration-500 ease-out',
          failed ? 'bg-danger' : 'bg-accent'
        )}
        style={{ width: `${pct}%` }}
      />
    </div>
  )
}

interface ProgressRingProps {
  value: number
  size?: number
  stroke?: number
  className?: string
  children?: React.ReactNode
}

/** Circular progress with a solid accent stroke; children render in the centre. */
export function ProgressRing({ value, size = 200, stroke = 10, className, children }: ProgressRingProps): React.JSX.Element {
  const pct = Math.max(0, Math.min(100, value))
  const r = (size - stroke) / 2
  const c = 2 * Math.PI * r
  return (
    <div
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(pct)}
      className={cn('relative shrink-0', className)}
      style={{ width: size, height: size }}
    >
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} className="relative -rotate-90">
        <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="hsl(var(--content) / 0.1)" strokeWidth={stroke} />
        <circle
          cx={size / 2}
          cy={size / 2}
          r={r}
          fill="none"
          stroke="rgb(var(--accent))"
          strokeWidth={stroke}
          strokeLinecap="round"
          strokeDasharray={c}
          strokeDashoffset={c * (1 - pct / 100)}
          className="transition-[stroke-dashoffset] duration-700 ease-out"
        />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center">{children}</div>
    </div>
  )
}
