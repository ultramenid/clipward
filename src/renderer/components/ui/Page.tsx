import type { ReactNode } from 'react'
import { cn } from '../../lib/utils'

const WIDTHS = {
  /** Progress, failure: one focused column. */
  focus: 'max-w-[720px]',
  /** Settings, Accounts: forms and lists. */
  narrow: 'max-w-[880px]',
  /** Create: main column plus settings rail. */
  default: 'max-w-[1280px]',
  /** Library, History, results: grids of media that fill wide screens. */
  wide: 'max-w-[1680px]'
} as const

/** Page column inside the scroll area. The layout already clears the title bar.
 *  Gutters tighten on small windows so content, not padding, gets the space. */
export function Page({ width = 'default', className, children }: {
  width?: keyof typeof WIDTHS
  className?: string
  children: ReactNode
}): React.JSX.Element {
  return <div className={cn('mx-auto w-full px-4 pb-10 pt-1 animate-fade-in sm:px-6 xl:px-8', WIDTHS[width], className)}>{children}</div>
}
