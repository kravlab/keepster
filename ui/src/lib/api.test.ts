import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  ApiError,
  SessionEnded,
  fileContent,
  fileDiff,
  entryAt,
  listRoots,
  newestTouching,
  previousSnapshot,
  restore,
  timeline,
} from './api'

/** Answers every request with `response`, recording the URLs asked for. */
const answer = (response: () => Response) => {
  const asked: string[] = []
  vi.stubGlobal('fetch', (url: string) => {
    asked.push(url)
    return Promise.resolve(response())
  })
  return asked
}

const json = (body: unknown, status = 200, type = 'application/json') =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': type } })

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the /v1 client', () => {
  it('reports a 401 as the session having ended', async () => {
    answer(() => json({ title: 'Unauthorized', status: 401, detail: 'no session', code: 'unauthorized' }, 401))

    await expect(listRoots()).rejects.toBeInstanceOf(SessionEnded)
  })

  it("reports any other failure with the daemon's own explanation", async () => {
    answer(() =>
      json(
        { title: 'Not Found', status: 404, detail: 'no Snapshot s', code: 'snapshot_not_found' },
        404,
        'application/problem+json',
      ),
    )

    const failure = await timeline('r', undefined).catch((e: unknown) => e)
    expect(failure).toBeInstanceOf(ApiError)
    expect(failure).toMatchObject({ status: 404, code: 'snapshot_not_found', detail: 'no Snapshot s' })
  })

  it('reports a failure without a problem body by its status', async () => {
    answer(() => new Response('gateway', { status: 502 }))

    const failure = await listRoots().catch((e: unknown) => e)
    expect(failure).toBeInstanceOf(ApiError)
    expect(failure).toMatchObject({ status: 502, detail: undefined })
  })

  it('asks for a timeline page by Root and cursor, both escaped', async () => {
    const asked = answer(() => json({ snapshots: [] }))

    await timeline('a/b', 'c d')

    expect(asked).toEqual(['/v1/roots/a%2Fb/snapshots?limit=100&cursor=c+d'])
  })

  it('asks for the timeline of one path when filtered by it', async () => {
    const asked = answer(() => json({ snapshots: [] }))

    await timeline('r', undefined, 'src/a b.rs')

    expect(asked).toEqual(['/v1/roots/r/snapshots?limit=100&path=src%2Fa+b.rs'])
  })

  it('finds the newest Snapshot that changed a path, or none', async () => {
    const asked = answer(() => json({ snapshots: [{ id: 'S9' }] }))

    await expect(newestTouching('r', 'a.txt')).resolves.toBe('S9')
    expect(asked).toEqual(['/v1/roots/r/snapshots?limit=1&path=a.txt'])

    answer(() => json({ snapshots: [] }))
    await expect(newestTouching('r', 'a.txt')).resolves.toBeNull()
  })

  // Real ids: …594 was taken at 2026-10-12T23:45:01.435Z, …V9WJ at .322Z; …595 shares …594's millisecond.
  const EARLIER = '01M4S82QCAT6S92580M1K8V9WJ'
  const FIRST_IN_MS = '01M4S82QFVD608VGXJFCBGS594'
  const SECOND_IN_MS = '01M4S82QFVD608VGXJFCBGS595'

  it('finds the Snapshot before one among those taken at or before its moment', async () => {
    const asked = answer(() => json({ snapshots: [{ id: FIRST_IN_MS }, { id: EARLIER }] }))

    await expect(previousSnapshot('r', FIRST_IN_MS)).resolves.toBe(EARLIER)
    expect(asked).toEqual(['/v1/roots/r/snapshots?limit=3&before=2026-10-12T23%3A45%3A01.435Z'])
  })

  it('finds it in the same millisecond, where a stopping daemon’s flush can put a second Snapshot', async () => {
    const page = { snapshots: [{ id: SECOND_IN_MS }, { id: FIRST_IN_MS }, { id: EARLIER }] }
    answer(() => json(page))

    await expect(previousSnapshot('r', SECOND_IN_MS)).resolves.toBe(FIRST_IN_MS)
    await expect(previousSnapshot('r', FIRST_IN_MS)).resolves.toBe(EARLIER)
  })

  it("finds none before a Root's first Snapshot", async () => {
    answer(() => json({ snapshots: [{ id: FIRST_IN_MS }] }))

    await expect(previousSnapshot('r', FIRST_IN_MS)).resolves.toBeNull()
  })

  it("finds a path's entry among its directory's own children, or none", async () => {
    const entry = { path: 'src/a.rs', kind: 'file' }
    const asked = answer(() => json({ entries: [{ path: 'src/b.rs', kind: 'file' }, entry] }))

    await expect(entryAt('r', 'S1', 'src/a.rs')).resolves.toEqual(entry)
    await expect(entryAt('r', 'S1', 'src/c.rs')).resolves.toBeNull()
    await entryAt('r', 'S1', 'top.txt')
    expect(asked).toEqual([
      '/v1/roots/r/snapshots/S1/entries?path=src',
      '/v1/roots/r/snapshots/S1/entries?path=src',
      '/v1/roots/r/snapshots/S1/entries',
    ])
  })

  it('answers a file diff with that one file', async () => {
    const file = { path: 'x.txt', change: 'modified', binary: false, too_large: false, hunks: [] }
    const asked = answer(() => json({ from: 'f', to: 't', files: [file] }))

    await expect(fileDiff('r', 'f', 't', 'dir/x.txt')).resolves.toEqual(file)
    expect(asked).toEqual(['/v1/roots/r/diff?from=f&to=t&path=dir%2Fx.txt'])
  })

  it('reads content as UTF-8 the way the daemon renders hunks: BOM kept, bad bytes replaced', async () => {
    const asked = answer(() => new Response(new Uint8Array([0xef, 0xbb, 0xbf, 0x61, 0xff, 0x0a])))

    await expect(fileContent('r', 's', 'a.txt')).resolves.toBe('\uFEFFa\uFFFD\n')
    expect(asked).toEqual(['/v1/roots/r/snapshots/s/content?path=a.txt'])
  })

  describe('restore', () => {
    const request = {
      from: 'S1',
      paths: ['a.txt'],
      dry_run: true,
      confirm: [],
      resolutions: {},
      skip_unresolvable: false,
    }

    it('posts the request as JSON and answers with what was done', async () => {
      const sent: { url: string; init?: RequestInit }[] = []
      const done = { dry_run: true, paths: [{ path: 'a.txt', outcome: 'restored' }] }
      vi.stubGlobal('fetch', (url: string, init?: RequestInit) => {
        sent.push({ url, init })
        return Promise.resolve(json(done))
      })

      await expect(restore('r', request)).resolves.toEqual({ kind: 'done', restored: done })
      expect(sent).toEqual([
        {
          url: '/v1/roots/r/restore',
          init: { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(request) },
        },
      ])
    })

    it('answers a refusal that needs confirmations with the plan it gave', async () => {
      const paths = [{ path: 'a.txt', outcome: 'blocked', issues: [{ reason: 'parent_missing' }] }]
      answer(() =>
        json(
          { title: 'Bad Request', status: 400, detail: 'needs', code: 'confirmation_required', paths },
          400,
          'application/problem+json',
        ),
      )

      await expect(restore('r', { ...request, dry_run: false })).resolves.toEqual({ kind: 'blocked', paths })
    })

    it('reports any other refusal as a failure', async () => {
      answer(() =>
        json(
          { title: 'Conflict', status: 409, detail: 'unavailable', code: 'root_unavailable' },
          409,
          'application/problem+json',
        ),
      )

      await expect(restore('r', request)).rejects.toMatchObject({ status: 409, code: 'root_unavailable' })
    })
  })
})
