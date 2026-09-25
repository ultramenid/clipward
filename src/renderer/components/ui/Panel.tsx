import type { HTMLAttributes, ReactNode } from 'react'
import { cn } from '../../lib/utils'

interface PanelProps extends HTMLAttributes<HTMLDivElement> {
  padded?: boolean
}

/** Glass panel: the standard surface for a group of related content. */
export function Panel({ padded = true, className, ...props }: PanelProps): React.JSX.Element {
  return <div className={cn('glass rounded-3xl', padded && 'p-4 xl:p-5', className)} {...props} />
}

interface PanelHeaderProps {
  title: ReactNode
  description?: ReactNode
  action?: ReactNode
  /** Glass icon tile left of the title. */
  icon?: ReactNode
  className?: string
}

export function PanelHeader({ title, description, action, icon, className }: PanelHeaderProps): React.JSX.Element {
  return (
    <div className={cn('flex items-start justify-between gap-3', className)}>
      <div className="flex min-w-0 items-start gap-3">
        {icon}
        <div className="min-w-0">
          <h2 className="text-sm font-semibold leading-5 text-ink">{title}</h2>
          {description && <p className="mt-0.5 text-xs text-ink-muted">{description}</p>}
        </div>
      </div>
      {action && <div className="flex shrink-0 items-center gap-2">{action}</div>}
    </div>
  )
}
