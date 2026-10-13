import { describe, expect, it } from 'vitest'
import { DEFAULT_PREFERENCES, applyTheme, readCache, writeCache } from './preferences'

/** A `Storage` held in memory; `failing` makes every call throw, as a browser with storage blocked does. */
const storage = (initial: Record<string, string> = {}, failing = false) => {
  const items = new Map(Object.entries(initial))
  return {
    items,
    getItem(key: string) {
      if (failing) throw new Error('storage is blocked')
      return items.get(key) ?? null
    },
    setItem(key: string, value: string) {
      if (failing) throw new Error('storage is blocked')
      items.set(key, value)
    },
  }
}

/** The root element as `applyTheme` sees it: its attributes. */
const element = () => {
  const attributes = new Map<string, string>()
  return {
    attributes,
    setAttribute: (name: string, value: string) => void attributes.set(name, value),
    removeAttribute: (name: string) => void attributes.delete(name),
  }
}

const chosen = { theme: 'dark', syntax_line_limit: 5000, wrap_lines: true } as const

describe('the preferences paint cache', () => {
  it('reads back what was written', () => {
    const cache = storage()

    writeCache(cache, chosen)

    expect(readCache(cache)).toEqual(chosen)
  })

  it('holds nothing before anything is written', () => {
    expect(readCache(storage())).toBeNull()
  })

  it.each([
    ['not JSON', '{'],
    ['not an object', '"dark"'],
    ['an unknown theme', JSON.stringify({ ...chosen, theme: 'blue' })],
    ['a limit that is not a positive whole number', JSON.stringify({ ...chosen, syntax_line_limit: 0 })],
    ['a limit over the largest the daemon takes', JSON.stringify({ ...chosen, syntax_line_limit: 10_000_001 })],
    ['a missing key', JSON.stringify({ theme: 'dark', syntax_line_limit: 5000 })],
  ])('treats a cache holding %s as no cache', (_, held) => {
    expect(readCache(storage({ 'keepster.preferences': held }))).toBeNull()
  })

  it('is no cache, and no failure, where storage is blocked', () => {
    const blocked = storage({}, true)

    expect(() => writeCache(blocked, chosen)).not.toThrow()
    expect(readCache(blocked)).toBeNull()
    expect(readCache(undefined)).toBeNull()
  })
})

describe('the defaults', () => {
  it('follow the system palette, highlight up to 20 000 characters a line and do not wrap', () => {
    expect(DEFAULT_PREFERENCES).toEqual({ theme: 'system', syntax_line_limit: 20_000, wrap_lines: false })
  })
})

describe('applying a theme', () => {
  it('pins a light or dark palette on the root element', () => {
    const root = element()

    applyTheme(root, 'dark')
    expect(root.attributes.get('data-theme')).toBe('dark')

    applyTheme(root, 'light')
    expect(root.attributes.get('data-theme')).toBe('light')
  })

  it('leaves the palette to the system for `system`', () => {
    const root = element()
    applyTheme(root, 'dark')

    applyTheme(root, 'system')

    expect(root.attributes.has('data-theme')).toBe(false)
  })
})
