import type { ReactNode } from 'react'
import { cn } from '../../lib/utils'

interface SettingRowProps {
  title: ReactNode
  description?: ReactNode
  /** The control on the right, usually a Switch. */
  control: ReactNode
  /** Inset tile (default) or a bare row inside an existing card. */
  bare?: boolean
  className?: string
}

/** A labelled setting: title and description on the left, control on the right. */
export function SettingRow({ title, description, control, bare = false, className }: SettingRowProps): React.JSX.Element {
  return (
    <div
      className={cn(
        'flex items-center justify-between gap-4',
        !bare && 'tile rounded-lg px-3 py-2.5',
        className
      )}
    >
      <div className="min-w-0">
        <p className="text-label text-ink">{title}</p>
        {description && <p className="mt-0.5 max-w-[65ch] text-caption text-ink-secondary">{description}</p>}
      </div>
      {control}
    </div>
  )
}
