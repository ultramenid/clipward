import type { ReactNode } from 'react'
import { AlertTriangle, Check, Info, TriangleAlert, X } from 'lucide-react'
import { cn } from '../../lib/utils'
import { Button } from './Button'

type CalloutTone = 'danger' | 'warning' | 'success' | 'info'

const TONES: Record<CalloutTone, { box: string; icon: string; Icon: typeof Info }> = {
  danger: {
    box: 'bg-danger/[0.07] shadow-[inset_0_0_0_1px_rgb(var(--danger)/0.26),inset_0_1px_0_rgb(255_255_255/0.05)]',
    icon: 'text-danger',
    Icon: AlertTriangle
  },
  warning: {
    box: 'bg-warning/[0.06] shadow-[inset_0_0_0_1px_rgb(var(--warning)/0.24),inset_0_1px_0_rgb(255_255_255/0.05)]',
    icon: 'text-warning',
    Icon: TriangleAlert
  },
  success: {
    box: 'bg-success/[0.06] shadow-[inset_0_0_0_1px_rgb(var(--success)/0.24),inset_0_1px_0_rgb(255_255_255/0.05)]',
    icon: 'text-success',
    Icon: Check
  },
  info: {
    box: 'bg-white/[0.045] shadow-[inset_0_0_0_1px_rgb(255_255_255/0.1),inset_0_1px_0_rgb(255_255_255/0.06)]',
    icon: 'text-ink-muted',
    Icon: Info
  }
}

interface CalloutProps {
  tone?: CalloutTone
  /** Replaces the tone's default icon. */
  icon?: ReactNode
  title?: ReactNode
  children?: ReactNode
  /** Buttons shown at the right (or under the text when `stacked`). */
  action?: ReactNode
  onDismiss?: () => void
  /** `role` for assistive tech; defaults to alert for danger, status otherwise. */
  role?: 'alert' | 'status'
  stacked?: boolean
  className?: string
  'data-testid'?: string
}

/** Tinted glass strip for errors, warnings and confirmations. */
export function Callout({
  tone = 'info',
  icon,
  title,
  children,
  action,
  onDismiss,
  role,
  stacked = false,
  className,
  'data-testid': testId
}: CalloutProps): React.JSX.Element {
  const t = TONES[tone]
  return (
    <div
      role={role ?? (tone === 'danger' ? 'alert' : 'status')}
      data-testid={testId}
      className={cn('flex items-start gap-2.5 rounded-xl px-3 py-2.5 animate-fade-in', t.box, className)}
    >
      <span className={cn('mt-px shrink-0 [&_svg]:h-4 [&_svg]:w-4', t.icon)}>
        {icon ?? <t.Icon strokeWidth={tone === 'success' ? 3 : 2} />}
      </span>
      <div className="min-w-0 flex-1">
        {title && <p className="text-sm font-medium text-ink">{title}</p>}
        {children && (
          <div className={cn('text-sm leading-relaxed', title ? 'mt-0.5 text-ink-muted' : 'text-ink')} data-selectable>
            {children}
          </div>
        )}
        {stacked && action && <div className="mt-3 flex flex-wrap gap-2">{action}</div>}
      </div>
      {!stacked && action && <div className="-my-1 flex shrink-0 items-center gap-2">{action}</div>}
      {onDismiss && (
        <Button
          variant="ghost"
          size="sm"
          iconOnly
          aria-label="Dismiss"
          onClick={onDismiss}
          className="-my-1 -mr-1.5"
          icon={<X className="h-3.5 w-3.5" />}
        />
      )}
    </div>
  )
}
