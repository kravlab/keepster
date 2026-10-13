import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError, SessionEnded, fileContent, fileDiff, listRoots, timeline } from './api'

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
})
