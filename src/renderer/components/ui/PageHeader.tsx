import type { ReactNode } from 'react'
import { cn } from '../../lib/utils'

interface PageHeaderProps {
  title: ReactNode
  description?: ReactNode
  actions?: ReactNode
  /** Small label above the title. */
  eyebrow?: ReactNode
  /** Rendered above the title, e.g. a back link. */
  leading?: ReactNode
  className?: string
}

export function PageHeader({ title, description, actions, eyebrow, leading, className }: PageHeaderProps): React.JSX.Element {
  return (
    <header className={cn('flex flex-wrap items-end justify-between gap-x-4 gap-y-2', className)}>
      <div className="min-w-0 flex-1">
        {leading && <div className="mb-2">{leading}</div>}
        {eyebrow && <p className="eyebrow mb-1">{eyebrow}</p>}
        <h1 className="truncate text-xl font-medium leading-tight text-ink">{title}</h1>
        {description && <p className="mt-1 max-w-2xl text-xs leading-relaxed text-content/50">{description}</p>}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-1.5">{actions}</div>}
    </header>
  )
}
