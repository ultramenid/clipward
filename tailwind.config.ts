import type { Config } from 'tailwindcss'

// Tokens live in globals.css, redefined for light and dark schemes, so every
// colour supports Tailwind's `/alpha` modifier (e.g. `bg-content/5`,
// `border-content/10`). Neutrals are HSL components (Settings → Appearance
// tints them), accent and status colours RGB channels. Surfaces (.card, .tile,
// .field, .popover-surface, …) are component classes in globals.css; see DESIGN.md.
const channel = (name: string): string => `rgb(var(--${name}) / <alpha-value>)`
const neutral = (name: string): string => `hsl(var(--${name}) / <alpha-value>)`
const tint = (strength: string): string => `hsl(var(--content) / var(--${strength}))`

// ─── Typography ────────────────────────────────────────────────────────────
// The type scale. Sizes and line heights are rem CSS variables (globals.css,
// "Typography" block) so an interface-scale setting that changes the root
// font size, or zooms the webview, scales every piece of text. Each token
// pairs a size with its line height, weight and tracking; `font-*` and
// `leading-*` utilities still override them. See DESIGN.md → Typography.
const type = (name: string, fontWeight: string, letterSpacing = '0em'): [string, { lineHeight: string; fontWeight: string; letterSpacing: string }] => [
  `var(--text-${name})`,
  { lineHeight: `var(--text-${name}-lh)`, fontWeight, letterSpacing }
]

const TYPE_SCALE = {
  display: type('display', '600', '-0.02em'), // 28/32 · big live numbers (progress)
  title: type('title', '600', '-0.012em'), // 20/26 · page titles, stat values
  heading: type('heading', '600', '-0.006em'), // 15/20 · dialog and hero-card titles
  section: type('section', '600'), // 13/18 · card and section titles
  label: type('label', '500'), // 13/18 · row, option and field labels, nav
  body: type('body', '400'), // 13/20 · running text, values
  caption: type('caption', '400'), // 12/18 · descriptions, hints, control text
  meta: type('meta', '400', '0.005em'), // 11/16 · metadata, badges, chips, shortcuts
  micro: type('micro', '500', '0.01em'), // 10/14 · step numbers, counters inside tiny marks
  // Legacy sizes, in rem so they scale too. New code uses the names above.
  '3xs': ['0.625rem', '0.875rem'],
  '2xs': ['0.6875rem', '1rem'],
  xs: ['0.75rem', '1rem'],
  sm: ['0.8125rem', '1.125rem'],
  base: ['0.875rem', '1.25rem'],
  lg: ['1rem', '1.375rem'],
  xl: ['1.25rem', '1.5625rem'],
  '2xl': ['1.5rem', '1.875rem'],
  '3xl': ['1.875rem', '2.25rem']
}
// The token names are listed again in TYPE_TOKENS (src/renderer/lib/utils.ts) for tailwind-merge.

const TYPE_FAMILIES = {
  sans: ['var(--font-sans)'],
  mono: ['var(--font-mono)']
}

// Text emphasis: the content colour at strengths tuned per scheme so both
// pass WCAG AA on the base and on cards (see globals.css).
const TYPE_COLORS = {
  'ink-secondary': 'hsl(var(--content) / var(--ink-secondary-alpha))',
  'ink-tertiary': 'hsl(var(--content) / var(--ink-tertiary-alpha))'
}
// ───────────────────────────────────────────────────────────────────────────

const config: Config = {
  darkMode: 'class',
  content: ['./src/renderer/**/*.{ts,tsx,html}'],
  theme: {
    extend: {
      colors: {
        canvas: neutral('bg'),
        sidebar: neutral('sidebar'),
        elevated: neutral('elevated'),
        content: neutral('content'),
        ink: {
          DEFAULT: neutral('content'),
          muted: neutral('ink-muted'),
          subtle: neutral('ink-subtle'),
          faint: neutral('ink-faint')
        },
        // Structural hairlines: border-stroke, divide-stroke, bg-stroke.
        stroke: tint('stroke-strength'),
        // Interactive fills. Prefer these over raw content opacity for
        // selected and raised UI: light mode uses gentler strengths.
        selection: {
          subtle: tint('selection-subtle'),
          DEFAULT: tint('selection'),
          strong: tint('selection-strong'),
          hover: tint('selection-hover')
        },
        accent: {
          DEFAULT: channel('accent'),
          fg: channel('accent-fg'),
          ink: channel('accent-ink')
        },
        success: channel('success'),
        danger: channel('danger'),
        warning: channel('warning'),
        brand: {
          gold: '#ffd500'
        },
        ...TYPE_COLORS
      },
      // Typography: see the block at the top of this file and DESIGN.md.
      fontFamily: TYPE_FAMILIES,
      fontSize: TYPE_SCALE,
      // Small, quiet corners. Controls and rows are `rounded-md`, menus rows
      // `rounded-lg`, cards and popovers `rounded-xl`, dialogs `rounded-2xl`.
      borderRadius: {
        sm: '4px',
        DEFAULT: '4px',
        md: '6px',
        lg: '8px',
        xl: '12px',
        '2xl': '16px'
      },
      spacing: {
        '7.5': '1.875rem',
        '4.5': '1.125rem'
      },
      boxShadow: {
        pop: '0 20px 25px -5px rgb(0 0 0 / 0.25), 0 8px 10px -6px rgb(0 0 0 / 0.2)',
        'accent-ring': '0 0 0 1px rgb(var(--accent) / 0.9)'
      },
      transitionDuration: {
        feedback: '120ms'
      },
      transitionTimingFunction: {
        out: 'cubic-bezier(0.22, 1, 0.36, 1)',
        enter: 'cubic-bezier(0.16, 1, 0.3, 1)'
      },
      keyframes: {
        'fade-in': {
          from: { opacity: '0', transform: 'translateY(4px)' },
          to: { opacity: '1', transform: 'translateY(0)' }
        },
        fade: {
          from: { opacity: '0' },
          to: { opacity: '1' }
        },
        'bar-grow': {
          from: { transform: 'scaleX(0)' },
          to: { transform: 'scaleX(1)' }
        },
        'pulse-ring': {
          '0%': { boxShadow: '0 0 0 0 rgb(var(--accent) / 0.45)' },
          '100%': { boxShadow: '0 0 0 8px rgb(var(--accent) / 0)' }
        }
      },
      animation: {
        // `backwards`, not `both`: a transform left on after entry would make the
        // element the containing block for fixed descendants.
        'fade-in': 'fade-in 200ms cubic-bezier(0.16, 1, 0.3, 1) backwards',
        fade: 'fade 160ms ease-out backwards',
        'bar-grow': 'bar-grow 600ms cubic-bezier(0.16, 1, 0.3, 1) backwards',
        'pulse-ring': 'pulse-ring 1.8s cubic-bezier(0.16, 1, 0.3, 1) infinite'
      }
    }
  },
  plugins: []
}

export default config
