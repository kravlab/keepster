import { afterEach, describe, expect, it, vi } from 'vitest'
import { PreferencesState } from './preferences.svelte'

const json = (body: unknown, status = 200, type = 'application/json') =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': type } })

const refused = (detail: string) =>
  json({ title: 'Bad Request', status: 400, detail, code: 'preference_invalid' }, 400, 'application/problem+json')

/** A `Storage` held in memory. */
const storage = (initial: Record<string, string> = {}) => {
  const items = new Map(Object.entries(initial))
  return {
    items,
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => void items.set(key, value),
  }
}

/** The root element, keeping only the theme attribute. */
const element = () => {
  const root = {
    theme: undefined as string | undefined,
    setAttribute: (_: string, value: string) => void (root.theme = value),
    removeAttribute: () => void (root.theme = undefined),
  }
  return root
}

/** Answers each request in turn with the next of `answers`, recording method and URL. */
const serve = (...answers: (() => Response | Promise<Response>)[]) => {
  const asked: string[] = []
  vi.stubGlobal('fetch', (url: string, init?: RequestInit) => {
    asked.push(`${init?.method ?? 'GET'} ${url}`)
    const next = answers.shift()
    if (!next) throw new Error(`no answer left for ${url}`)
    return Promise.resolve(next())
  })
  return asked
}

const dark = { theme: 'dark', syntax_line_limit: 5000, wrap_lines: true }
const light = { theme: 'light', syntax_line_limit: 5000, wrap_lines: true }

const text = (e: unknown) => (e instanceof Error ? e.message : String(e))

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the preferences the page shows', () => {
  it('start from the paint cache, then take the daemon’s answer and remember it', async () => {
    const cache = storage({ 'keepster.preferences': JSON.stringify(light) })
    const root = element()
    serve(() => json(dark))

    const preferences = new PreferencesState(text, { storage: cache, root })
    expect(preferences.current).toEqual(light)
    expect(root.theme).toBe('light')

    await preferences.load()

    expect(preferences.current).toEqual(dark)
    expect(root.theme).toBe('dark')
    expect(JSON.parse(cache.items.get('keepster.preferences') ?? '')).toEqual(dark)
  })

  it('start from the defaults without a cache', () => {
    const root = element()

    const preferences = new PreferencesState(text, { storage: storage(), root })

    expect(preferences.current).toEqual({ theme: 'system', syntax_line_limit: 20_000, wrap_lines: false })
    expect(root.theme).toBeUndefined()
  })

  it('show a change at once and keep what the daemon stored', async () => {
    const cache = storage()
    const root = element()
    const asked = serve(
      () => json(dark),
      () => json(light),
    )
    const preferences = new PreferencesState(text, { storage: cache, root })
    await preferences.load()

    const saving = preferences.change({ theme: 'light' })
    expect(root.theme).toBe('light')
    await saving

    expect(asked).toEqual(['GET /v1/preferences', 'PUT /v1/preferences'])
    expect(preferences.current).toEqual(light)
    expect(preferences.error).toBeNull()
    expect(JSON.parse(cache.items.get('keepster.preferences') ?? '')).toEqual(light)
  })

  it('go back to what the daemon last accepted when a change is refused, and say why', async () => {
    const cache = storage()
    const root = element()
    serve(
      () => json(dark),
      () => refused('it takes 1 to 10000000'),
    )
    const preferences = new PreferencesState(text, { storage: cache, root })
    await preferences.load()

    await preferences.change({ theme: 'light', syntax_line_limit: 0 })

    expect(preferences.current).toEqual(dark)
    expect(root.theme).toBe('dark')
    expect(preferences.error).toEqual({ when: 'save', text: 'it takes 1 to 10000000' })
    expect(JSON.parse(cache.items.get('keepster.preferences') ?? '')).toEqual(dark)
  })

  it('clear the failure once a later change is stored', async () => {
    serve(
      () => json(dark),
      () => refused('no'),
      () => json(light),
    )
    const preferences = new PreferencesState(text, { storage: storage(), root: element() })
    await preferences.load()
    await preferences.change({ syntax_line_limit: 0 })

    await preferences.change({ theme: 'light' })

    expect(preferences.error).toBeNull()
    expect(preferences.current).toEqual(light)
  })

  it('let the newest change decide when an older answer arrives after it', async () => {
    let answerFirst: (r: Response) => void = () => {}
    serve(
      () => json(dark),
      () => new Promise<Response>((resolve) => (answerFirst = resolve)),
      () => json(light),
    )
    const root = element()
    const preferences = new PreferencesState(text, { storage: storage(), root })
    await preferences.load()

    const first = preferences.change({ syntax_line_limit: 0 })
    await preferences.change({ theme: 'light' })
    answerFirst(refused('no'))
    await first

    expect(preferences.current).toEqual(light)
    expect(root.theme).toBe('light')
    expect(preferences.error).toBeNull()
  })

  it('keep what they show when the daemon cannot be read, and say why', async () => {
    serve(() => json({ title: 'Bad Gateway', status: 502, detail: 'down', code: 'x' }, 502, 'application/problem+json'))
    const preferences = new PreferencesState(text, {
      storage: storage({ 'keepster.preferences': JSON.stringify(dark) }),
      root: element(),
    })

    await preferences.load()

    expect(preferences.current).toEqual(dark)
    expect(preferences.error).toEqual({ when: 'read', text: 'down' })
  })
})
