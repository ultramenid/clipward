import type { Config } from 'tailwindcss'

// Tokens live in globals.css, redefined for light and dark schemes, so every
// colour supports Tailwind's `/alpha` modifier (e.g. `bg-content/5`,
// `border-content/10`). Neutrals are HSL components (Settings → Appearance
// tints them), accent and status colours RGB channels. Surfaces (.card, .tile,
// .field, .popover-surface, …) are component classes in globals.css; see DESIGN.md.
const channel = (name: string): string => `rgb(var(--${name}) / <alpha-value>)`
const neutral = (name: string): string => `hsl(var(--${name}) / <alpha-value>)`
const tint = (strength: string): string => `hsl(var(--content) / var(--${strength}))`

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
        }
      },
      fontFamily: {
        sans: [
          'system-ui',
          '-apple-system',
          'BlinkMacSystemFont',
          '"Segoe UI"',
          'Roboto',
          'Ubuntu',
          'Cantarell',
          '"Helvetica Neue"',
          'Geist',
          'sans-serif'
        ],
        mono: ['ui-monospace', 'SFMono-Regular', 'Menlo', 'Monaco', 'Consolas', '"Geist Mono"', '"Liberation Mono"', 'monospace']
      },
      fontSize: {
        '3xs': ['10px', '14px'],
        '2xs': ['11px', '15px'],
        xs: ['12px', '16px'],
        sm: ['13px', '18px'],
        base: ['14px', '20px'],
        lg: ['16px', '22px'],
        xl: ['20px', '25px'],
        '2xl': ['24px', '30px'],
        '3xl': ['30px', '36px']
      },
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
