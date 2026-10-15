import { afterEach, describe, expect, it, vi } from 'vitest'
import { TimelinePages } from './pages.svelte'
import { FakeSource, following, stopFollowing, stream } from './testing'

/** A Snapshot as the timeline lists it; only the id matters here. */
const snap = (id: string) => ({ id, taken_at: '2026-10-12T10:00:00Z', trigger: 'manual', pinned: false })

/** A timeline page holding `ids`, with `next` as its cursor. */
const page = (ids: string[], next?: string) => ({ snapshots: ids.map(snap), next_cursor: next })

/**
 * Answers each timeline request with the page `pages` gives for its cursor (`''` for the first page),
 * recording the URLs asked for. A page given as a promise is answered when it settles.
 */
const serve = (pages: (cursor: string) => unknown) => {
  const asked: string[] = []
  vi.stubGlobal('fetch', async (url: string) => {
    asked.push(url)
    const cursor = new URL(url, 'http://x').searchParams.get('cursor') ?? ''
    const body = await pages(cursor)
    return new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } })
  })
  return asked
}

const ids = (pages: TimelinePages) => pages.snapshots.map((s) => s.id)

afterEach(() => {
  stopFollowing()
  vi.unstubAllGlobals()
})

describe('a view mode’s timeline pages', () => {
  // Ids order as Snapshot ids do, by when they were taken: `s3` is newer than `s2`.
  it('loads the newest page, then older pages until the wanted Snapshot is among them', async () => {
    serve((cursor) => ({ '': page(['s5', 's4'], 'p2'), p2: page(['s3'], 'p3'), p3: page(['s2']) })[cursor])
    const pages = new TimelinePages('r', String)

    await pages.reload('s3')

    expect(ids(pages)).toEqual(['s5', 's4', 's3'])
    expect(pages.cursor).toBe('p3')
    expect(pages.loaded).toBe(true)
    expect(pages.loading).toBe(false)
  })

  it('stops at the first Snapshot older than the wanted one when the list does not hold it', async () => {
    serve((cursor) => ({ '': page(['s6', 's5'], 'p2'), p2: page(['s3', 's2'], 'p3'), p3: page(['s1']) })[cursor])
    const pages = new TimelinePages('r', String)

    await pages.reload('s4')

    expect(ids(pages)).toEqual(['s6', 's5', 's3', 's2'])
    expect(pages.cursor).toBe('p3')
  })

  it('stops at the oldest page when every Snapshot is newer than the wanted one', async () => {
    serve((cursor) => ({ '': page(['s3'], 'p2'), p2: page(['s2']) })[cursor])
    const pages = new TimelinePages('r', String)

    await pages.reload('s1')

    expect(ids(pages)).toEqual(['s3', 's2'])
    expect(pages.cursor).toBeUndefined()
  })

  it('is loading from the start of a fresh load until its last page is in', async () => {
    let release = () => {}
    const held = new Promise<void>((resolve) => (release = resolve))
    serve(async (cursor) => {
      if (cursor === 'p2') await held
      return { '': page(['b'], 'p2'), p2: page(['a']) }[cursor]
    })
    const pages = new TimelinePages('r', String)

    const loading = pages.reload('a')
    expect(pages.loading).toBe(true)
    await vi.waitFor(() => expect(ids(pages)).toEqual(['b']))
    expect(pages.loading).toBe(true)
    release()
    await loading

    expect(pages.loading).toBe(false)
  })

  it('asks for the Snapshots of the filtered path only, as it reads at each request', async () => {
    const asked = serve(() => page(['a']))
    let path: string | undefined = 'src/a.txt'
    const pages = new TimelinePages('r', String, () => path)

    await pages.reload()
    path = undefined
    await pages.reload()

    expect(new URL(asked[0] ?? '', 'http://x').searchParams.get('path')).toBe('src/a.txt')
    expect(new URL(asked[1] ?? '', 'http://x').searchParams.has('path')).toBe(false)
  })

  it('drops the answer to a load a newer one outdated', async () => {
    let release = () => {}
    const held = new Promise<void>((resolve) => (release = resolve))
    let first = true
    serve(async () => {
      if (!first) return page(['new'])
      first = false
      await held
      return page(['stale'])
    })
    const pages = new TimelinePages('r', String)

    const outdated = pages.reload()
    await pages.reload()
    release()
    await outdated

    expect(ids(pages)).toEqual(['new'])
    expect(pages.loading).toBe(false)
  })

  it('shows a failed load in words and counts it as loaded', async () => {
    vi.stubGlobal('fetch', () => Promise.reject(new Error('offline')))
    const pages = new TimelinePages('r', (e) => `failed: ${(e as Error).message}`)

    await pages.reload()

    expect(pages.error).toBe('failed: offline')
    expect(pages.loaded).toBe(true)
    expect(pages.loading).toBe(false)
  })

  it('puts Snapshots taken since the newest loaded one on top', async () => {
    let fresh = page(['b', 'a'])
    serve(() => fresh)
    const pages = new TimelinePages('r', String)
    await pages.reload()

    fresh = page(['d', 'c', 'b'])
    await pages.refresh()

    expect(ids(pages)).toEqual(['d', 'c', 'b', 'a'])
  })

  it('loads afresh when the newest page no longer reaches the newest loaded Snapshot', async () => {
    let fresh = page(['b', 'a'])
    serve(() => fresh)
    const pages = new TimelinePages('r', String)
    await pages.reload()

    fresh = page(['f', 'e'], 'more')
    await pages.refresh()

    expect(ids(pages)).toEqual(['f', 'e'])
    expect(pages.cursor).toBe('more')
  })

  it('appends the next, older page and knows when the oldest is loaded', async () => {
    serve((cursor) => ({ '': page(['b'], 'p2'), p2: page(['a']) })[cursor])
    const pages = new TimelinePages('r', String)
    await pages.reload()

    await pages.older()

    expect(ids(pages)).toEqual(['b', 'a'])
    expect(pages.cursor).toBeUndefined()
    expect(pages.paging).toBe(false)
  })

  it('asks for no older page past the oldest', async () => {
    const asked = serve(() => page(['a']))
    const pages = new TimelinePages('r', String)
    await pages.reload()

    await pages.older()

    expect(asked).toHaveLength(1)
  })

  describe('following the live stream', () => {
    it('puts a new Snapshot of this Root on top', async () => {
      vi.stubGlobal('EventSource', FakeSource)
      let fresh = page(['s1'])
      serve(() => fresh)
      const pages = new TimelinePages('r', String)
      await pages.reload()
      following(pages.follow())

      fresh = page(['s2', 's1'])
      stream().send('snapshot.created', { root: 'r' })

      await vi.waitFor(() => expect(ids(pages)).toEqual(['s2', 's1']))
    })

    it("asks nothing for another Root's Snapshot", async () => {
      vi.stubGlobal('EventSource', FakeSource)
      const asked = serve(() => page(['s1']))
      const pages = new TimelinePages('r', String)
      await pages.reload()
      following(pages.follow())

      stream().send('snapshot.created', { root: 'other' })
      await Promise.resolve()

      expect(asked).toHaveLength(1)
    })

    it('says so on resync, then loads afresh back to the wanted Snapshot', async () => {
      vi.stubGlobal('EventSource', FakeSource)
      let pagesServed = { '': page(['s3'], 'p2'), p2: page(['s2']) } as Record<string, unknown>
      serve((cursor) => pagesServed[cursor])
      const pages = new TimelinePages('r', String)
      await pages.reload()
      const told: string[] = []
      following(pages.follow({ until: () => 's2', onResync: () => told.push('resync') }))

      pagesServed = { '': page(['s4'], 'p2'), p2: page(['s2']) }
      stream().send('resync', {})

      expect(told).toEqual(['resync'])
      await vi.waitFor(() => expect(ids(pages)).toEqual(['s4', 's2']))
    })

    it('closes the stream when told to stop', () => {
      vi.stubGlobal('EventSource', FakeSource)
      const pages = new TimelinePages('r', String)

      const stop = following(pages.follow())
      stop()

      expect(stream().closed).toBe(true)
    })
  })
})
