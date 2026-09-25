import { forwardRef, type InputHTMLAttributes, type ReactNode, type TextareaHTMLAttributes } from 'react'
import { cn } from '../../lib/utils'

interface FieldProps {
  label: ReactNode
  hint?: ReactNode
  /** Right-aligned next to the label, e.g. a "Get a key" link. */
  aside?: ReactNode
  htmlFor?: string
  children: ReactNode
  className?: string
}

export function Field({ label, hint, aside, htmlFor, children, className }: FieldProps): React.JSX.Element {
  return (
    <div className={cn('space-y-1.5', className)}>
      <div className="flex items-center justify-between gap-3">
        <label htmlFor={htmlFor} className="flex items-center gap-2 text-label text-ink">
          {label}
        </label>
        {aside}
      </div>
      {children}
      {hint && <p className="text-caption text-ink-secondary">{hint}</p>}
    </div>
  )
}

/** Field surface shared by text inputs, selects and text areas: a hairline
 *  box that brightens on hover and takes the accent edge on focus. */
export const WELL =
  'field transition-[border-color,background-color] duration-feedback ease-out hover:border-content/20 ' +
  'focus-within:border-accent/70 focus-within:hover:border-accent/70'

interface TextInputProps extends InputHTMLAttributes<HTMLInputElement> {
  leading?: ReactNode
  trailing?: ReactNode
  inputSize?: 'sm' | 'md' | 'lg'
  mono?: boolean
}

export const TextInput = forwardRef<HTMLInputElement, TextInputProps>(function TextInput(
  { leading, trailing, inputSize = 'md', mono = false, className, ...props },
  ref
) {
  return (
    <div
      className={cn(
        'group/input relative flex items-center',
        'rounded-md',
        WELL,
        props.disabled && 'opacity-50',
        className
      )}
    >
      {leading && <span className="pointer-events-none pl-2 text-ink-secondary [&_svg]:h-3.5 [&_svg]:w-3.5">{leading}</span>}
      <input
        ref={ref}
        spellCheck={false}
        autoComplete="off"
        className={cn(
          'w-full min-w-0 bg-transparent text-ink placeholder:text-ink-tertiary focus:outline-none focus-visible:outline-none disabled:cursor-not-allowed',
          // Wrapper border adds 2px: sm totals 24px, md 28px, lg 32px, matching Button sizes.
          inputSize === 'lg' ? 'h-[30px] px-2.5 text-body' : inputSize === 'sm' ? 'h-[22px] px-2 text-caption' : 'h-[26px] px-2 text-caption',
          leading && 'pl-1.5',
          mono && 'font-mono text-caption tabular-nums',
          trailing && 'pr-1'
        )}
        {...props}
      />
      {trailing && <span className="flex shrink-0 items-center pr-1">{trailing}</span>}
    </div>
  )
})

export const TextArea = forwardRef<HTMLTextAreaElement, TextareaHTMLAttributes<HTMLTextAreaElement>>(function TextArea(
  { className, ...props },
  ref
) {
  return (
    <textarea
      ref={ref}
      className={cn(
        'block w-full resize-y rounded-md px-2.5 py-2 text-body text-ink placeholder:text-ink-tertiary focus:outline-none focus-visible:outline-none',
        'field transition-[border-color] duration-feedback ease-out hover:border-content/20 focus:border-accent/70 focus:hover:border-accent/70',
        className
      )}
      {...props}
    />
  )
})
