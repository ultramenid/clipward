import type { ReactNode } from 'react'
import { cn } from '../../lib/utils'
import { IconTile } from './IconTile'

interface EmptyStateProps {
  icon: ReactNode
  title: string
  description?: ReactNode
  action?: ReactNode
  className?: string
}

export function EmptyState({ icon, title, description, action, className }: EmptyStateProps): React.JSX.Element {
  return (
    <div className={cn('card flex flex-col items-center justify-center overflow-hidden rounded-xl px-6 py-10 text-center', className)}>
      <IconTile size="md" className="mb-3">
        {icon}
      </IconTile>
      <p className="text-[13px] font-semibold text-ink">{title}</p>
      {description && <p className="mt-1 max-w-sm text-xs leading-relaxed text-ink-muted">{description}</p>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  )
}
