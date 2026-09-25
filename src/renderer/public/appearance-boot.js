// Paints Settings → Appearance before the app bundle runs, so a light theme,
// tint or custom accent doesn't flash in after launch. Reads the copy that
// src/renderer/lib/appearance.ts caches in localStorage and mirrors its
// applyAppearance; keep the two in step. The app shell's settings.json stays
// the source of truth: the page reapplies it once settings load. Interface
// scale needs nothing here, the shell sets the webview zoom before loading.
// Adapted from the inline bootstrap in MonoCode's index.html (MIT,
// Copyright (c) 2026 Nick; see THIRD_PARTY_NOTICES.md). A file, not an inline
// script, because the CSP allows only 'self' scripts.
;(function () {
  try {
    var raw = localStorage.getItem('clipward.appearance')
    var look = raw ? JSON.parse(raw) : {}
    if (!look || typeof look !== 'object') look = {}
    var root = document.documentElement
    var number = function (value, min, max, fallback) {
      var n = typeof value === 'number' && isFinite(value) ? value : fallback
      return Math.min(max, Math.max(min, n))
    }

    var theme = look.theme === 'light' || look.theme === 'system' ? look.theme : 'dark'
    if (theme === 'system') {
      theme = window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark'
    }
    root.classList.add(theme === 'light' ? 'theme-light' : 'theme-dark')

    root.style.setProperty('--theme-hue', String(Math.round(number(look.hue, 0, 360, 240))))
    root.style.setProperty('--theme-saturation', Math.round(number(look.saturation, 0, 100, 0)) + '%')
    root.style.setProperty('--theme-dark-lightness', Math.round(number(look.darkLightness, 0, 30, 9)) + '%')
    root.style.setProperty('--sidebar-opacity', String(number(look.sidebarOpacity, 0.15, 1, 0.85)))
    if (look.mainPaneGlass !== false) root.classList.add('glass-body')

    var accent = typeof look.accentColor === 'string' && /^#[0-9a-fA-F]{6}$/.test(look.accentColor) ? look.accentColor : null
    if (accent) {
      var channels = [1, 3, 5].map(function (at) { return parseInt(accent.slice(at, at + 2), 16) })
      var mix = function (target, amount) {
        return channels.map(function (c) { return Math.round(c + (target - c) * amount) }).join(' ')
      }
      var linear = channels.map(function (c) {
        var v = c / 255
        return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4)
      })
      var luminance = 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]
      root.classList.add('has-user-accent')
      root.style.setProperty('--user-accent', channels.join(' '))
      root.style.setProperty('--user-accent-ink', luminance > 0.179 ? '0 0 0' : '255 255 255')
      root.style.setProperty('--user-accent-fg-dark', mix(255, 0.3))
      root.style.setProperty('--user-accent-fg-light', mix(0, 0.25))
    }
  } catch {
    // No storage (private mode) or a damaged cache: the defaults paint until settings load.
  }
})()
