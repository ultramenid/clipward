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
    <div className={cn('glass relative flex flex-col items-center justify-center overflow-hidden rounded-3xl px-6 py-10 text-center', className)}>
      <IconTile size="lg" className="relative mb-3">
        {icon}
      </IconTile>
      <p className="relative text-base font-semibold text-ink">{title}</p>
      {description && <p className="relative mt-1 max-w-sm text-sm text-ink-muted">{description}</p>}
      {action && <div className="relative mt-4">{action}</div>}
    </div>
  )
}
