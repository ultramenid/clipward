/**
 * One z-index scale for everything that floats over the app chrome (adapted
 * from MonoCode, MIT). Panels and buttons inside a page stay below 50;
 * anything that escapes its pane picks a layer from here, so the order is
 * decided in one place instead of by whichever stray `z-50` was written last.
 *
 * Tailwind needs literal class names, so the classes are spelled out beside
 * the numbers.
 */
export const LAYER = {
  /** Anchored menus, dropdowns and hover cards. */
  popover: 80,
  /** Modal dialogs. */
  dialog: 90,
  /** Menus and tooltips opened from inside a modal dialog. */
  dialogPopover: 91,
  /** The update prompt and toasts, which outrank whatever they interrupt. */
  toast: 100
} as const

export const LAYER_CLASS: Record<keyof typeof LAYER, string> = {
  popover: 'z-[80]',
  dialog: 'z-[90]',
  dialogPopover: 'z-[91]',
  toast: 'z-[100]'
}
