import { afterEach, describe, expect, it, vi } from 'vitest'
import { currentFor, fileVersion, snapshotChanges, untouchedAt } from './shown'

const ROOT = 'r'
const EARLIER = '01M4S82QCAT6S92580M1K8V9WJ'
const LATER = '01M4S82QFVD608VGXJFCBGS594'

/** A Snapshot as the timeline lists it; only the id matters here. */
const snap = (id: string) => ({ id, taken_at: '2026-10-12T23:45:01Z', trigger: 'manual', pinned: false })

/** A changed file as a diff summary lists it. */
const changed = (path: string, change = 'modified') => ({ path, change, binary: false, too_large: false, hunks: [] })

/** A plain file as a Snapshot's entries list it. */
const entry = (path: string) => ({ path, kind: 'file', binary: false, size: 10 })

/**
 * Answers each request with what `route` gives for its path and query, recording the requests as
 * `path?query`, origin dropped.
 */
const serve = (route: (path: string, query: URLSearchParams) => unknown) => {
  const asked: string[] = []
  vi.stubGlobal('fetch', (target: string) => {
    const url = new URL(target, 'http://x')
    asked.push(`${url.pathname}${url.search}`)
    const body = route(url.pathname, url.searchParams)
    return Promise.resolve(
      new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } }),
    )
  })
  return asked
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('a Snapshot’s changes', () => {
  it('are listed against a predecessor the list already knows, without asking for it', async () => {
    const asked = serve(() => ({ files: [changed('a.txt')] }))

    const changes = await snapshotChanges(ROOT, LATER, EARLIER)

    expect(changes).toEqual({
      to: LATER,
      from: EARLIER,
      files: [{ path: 'a.txt', oldPath: null, change: 'modified', card: null }],
    })
    expect(asked).toEqual([`/v1/roots/r/diff?from=${EARLIER}&to=${LATER}`])
  })

  it('ask for the predecessor when the list cannot tell', async () => {
    const asked = serve((path) =>
      path.endsWith('/snapshots') ? { snapshots: [snap(LATER), snap(EARLIER)] } : { files: [changed('a.txt')] },
    )

    const changes = await snapshotChanges(ROOT, LATER, undefined)

    expect(changes.from).toBe(EARLIER)
    expect(asked[0]).toMatch(/^\/v1\/roots\/r\/snapshots\?/)
    expect(asked[1]).toBe(`/v1/roots/r/diff?from=${EARLIER}&to=${LATER}`)
  })

  it('are everything a Root’s first Snapshot holds, directories aside', async () => {
    const asked = serve(() => ({ entries: [{ path: 'src', kind: 'dir' }, entry('src/a.rs')] }))

    const changes = await snapshotChanges(ROOT, EARLIER, null)

    expect(changes).toEqual({
      to: EARLIER,
      from: null,
      files: [{ path: 'src/a.rs', oldPath: null, change: 'added', card: null }],
    })
    expect(asked).toEqual([`/v1/roots/r/snapshots/${EARLIER}/entries?depth=infinity`])
  })
})

describe('a file’s version', () => {
  it('is its change against the Snapshot before', async () => {
    serve((path) =>
      path.endsWith('/snapshots')
        ? { snapshots: [snap(EARLIER)] }
        : { files: [changed('other.txt'), changed('a.txt', 'deleted')] },
    )

    expect(await fileVersion(ROOT, LATER, 'a.txt')).toEqual({
      to: LATER,
      path: 'a.txt',
      from: EARLIER,
      file: { path: 'a.txt', oldPath: null, change: 'deleted', card: null },
    })
  })

  it('is no file where the Snapshot did not change it', async () => {
    serve((path) => (path.endsWith('/snapshots') ? { snapshots: [snap(EARLIER)] } : { files: [changed('other.txt')] }))

    expect((await fileVersion(ROOT, LATER, 'a.txt')).file).toBeNull()
  })

  it('is the whole file in a Root’s first Snapshot', async () => {
    const asked = serve((path) => (path.endsWith('/snapshots') ? { snapshots: [] } : { entries: [entry('a.txt')] }))

    expect(await fileVersion(ROOT, EARLIER, 'a.txt')).toEqual({
      to: EARLIER,
      path: 'a.txt',
      from: null,
      file: { path: 'a.txt', oldPath: null, change: 'added', card: null },
    })
    expect(asked.some((a) => a.includes('/diff'))).toBe(false)
  })

  it('is no file in a Root’s first Snapshot that does not hold it', async () => {
    serve((path) => (path.endsWith('/snapshots') ? { snapshots: [] } : { entries: [entry('other.txt')] }))

    expect(await fileVersion(ROOT, EARLIER, 'a.txt')).toEqual({ to: EARLIER, path: 'a.txt', from: null, file: null })
  })
})

describe('a file the Snapshot did not change', () => {
  it('is shown as the Snapshot holds it', async () => {
    serve(() => ({ entries: [entry('src/a.rs')] }))

    expect(await untouchedAt(ROOT, LATER, 'src/a.rs')).toEqual({
      to: LATER,
      path: 'src/a.rs',
      file: { path: 'src/a.rs', oldPath: null, change: 'unchanged', card: null },
    })
  })

  it('is absent where the Snapshot holds nothing at its path', async () => {
    serve(() => ({ entries: [] }))

    expect((await untouchedAt(ROOT, LATER, 'gone.txt')).file.card).toBe('absent')
  })
})

describe('the result handed to the diff pane', () => {
  const found = { to: EARLIER, path: 'a.txt', from: null, file: null }

  it('is the one found for the Snapshot and file selected', () => {
    expect(currentFor(found, EARLIER, 'a.txt')).toBe(found)
  })

  it('is nothing while another Snapshot is selected, so the pane never mixes two', () => {
    expect(currentFor(found, LATER, 'a.txt')).toBeNull()
  })

  it('is nothing while another file is selected', () => {
    expect(currentFor(found, EARLIER, 'b.txt')).toBeNull()
  })

  it('ignores the file for a result that is not about one file', () => {
    const changes = { to: EARLIER, from: null, files: [] }

    expect(currentFor(changes, EARLIER, 'b.txt')).toBe(changes)
  })

  it('is nothing before anything is found or selected', () => {
    expect(currentFor(null, EARLIER, 'a.txt')).toBeNull()
    expect(currentFor(found, null, 'a.txt')).toBeNull()
  })
})
