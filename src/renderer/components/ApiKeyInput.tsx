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
            <span className="inline-flex h-[1.125rem] items-center gap-1 rounded border border-success/25 bg-success/10 px-1.5 text-meta font-medium text-success animate-fade-in">
              <Check className="h-2.5 w-2.5" strokeWidth={3} />
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
              className="rounded-md px-2 py-1 text-caption text-ink-secondary transition-colors duration-feedback hover:bg-danger/10 hover:text-danger"
            >
              Remove key
            </button>
          )}
          {getKeyUrl && (
            <button
              type="button"
              onClick={() => getApi().shell.openPath(getKeyUrl)}
              className="inline-flex items-center gap-0.5 rounded-md px-2 py-1 text-caption text-ink-secondary transition-colors duration-feedback hover:bg-content/10 hover:text-content"
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
        leading={<KeyRound />}
        trailing={
          <Button
            variant="ghost"
            size="sm"
            iconOnly
            className="h-5 w-5 rounded"
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
