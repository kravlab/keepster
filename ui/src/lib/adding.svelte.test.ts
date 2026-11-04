import { afterEach, describe, expect, it, vi } from 'vitest'
import { AddingRoot } from './adding.svelte'
import { SessionEnded } from './api'
import { WatchedRoots } from './roots.svelte'
import { FakeSource, following, ndjson, stopFollowing, stream } from './testing'
import type { GitStatus, PreviewSummary, Root } from './wire'

const json = (body: unknown, status = 200, type = 'application/json') =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': type } })

const problem = (status: number, code: string, detail: string, details?: Record<string, string>) =>
  json({ title: 'Refused', status, detail, code, ...(details && { details }) }, status, 'application/problem+json')

const root = (path: string): Root => ({ id: 'r1', path, state: 'watching' })

/** `GET /v1/status` with store maintenance as `maintenance` says, and `git` as given. */
const status = (maintenance: string, git: GitStatus = { minimum: '2.30' }) =>
  json({ daemon: { maintenance, git }, roots: [] })

/** A Capture Preview's summary, past the claims `past` names. */
const summary = (...past: string[]): PreviewSummary => ({
  type: 'summary',
  captured: { files: 120, directories: 14, bytes: 5 * 1024 * 1024 },
  excluded: { files: 8, directories: 2, bytes: 4096, pruned: 1 },
  withheld: {},
  unreadable: 0,
  groups: [],
  largest_captured: [{ path: 'data/big.bin', bytes: 4 * 1024 * 1024 }],
  largest_left_out: [],
  past_ceiling: past,
})

/** `GET /v1/preview` answering with `records`, one line each, as a stream that has already ended. */
const previewed =
  (...records: unknown[]) =>
  () =>
    ndjson(...records.map((r) => JSON.stringify(r)))

/** A preview of a directory within what one Root is claimed to hold. */
const within = previewed({ type: 'header', path: '/home/u/src' }, { type: 'progress', walked: 3 }, summary())

/** One request as the daemon receives it. */
interface Sent {
  method: string
  url: string
  body?: unknown
}

/** Answers each request in turn with the next of `answers`, recording what was sent. */
const serve = (...answers: (() => Response)[]) => {
  const sent: Sent[] = []
  vi.stubGlobal('fetch', (url: string, init?: RequestInit) => {
    sent.push({
      method: init?.method ?? 'GET',
      url,
      ...(typeof init?.body === 'string' && { body: JSON.parse(init.body) as unknown }),
    })
    const next = answers.shift()
    if (!next) throw new Error(`no answer left for ${url}`)
    return Promise.resolve(next())
  })
  return sent
}

/** Lets the requests the flow started without waiting for them settle. */
const settled = () => new Promise((resolve) => setTimeout(resolve, 0))

/** The failure text the dialog shows: the daemon's explanation. */
const reason = (e: unknown) => (e instanceof Error ? e.message : String(e))

/** The live list's lookup when it knows no Root the daemon names. */
const unknown = () => undefined

afterEach(() => {
  stopFollowing()
  vi.unstubAllGlobals()
})

describe('adding a directory', () => {
  it('registers a directory within the claim after its preview, with no question', async () => {
    const sent = serve(
      within,
      () => json(root('/home/u/src'), 201),
      () => status('available'),
    )
    const adding = new AddingRoot(reason, unknown)

    await adding.add('/home/u/src')

    expect(sent.slice(0, 2)).toEqual([
      { method: 'GET', url: '/v1/preview?path=/home/u/src' },
      { method: 'POST', url: '/v1/roots', body: { path: '/home/u/src' } },
    ])
    expect(adding.step).toEqual({ kind: 'done', root: root('/home/u/src'), maintenance: null })
  })

  it.each([
    [409, 'root_overlaps', '/home/u/src/app overlaps the Root 0190a3c4'],
    [400, 'path_invalid', '/home/u/notes.txt is not a directory'],
  ])("shows the daemon's reason for a refusal (%i %s) and can be tried again", async (code, name, detail) => {
    const sent = serve(
      within,
      () => problem(code, name, detail),
      within,
      () => json(root('/home/u/other'), 201),
      () => status('available'),
    )
    const adding = new AddingRoot(reason, unknown)

    await adding.add('/home/u/src/app')
    expect(adding.step).toEqual({ kind: 'failed', path: '/home/u/src/app', reason: detail })
    await adding.add('/home/u/other')

    expect(sent.filter((s) => s.method === 'POST').map((s) => s.body)).toEqual([
      { path: '/home/u/src/app' },
      { path: '/home/u/other' },
    ])
    expect(adding.step).toMatchObject({ kind: 'done', root: root('/home/u/other') })
  })

  it('shows up in the followed Watched Roots once the daemon announces it', async () => {
    const registry: Root[] = []
    vi.stubGlobal('fetch', (url: string, init?: RequestInit) => {
      if (url.startsWith('/v1/preview')) return Promise.resolve(within())
      if (init?.method === 'POST') {
        registry.push(root('/home/u/src'))
        return Promise.resolve(json(root('/home/u/src'), 201))
      }
      return Promise.resolve(url === '/v1/status' ? status('available') : json(registry))
    })
    vi.stubGlobal('EventSource', FakeSource)
    const watched = new WatchedRoots(reason)
    await watched.load()
    following(watched.follow())
    const adding = new AddingRoot(reason, unknown)

    await adding.add('/home/u/src')
    stream().send('root.registered', { event: 'root.registered', root: 'r1' })

    await vi.waitFor(() => expect(watched.roots).toEqual([root('/home/u/src')]))
  })

  describe('a directory overlapping a Root', () => {
    const overlaps = () =>
      problem(409, 'root_overlaps', '/home/u/src/app overlaps the Root 0190a3c4', {
        path: '/home/u/src/app',
        root: '0190a3c4',
      })

    it('names the Root by its path when it is in the live list', async () => {
      serve(within, overlaps)
      const known: Root = { id: '0190a3c4', path: '/home/u/src', state: 'watching' }
      const adding = new AddingRoot(reason, (id) => (id === known.id ? known : undefined))

      await adding.add('/home/u/src/app')

      expect(adding.step).toEqual({
        kind: 'failed',
        path: '/home/u/src/app',
        reason: '/home/u/src/app overlaps the Root at /home/u/src',
      })
    })

    it("keeps the daemon's reason when the Root is not in the live list", async () => {
      serve(within, overlaps)
      const adding = new AddingRoot(reason, unknown)

      await adding.add('/home/u/src/app')

      expect(adding.step).toMatchObject({ kind: 'failed', reason: '/home/u/src/app overlaps the Root 0190a3c4' })
    })
  })

  it('sends nothing more while a directory is being registered', async () => {
    let answer: (response: Response) => void = () => undefined
    const posts: string[] = []
    vi.stubGlobal('fetch', (url: string, init?: RequestInit) => {
      if (url.startsWith('/v1/preview')) return Promise.resolve(within())
      if (init?.method === 'POST') posts.push(url)
      return init?.method === 'POST'
        ? new Promise<Response>((resolve) => (answer = resolve))
        : Promise.resolve(status('available'))
    })
    const adding = new AddingRoot(reason, unknown)

    const first = adding.add('/home/u/src')
    await vi.waitFor(() => expect(adding.step.kind).toBe('registering'))
    await adding.add('/home/u/other')
    answer(json(root('/home/u/src'), 201))
    await first

    expect(posts).toEqual(['/v1/roots'])
    expect(adding.step).toMatchObject({ kind: 'done', root: root('/home/u/src') })
  })

  describe('the notice that store maintenance is unavailable', () => {
    const git: GitStatus = {
      minimum: '2.30',
      reason: 'missing',
      install: { command: 'sudo apt install git', link: 'https://example.com/git' },
    }

    it("comes with the daemon's git guidance once the status says maintenance is unavailable", async () => {
      const sent = serve(
        within,
        () => json(root('/home/u/src'), 201),
        () => status('unavailable', git),
      )
      const adding = new AddingRoot(reason, unknown)

      await adding.add('/home/u/src')

      await vi.waitFor(() => expect(adding.step).toEqual({ kind: 'done', root: root('/home/u/src'), maintenance: git }))
      expect(sent.map((s) => `${s.method} ${s.url}`)).toEqual([
        'GET /v1/preview?path=/home/u/src',
        'POST /v1/roots',
        'GET /v1/status',
      ])
    })

    it('does not hold the directory added back while the status is on its way, and follows once it answers', async () => {
      let answer: (response: Response) => void = () => undefined
      vi.stubGlobal('fetch', (url: string, init?: RequestInit) =>
        url.startsWith('/v1/preview')
          ? Promise.resolve(within())
          : init?.method === 'POST'
            ? Promise.resolve(json(root('/home/u/src'), 201))
            : new Promise<Response>((resolve) => (answer = resolve)),
      )
      const adding = new AddingRoot(reason, unknown)

      await adding.add('/home/u/src')
      expect(adding.step).toEqual({ kind: 'done', root: root('/home/u/src'), maintenance: null })

      answer(status('unavailable', git))
      await vi.waitFor(() => expect(adding.step).toEqual({ kind: 'done', root: root('/home/u/src'), maintenance: git }))
    })

    it('hands an ended session on, so the page can say so, and still has the directory added', async () => {
      serve(
        within,
        () => json(root('/home/u/src'), 201),
        () => new Response('', { status: 401 }),
      )
      const failures: unknown[] = []
      const adding = new AddingRoot((e) => {
        failures.push(e)
        return reason(e)
      }, unknown)

      await adding.add('/home/u/src')
      await vi.waitFor(() => expect(failures).toEqual([expect.any(SessionEnded)]))

      expect(adding.step).toEqual({ kind: 'done', root: root('/home/u/src'), maintenance: null })
    })

    it.each(['available', 'unknown'])('is absent while the status says maintenance is %s', async (maintenance) => {
      serve(
        within,
        () => json(root('/home/u/src'), 201),
        () => status(maintenance, git),
      )
      const adding = new AddingRoot(reason, unknown)

      await adding.add('/home/u/src')
      await settled()

      expect(adding.step).toEqual({ kind: 'done', root: root('/home/u/src'), maintenance: null })
    })

    it('is absent, and the directory still added, when the status cannot be read', async () => {
      serve(
        within,
        () => json(root('/home/u/src'), 201),
        () => problem(500, 'registry_failed', 'the Registry could not be read'),
      )
      const failure = vi.fn(reason)
      const adding = new AddingRoot(failure, unknown)

      await adding.add('/home/u/src')
      await settled()

      expect(adding.step).toEqual({ kind: 'done', root: root('/home/u/src'), maintenance: null })
      expect(failure).not.toHaveBeenCalled()
    })
  })

  describe('a directory reached through a link', () => {
    const throughLink = () =>
      problem(400, 'confirmation_required', '/home/u/proj leads through a link', {
        path: '/home/u/proj',
        real_path: '/mnt/work/proj',
        missing: 'path_mode',
      })

    it('asks whether the Root is the link or the real directory, registering nothing yet', async () => {
      const sent = serve(within, throughLink)
      const adding = new AddingRoot(reason, unknown)

      await adding.add('/home/u/proj')

      expect(sent).toHaveLength(2)
      expect(adding.step).toEqual({ kind: 'askingLink', path: '/home/u/proj', realPath: '/mnt/work/proj' })
    })

    it.each(['link', 'canonical'] as const)('registers it again as the %s when told to', async (mode) => {
      const sent = serve(
        within,
        throughLink,
        () => json(root('/home/u/proj'), 201),
        () => status('available'),
      )
      const adding = new AddingRoot(reason, unknown)
      await adding.add('/home/u/proj')

      await adding.choose(mode)

      expect(sent[2]).toEqual({ method: 'POST', url: '/v1/roots', body: { path: '/home/u/proj', path_mode: mode } })
      expect(adding.step).toMatchObject({ kind: 'done', root: root('/home/u/proj') })
    })

    it('shows the reason when registering it as chosen is refused', async () => {
      serve(within, throughLink, () => problem(409, 'root_overlaps', '/mnt/work/proj overlaps the Root 0190a3c4'))
      const adding = new AddingRoot(reason, unknown)
      await adding.add('/home/u/proj')

      await adding.choose('canonical')

      expect(adding.step).toEqual({
        kind: 'failed',
        path: '/home/u/proj',
        reason: '/mnt/work/proj overlaps the Root 0190a3c4',
      })
    })

    it('registers nothing when the question is cancelled, and goes back to choosing', async () => {
      const sent = serve(within, throughLink)
      const adding = new AddingRoot(reason, unknown)
      await adding.add('/home/u/proj')

      adding.cancel()

      expect(sent).toHaveLength(2)
      expect(adding.step).toEqual({ kind: 'choosing' })
    })
  })

  describe('the Capture Preview before registering', () => {
    /** The text of each request sent, in order. */
    const asked = (sent: Sent[]) => sent.map((s) => `${s.method} ${s.url}`)

    it('asks, registering nothing, when the directory is past the claim', async () => {
      const sent = serve(previewed({ type: 'progress', walked: 9 }, summary('files', 'bytes')))
      const adding = new AddingRoot(reason, unknown)

      await adding.add('/home/u/huge')

      expect(asked(sent)).toEqual(['GET /v1/preview?path=/home/u/huge'])
      expect(adding.step).toEqual({ kind: 'askingClaim', path: '/home/u/huge', summary: summary('files', 'bytes') })
    })

    it('registers the directory past the claim once told to watch it anyway', async () => {
      const sent = serve(
        previewed(summary('directories')),
        () => json(root('/home/u/huge'), 201),
        () => status('available'),
      )
      const adding = new AddingRoot(reason, unknown)
      await adding.add('/home/u/huge')

      await adding.watchAnyway()

      expect(sent[1]).toEqual({ method: 'POST', url: '/v1/roots', body: { path: '/home/u/huge' } })
      expect(adding.step).toMatchObject({ kind: 'done', root: root('/home/u/huge') })
    })

    it('registers nothing when told not to watch it, and goes back to choosing', async () => {
      const sent = serve(previewed(summary('files')))
      const adding = new AddingRoot(reason, unknown)
      await adding.add('/home/u/huge')

      adding.cancel()
      await adding.watchAnyway()

      expect(asked(sent)).toEqual(['GET /v1/preview?path=/home/u/huge'])
      expect(adding.step).toEqual({ kind: 'choosing' })
    })

    it.each([
      ['the walk stops with an error', previewed({ type: 'error', detail: 'the directory vanished' })],
      ['the stream ends before its summary', previewed({ type: 'progress', walked: 2 })],
      ['the daemon fails it', () => problem(500, 'preview_failed', 'the preview could not be prepared')],
      ['it is refused other than as busy', () => problem(409, 'config_invalid', 'the .keepster.toml cannot be read')],
      ['it is malformed', () => problem(400, 'path_invalid', 'not absolute')],
      ['it sends a line that is not JSON', () => new Response('garbage\n')],
      [
        'the daemon cannot be reached',
        () => {
          throw new TypeError('Failed to fetch')
        },
      ],
    ])('still registers the directory when %s, as the CLI does', async (_, failed) => {
      const sent = serve(
        failed,
        () => json(root('/home/u/src'), 201),
        () => status('available'),
      )
      const adding = new AddingRoot(reason, unknown)

      await adding.add('/home/u/src')

      expect(asked(sent).slice(0, 2)).toEqual(['GET /v1/preview?path=/home/u/src', 'POST /v1/roots'])
      expect(adding.step).toMatchObject({ kind: 'done', root: root('/home/u/src') })
    })

    it('stops with "try again", registering nothing, while another preview is walking', async () => {
      const sent = serve(() => problem(409, 'preview_busy', 'A Capture Preview is already walking'))
      const adding = new AddingRoot(reason, unknown)

      await adding.add('/home/u/src')

      expect(asked(sent)).toEqual(['GET /v1/preview?path=/home/u/src'])
      expect(adding.step).toEqual({
        kind: 'failed',
        path: '/home/u/src',
        reason: 'Another Capture Preview is walking, and this directory must be looked at first; try again.',
      })
    })

    it('hands an ended session on, registering nothing', async () => {
      const sent = serve(() => new Response('', { status: 401 }))
      const failures: unknown[] = []
      const adding = new AddingRoot((e) => {
        failures.push(e)
        return reason(e)
      }, unknown)

      await adding.add('/home/u/src')

      expect(asked(sent)).toEqual(['GET /v1/preview?path=/home/u/src'])
      expect(failures).toEqual([expect.any(SessionEnded)])
      expect(adding.step).toMatchObject({ kind: 'failed', path: '/home/u/src' })
    })

    describe('while the walk runs', () => {
      /** A preview answered by a stream fed by hand, recording what was asked and whether it was stopped. */
      const walking = () => {
        const encoder = new TextEncoder()
        let feed: ReadableStreamDefaultController<Uint8Array> | undefined
        const run = { asked: [] as string[], signal: undefined as AbortSignal | undefined, cancelled: false }
        const body = new ReadableStream<Uint8Array>({
          start: (controller) => void (feed = controller),
          cancel: () => void (run.cancelled = true),
        })
        vi.stubGlobal('fetch', (url: string, init?: RequestInit) => {
          run.asked.push(`${init?.method ?? 'GET'} ${url}`)
          run.signal = init?.signal ?? undefined
          return Promise.resolve(new Response(body, { headers: { 'content-type': 'application/x-ndjson' } }))
        })
        const send = (record: unknown) => feed?.enqueue(encoder.encode(`${JSON.stringify(record)}\n`))
        return { run, send }
      }

      it('counts the paths walked so far as its progress comes', async () => {
        const { send } = walking()
        const adding = new AddingRoot(reason, unknown)

        void adding.add('/home/u/src')
        await vi.waitFor(() => expect(adding.step).toEqual({ kind: 'previewing', path: '/home/u/src', walked: 0 }))
        send({ type: 'progress', walked: 10 })
        await vi.waitFor(() => expect(adding.step).toEqual({ kind: 'previewing', path: '/home/u/src', walked: 10 }))
        send({ type: 'progress', walked: 25 })

        await vi.waitFor(() => expect(adding.step).toEqual({ kind: 'previewing', path: '/home/u/src', walked: 25 }))
      })

      it('starts nothing else meanwhile', async () => {
        const { run } = walking()
        const adding = new AddingRoot(reason, unknown)

        void adding.add('/home/u/src')
        await adding.add('/home/u/other')

        expect(run.asked).toEqual(['GET /v1/preview?path=/home/u/src'])
      })

      it('is stopped by cancelling, which closes the stream and registers nothing', async () => {
        const { run, send } = walking()
        const adding = new AddingRoot(reason, unknown)
        const walk = adding.add('/home/u/src')
        send({ type: 'progress', walked: 10 })
        await vi.waitFor(() => expect(adding.step).toMatchObject({ kind: 'previewing', walked: 10 }))

        adding.cancel()
        await walk

        expect(run.signal?.aborted).toBe(true)
        await vi.waitFor(() => expect(run.cancelled).toBe(true))
        expect(run.asked).toEqual(['GET /v1/preview?path=/home/u/src'])
        expect(adding.step).toEqual({ kind: 'choosing' })
      })
    })
  })
})
