import type { ReactNode } from 'react'
import { cn } from '../../lib/utils'

interface SettingRowProps {
  title: ReactNode
  description?: ReactNode
  /** The control on the right, usually a Switch. */
  control: ReactNode
  /** Inset glass tile (default) or a bare row inside an existing tile. */
  bare?: boolean
  className?: string
}

/** A labelled setting: title and description on the left, control on the right. */
export function SettingRow({ title, description, control, bare = false, className }: SettingRowProps): React.JSX.Element {
  return (
    <div
      className={cn(
        'flex items-center justify-between gap-3',
        !bare && 'glass-tile rounded-xl px-3 py-2',
        className
      )}
    >
      <div className="min-w-0">
        <p className="text-sm font-medium text-ink">{title}</p>
        {description && <p className="mt-0.5 text-2xs leading-relaxed text-ink-subtle">{description}</p>}
      </div>
      {control}
    </div>
  )
}
