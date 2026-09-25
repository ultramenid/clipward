import lockupUrl from '../../../../resources/clipward-logo.svg'
import lockupLightUrl from '../../../../resources/clipward-logo-light.svg'
import iconUrl from '../../../../resources/clipward-icon-small.svg'
import markUrl from '../../../../resources/clipward-mark.svg'
import { cn } from '../../lib/utils'
import { useColorScheme } from '../../lib/appearance'

interface ClipwardLogoProps {
  /**
   * lockup: Clipward mark + "Clipward" wordmark (artwork is for dark surfaces).
   * icon:   the app icon tile, a clip/play emblem in Clipward gold and cyan.
   * mark:   the mark alone, for tight spaces such as the collapsed sidebar.
   */
  variant?: 'lockup' | 'icon' | 'mark'
  /** Size by height (e.g. "h-6"); width follows the artwork. */
  className?: string
  alt?: string
}

/**
 * Clipward brand artwork. Regenerate with scripts/icon/build-logo.py (all SVGs)
 * then scripts/icon/build-icons.sh (raster app icons).
 */
export function ClipwardLogo({ variant = 'lockup', className, alt = 'Clipward' }: ClipwardLogoProps): React.JSX.Element {
  // The lockup's wordmark is white; the light scheme (Settings → Appearance →
  // Theme, or the OS) gets the dark-ink version.
  const light = useColorScheme() === 'light'
  return (
    <img
      src={variant === 'icon' ? iconUrl : variant === 'mark' ? markUrl : light ? lockupLightUrl : lockupUrl}
      alt={alt}
      draggable={false}
      className={cn('w-auto shrink-0 select-none', className)}
    />
  )
}
