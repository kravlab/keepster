import { afterEach, describe, expect, it, vi } from 'vitest'
import { SearchRun } from './searching.svelte'

const json = (body: unknown, status = 200, type = 'application/json') =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': type } })

const text = (e: unknown) => (e instanceof Error ? e.message : String(e))

const hit = { root: 'r1', path: 'a.txt', snapshots: 2, first: 'S1', last: 'S2', exists: true }

/** A streamed body the test writes into line by line, and the requests that opened it. */
const stream = () => {
  let controller!: ReadableStreamDefaultController<Uint8Array>
  const body = new ReadableStream<Uint8Array>({
    start: (c) => void (controller = c),
  })
  return {
    body,
    send: (record: unknown) => controller.enqueue(new TextEncoder().encode(`${JSON.stringify(record)}\n`)),
    end: () => controller.close(),
  }
}

/** Waits until the stream has been read as far as it has been written. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('a search run', () => {
  it('finds paths in every Root', async () => {
    const asked: string[] = []
    vi.stubGlobal('fetch', (url: string) => {
      asked.push(url)
      return Promise.resolve(json({ hits: [hit], truncated: false }))
    })
    const run = new SearchRun(text)

    await run.start('a', { content: false, root: 'r1', everyRoot: false })

    expect(asked).toEqual(['/v1/search/paths?q=a'])
    expect(run.paths).toEqual({ hits: [hit], truncated: false })
    expect(run.running).toBe(false)
  })

  it('shows a content search as its records arrive, in the current Root', async () => {
    const answer = stream()
    const asked: string[] = []
    vi.stubGlobal('fetch', (url: string) => {
      asked.push(url)
      return Promise.resolve(new Response(answer.body))
    })
    const run = new SearchRun(text)

    const done = run.start('needle', { content: true, root: 'r1', everyRoot: false })
    answer.send({ type: 'plan', roots: [{ root: 'r1', versions: 2, bytes: 10 }] })
    await settle()

    expect(asked).toEqual(['/v1/search/content?q=needle&roots=r1'])
    expect(run.running).toBe(true)
    expect(run.content?.costs).toEqual([{ root: 'r1', versions: 2, bytes: 10 }])

    answer.send({ type: 'summary', versions_scanned: 2, binaries_skipped: 0, withheld: 0 })
    answer.end()
    await done

    expect(run.running).toBe(false)
    expect(run.content?.summary?.versions_scanned).toBe(2)
  })

  it('searches the content of every Root when asked', async () => {
    const asked: string[] = []
    vi.stubGlobal('fetch', (url: string) => {
      asked.push(url)
      return Promise.resolve(new Response('{"type":"summary","versions_scanned":0}\n'))
    })

    await new SearchRun(text).start('needle', { content: true, root: 'r1', everyRoot: true })

    expect(asked).toEqual(['/v1/search/content?q=needle&all=true'])
  })

  it('stops a content search when cancelled, keeping what it found', async () => {
    const answer = stream()
    let signal: AbortSignal | undefined
    vi.stubGlobal('fetch', (_: string, init?: RequestInit) => {
      signal = init?.signal ?? undefined
      return Promise.resolve(new Response(answer.body))
    })
    const run = new SearchRun(text)
    const done = run.start('needle', { content: true, root: 'r1', everyRoot: false })
    answer.send({ type: 'plan', roots: [] })
    await settle()

    run.cancel()
    await done

    expect(signal?.aborted).toBe(true)
    expect(run.running).toBe(false)
    expect(run.cancelled).toBe(true)
    expect(run.error).toBeNull()
    expect(run.content?.costs).toEqual([])
  })

  it('lets a newer search replace an older one still running', async () => {
    let answerOlder: (r: Response) => void = () => {}
    vi.stubGlobal('fetch', (url: string) =>
      url.includes('q=old')
        ? new Promise<Response>((resolve) => (answerOlder = resolve))
        : Promise.resolve(json({ hits: [hit], truncated: false })),
    )
    const run = new SearchRun(text)

    const older = run.start('old', { content: false, root: 'r1', everyRoot: false })
    await run.start('new', { content: false, root: 'r1', everyRoot: false })
    answerOlder(json({ hits: [], truncated: true }))
    await older

    expect(run.paths).toEqual({ hits: [hit], truncated: false })
    expect(run.cancelled).toBe(false)
  })

  it('clears everything for an empty query, asking nothing', async () => {
    const fetch = vi.fn()
    vi.stubGlobal('fetch', fetch)
    const run = new SearchRun(text)

    await run.start('   ', { content: false, root: 'r1', everyRoot: false })

    expect(fetch).not.toHaveBeenCalled()
    expect(run.paths).toBeNull()
    expect(run.content).toBeNull()
  })

  it('says why a search failed', async () => {
    vi.stubGlobal('fetch', () =>
      Promise.resolve(
        json(
          { title: 'Bad Request', status: 400, detail: 'not a regex', code: 'query_invalid' },
          400,
          'application/problem+json',
        ),
      ),
    )
    const run = new SearchRun(text)

    await run.start('(', { content: true, root: 'r1', everyRoot: false })

    expect(run.error).toBe('not a regex')
    expect(run.running).toBe(false)
  })
})
