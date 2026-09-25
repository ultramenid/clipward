import { forwardRef, type HTMLAttributes, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { cn } from '../../lib/utils'
import { LAYER_CLASS } from '../../lib/layers'

interface DialogProps extends HTMLAttributes<HTMLDivElement> {
  /** Mouse down on the dimmed backdrop (outside the panel). */
  onBackdropMouseDown?: () => void
  /** Stacking order; the update prompt sits above everything else. */
  layer?: 'dialog' | 'system'
  panelClassName?: string
  children: ReactNode
}

/**
 * Modal shell: a dimmed backdrop and a blurred popover-surface panel set in
 * the upper part of the window,
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
        'modal-backdrop fixed inset-0 flex items-start justify-center bg-black/40 px-3 pb-4 pt-[10vh]',
        layer === 'system' ? LAYER_CLASS.toast : LAYER_CLASS.dialog,
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
        className={cn('popover-surface modal-panel flex max-h-full w-full flex-col overflow-hidden rounded-2xl shadow-2xl focus:outline-none', panelClassName)}
        {...props}
      >
        {children}
      </div>
    </div>,
    document.body
  )
})

/** Footer bar for a dialog: actions right-aligned under a hairline. */
export function DialogFooter({ className, children }: { className?: string; children: ReactNode }): React.JSX.Element {
  return (
    <div className={cn('flex items-center justify-end gap-1.5 border-t border-stroke px-4 py-3', className)}>
      {children}
    </div>
  )
}
