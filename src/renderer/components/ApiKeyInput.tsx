import { useId, useState } from 'react'
import { ArrowUpRight, Check, Eye, EyeOff, KeyRound } from 'lucide-react'
import { Field, TextInput } from './ui/Field'
import { Button } from './ui/Button'
import { getApi } from '../lib/ipc'

interface ApiKeyInputProps {
  label: string
  value: string
  configured?: boolean
  onChange: (value: string) => void
  onRemove?: () => void
  onBlur?: () => void
  placeholder?: string
  description?: string
  /** Where to create a key; shown as a link next to the label. */
  getKeyUrl?: string
  disabled?: boolean
}

export function ApiKeyInput({
  label,
  value,
  configured = false,
  onChange,
  onRemove,
  onBlur,
  placeholder,
  description,
  getKeyUrl,
  disabled = false
}: ApiKeyInputProps): React.JSX.Element {
  const id = useId()
  const [visible, setVisible] = useState(false)
  const hasValue = value.trim().length > 0

  return (
    <Field
      htmlFor={id}
      label={
        <>
          {label}
          {(hasValue || configured) && (
            <span className="inline-flex h-5 items-center gap-1 rounded-full bg-success/[0.12] px-2 text-2xs font-medium text-success shadow-[inset_0_0_0_1px_rgb(var(--success)/0.28)] animate-fade-in">
              <Check className="h-3 w-3" strokeWidth={3} />
              Added
            </span>
          )}
        </>
      }
      aside={
        <span className="inline-flex items-center gap-1">
          {configured && onRemove && (
            <button
              type="button"
              onClick={onRemove}
              className="rounded-full px-2 py-1 text-xs text-ink-subtle transition-colors hover:bg-danger/10 hover:text-danger"
            >
              Remove key
            </button>
          )}
          {getKeyUrl && (
            <button
              type="button"
              onClick={() => getApi().shell.openPath(getKeyUrl)}
              className="inline-flex items-center gap-0.5 rounded-full px-2 py-1 text-xs text-ink-muted transition-colors hover:bg-accent/10 hover:text-accent-hover"
            >
              Get a key
              <ArrowUpRight className="h-3 w-3" />
            </button>
          )}
        </span>
      }
      hint={description}
    >
      <TextInput
        id={id}
        type={visible ? 'text' : 'password'}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onBlur}
        placeholder={configured ? 'Saved securely. Paste a new key to replace.' : placeholder}
        disabled={disabled}
        mono={visible && hasValue}
        leading={<KeyRound className="h-3.5 w-3.5" />}
        trailing={
          <Button
            variant="ghost"
            size="sm"
            iconOnly
            className="h-7 w-7"
            aria-label={visible ? 'Hide key' : 'Show key'}
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => setVisible(!visible)}
            disabled={disabled}
            icon={visible ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
          />
        }
      />
    </Field>
  )
}
