import { forwardRef, type HTMLAttributes, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { cn } from '../../lib/utils'

interface DialogProps extends HTMLAttributes<HTMLDivElement> {
  /** Mouse down on the dimmed backdrop (outside the panel). */
  onBackdropMouseDown?: () => void
  /** Stacking order; the update prompt sits above everything else. */
  layer?: 'dialog' | 'system'
  panelClassName?: string
  children: ReactNode
}

/**
 * Modal shell: a dimmed, softly blurred backdrop and a thick-glass panel,
 * portalled to <body> so no transformed ancestor (page entry animations,
 * hover lifts) can become its containing block. Focus management and Escape
 * stay with the caller; the forwarded ref is the panel (role="dialog").
 */
export const Dialog = forwardRef<HTMLDivElement, DialogProps>(function Dialog(
  { onBackdropMouseDown, layer = 'dialog', panelClassName, className, children, ...props },
  ref
) {
  return createPortal(
    <div
      className={cn(
        'no-drag fixed inset-0 flex items-center justify-center bg-black/45 p-4 backdrop-blur-[6px] animate-fade',
        layer === 'system' ? 'z-[100]' : 'z-[90]',
        className
      )}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onBackdropMouseDown?.()
      }}
    >
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        tabIndex={-1}
        className={cn('glass-thick flex max-h-full w-full flex-col overflow-hidden rounded-3xl animate-pop-in focus:outline-none', panelClassName)}
        {...props}
      >
        {children}
      </div>
    </div>,
    document.body
  )
})

/** Footer bar for a dialog: actions right-aligned on a slightly darker strip. */
export function DialogFooter({ className, children }: { className?: string; children: ReactNode }): React.JSX.Element {
  return (
    <div className={cn('flex items-center justify-end gap-2 border-t border-white/[0.07] bg-black/20 px-4 py-3', className)}>
      {children}
    </div>
  )
}
