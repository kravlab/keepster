/**
 * The web UI's preferences on the page's side (ticket 66): the defaults, the paint cache, and painting a
 * theme.
 *
 * The daemon is where preferences live; `localStorage` only remembers the last values it answered, so the
 * first paint of a new page load is already in the right palette instead of flashing the default one. A
 * cache that is lost, blocked or holds anything unexpected is simply no cache: the daemon's answer follows
 * a moment later.
 */
import type { Preferences } from './wire'

/** The palettes this page paints. */
export const THEMES = ['system', 'light', 'dark'] as const
/** One of [`THEMES`]. */
export type Theme = (typeof THEMES)[number]

/** The preferences before anything is chosen, as the daemon has them (ticket 16). */
export const DEFAULT_PREFERENCES: Readonly<Preferences> = {
  theme: 'system',
  syntax_line_limit: 20_000,
  wrap_lines: false,
}

/** The values `syntax_line_limit` may take, as the daemon checks them. */
export const SYNTAX_LINE_LIMITS = { min: 1, max: 10_000_000 } as const

/** The `localStorage` key of the paint cache. */
const CACHE_KEY = 'keepster.preferences'

/** The part of `Storage` the cache uses, so that a test can hand in its own. */
export type CacheStorage = Pick<Storage, 'getItem' | 'setItem'>

/** The page's root element as painting a theme touches it. */
export type ThemeTarget = Pick<Element, 'setAttribute' | 'removeAttribute'>

/** `value` when it has the shape of preferences this page can paint, otherwise `null`. */
function preferencesIn(value: unknown): Preferences | null {
  if (typeof value !== 'object' || value === null) return null
  const { theme, syntax_line_limit, wrap_lines } = value as Record<string, unknown>
  const known = typeof theme === 'string' && (THEMES as readonly string[]).includes(theme)
  const limit =
    typeof syntax_line_limit === 'number' &&
    Number.isInteger(syntax_line_limit) &&
    syntax_line_limit >= SYNTAX_LINE_LIMITS.min &&
    syntax_line_limit <= SYNTAX_LINE_LIMITS.max
  if (!known || !limit || typeof wrap_lines !== 'boolean') return null
  return { theme, syntax_line_limit, wrap_lines }
}

/** The browser's `localStorage`, or nothing where reaching it throws (storage blocked for the site). */
export function browserStorage(): CacheStorage | undefined {
  try {
    return globalThis.localStorage
  } catch {
    return undefined
  }
}

/** The preferences the daemon last answered, as the cache remembers them; `null` when it holds none. */
export function readCache(storage: CacheStorage | undefined): Preferences | null {
  try {
    const held = storage?.getItem(CACHE_KEY)
    return held ? preferencesIn(JSON.parse(held)) : null
  } catch {
    return null
  }
}

/** Remembers the daemon's answer for the next page load's first paint. A cache that cannot be written is skipped. */
export function writeCache(storage: CacheStorage | undefined, preferences: Preferences): void {
  try {
    storage?.setItem(CACHE_KEY, JSON.stringify(preferences))
  } catch {
    // Only the next first paint is affected: it shows the default palette until the daemon answers.
  }
}

/**
 * Paints `theme`: `light` and `dark` are pinned with `data-theme` on the root element, anything else
 * (`system`, or a palette this page does not know) leaves the stylesheet to follow the system's setting.
 */
export function applyTheme(root: ThemeTarget, theme: string): void {
  if (theme === 'light' || theme === 'dark') root.setAttribute('data-theme', theme)
  else root.removeAttribute('data-theme')
}
