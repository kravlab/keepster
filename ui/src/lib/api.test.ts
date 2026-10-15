import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  ApiError,
  SessionEnded,
  UnexpectedAnswer,
  fileContent,
  fileDiff,
  entryAt,
  listRoots,
  newestTouching,
  previousSnapshot,
  readPreferences,
  restore,
  searchContent,
  searchPaths,
  timeline,
  writePreferences,
} from './api'
import { exactPath } from './paths'

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

    expect(asked).toEqual(['/v1/roots/r/snapshots?limit=100&path=src/a%20b.rs'])
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
    expect(asked).toEqual(['/v1/roots/r/diff?from=f&to=t&path=dir/x.txt'])
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

  // Names that are not UTF-8 (ticket 74): `\xff.txt` and `\xfe.txt`, the same text on the wire.
  describe('a path that is not UTF-8', () => {
    const odd = exactPath('\uFFFD.txt', '_y50eHQ')
    const other = exactPath('\uFFFD.txt', '_i50eHQ')

    it('asks for it by its bytes', async () => {
      const asked = answer(() => json({ snapshots: [] }))

      await timeline('r', undefined, odd)

      expect(asked).toEqual(['/v1/roots/r/snapshots?limit=100&path=%FF.txt'])
    })

    it('finds its entry by its bytes, apart from one with the same text', async () => {
      answer(() =>
        json({
          entries: [
            { path: '\uFFFD.txt', path_bytes: '_i50eHQ', kind: 'file', size: 2 },
            { path: '\uFFFD.txt', path_bytes: '_y50eHQ', kind: 'file', size: 1 },
          ],
        }),
      )

      await expect(entryAt('r', 'S1', odd)).resolves.toMatchObject({ path: odd, size: 1 })
      await expect(entryAt('r', 'S1', other)).resolves.toMatchObject({ path: other, size: 2 })
    })

    it('holds a diff file by its bytes', async () => {
      answer(() =>
        json({
          files: [{ path: '\uFFFD.txt', path_bytes: '_y50eHQ', binary: false, too_large: false, hunks: [] }],
        }),
      )

      await expect(fileDiff('r', 'f', 't', odd)).resolves.toMatchObject({ path: odd })
    })

    it('restores it by its bytes, its answer keyed by them, and holds what comes back by them', async () => {
      const sent: unknown[] = []
      const plan = {
        dry_run: true,
        paths: [{ path: '\uFFFD.txt', path_bytes: '_y50eHQ', outcome: 'blocked', issues: [] }],
      }
      vi.stubGlobal('fetch', (_: string, init?: RequestInit) => {
        sent.push(JSON.parse(String(init?.body)))
        return Promise.resolve(json(plan))
      })

      const answered = await restore('r', {
        from: 'S1',
        paths: ['a.txt', odd],
        dry_run: true,
        confirm: [],
        resolutions: { 'a.txt': { write: true }, [odd]: { modes: 'keep' } },
        skip_unresolvable: false,
      })

      expect(sent).toEqual([
        {
          from: 'S1',
          paths: ['a.txt', '\uFFFD.txt'],
          paths_bytes: [null, '_y50eHQ'],
          dry_run: true,
          confirm: [],
          resolutions: { 'a.txt': { write: true } },
          resolutions_bytes: { _y50eHQ: { modes: 'keep' } },
          skip_unresolvable: false,
        },
      ])
      expect(answered).toMatchObject({ kind: 'done', restored: { paths: [{ path: odd }] } })
    })

    it('holds a path search hit by its bytes', async () => {
      answer(() =>
        json({
          hits: [
            { root: 'r', path: '\uFFFD.txt', path_bytes: '_y50eHQ', snapshots: 1, first: 'a', last: 'a', exists: true },
          ],
          truncated: false,
        }),
      )

      await expect(searchPaths('txt')).resolves.toMatchObject({ hits: [{ path: odd }] })
    })
  })

  describe('preferences', () => {
    const chosen = { theme: 'dark', syntax_line_limit: 5000, wrap_lines: true }

    it('reads them', async () => {
      const asked = answer(() => json(chosen))

      await expect(readPreferences()).resolves.toEqual(chosen)
      expect(asked).toEqual(['/v1/preferences'])
    })

    it('replaces them whole with a PUT and answers with what the daemon stored', async () => {
      const sent: { url: string; init?: RequestInit }[] = []
      vi.stubGlobal('fetch', (url: string, init?: RequestInit) => {
        sent.push({ url, init })
        return Promise.resolve(json(chosen))
      })

      await expect(writePreferences(chosen)).resolves.toEqual(chosen)
      expect(sent).toEqual([
        {
          url: '/v1/preferences',
          init: { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(chosen) },
        },
      ])
    })

    it("reports a refused value with the daemon's explanation", async () => {
      answer(() =>
        json(
          { title: 'Bad Request', status: 400, detail: 'it takes 1 to 10000000', code: 'preference_invalid' },
          400,
          'application/problem+json',
        ),
      )

      await expect(writePreferences({ ...chosen, syntax_line_limit: 0 })).rejects.toMatchObject({
        status: 400,
        code: 'preference_invalid',
        detail: 'it takes 1 to 10000000',
      })
    })
  })

  describe('search', () => {
    const ndjson = (...lines: string[]) =>
      new Response(lines.map((l) => `${l}\n`).join(''), { headers: { 'content-type': 'application/x-ndjson' } })

    it('asks for the paths a fragment finds in every Root', async () => {
      const found = { hits: [], truncated: false }
      const asked = answer(() => json(found))

      await expect(searchPaths('src/a b')).resolves.toEqual(found)
      expect(asked).toEqual(['/v1/search/paths?q=src%2Fa+b'])
    })

    it('streams a content search of the Roots named, record by record', async () => {
      const asked = answer(() => ndjson('{"type":"plan","roots":[]}', '{"type":"summary","versions_scanned":0}'))

      const records = []
      for await (const record of searchContent('needle', { roots: ['r1', 'r2'] })) records.push(record.type)

      expect(records).toEqual(['plan', 'summary'])
      expect(asked).toEqual(['/v1/search/content?q=needle&roots=r1%2Cr2'])
    })

    it('asks for every Root when told to', async () => {
      const asked = answer(() => ndjson('{"type":"summary","versions_scanned":0}'))

      for await (const record of searchContent('needle', 'all')) void record

      expect(asked).toEqual(['/v1/search/content?q=needle&all=true'])
    })

    it("reports a refused search with the daemon's explanation", async () => {
      answer(() =>
        json(
          { title: 'Bad Request', status: 400, detail: 'empty', code: 'query_invalid' },
          400,
          'application/problem+json',
        ),
      )

      const reading = async () => {
        for await (const record of searchContent('', 'all')) void record
      }
      await expect(reading()).rejects.toMatchObject({ status: 400, code: 'query_invalid' })
    })

    it('reports a line that is not a record as an answer it cannot read', async () => {
      answer(() => ndjson('{"type":"plan","roots":[]}', 'garbage'))

      const reading = async () => {
        for await (const record of searchContent('x', 'all')) void record
      }
      await expect(reading()).rejects.toBeInstanceOf(UnexpectedAnswer)
    })

    it('hands the abort signal to both requests', async () => {
      const signals: (AbortSignal | null | undefined)[] = []
      vi.stubGlobal('fetch', (_: string, init?: RequestInit) => {
        signals.push(init?.signal)
        return Promise.resolve(
          _.includes('content') ? ndjson('{"type":"summary"}') : json({ hits: [], truncated: false }),
        )
      })
      const controller = new AbortController()

      await searchPaths('x', controller.signal)
      for await (const record of searchContent('x', 'all', controller.signal)) void record

      expect(signals).toEqual([controller.signal, controller.signal])
    })
  })
})
