import { afterEach, describe, expect, it, vi } from 'vitest'
import { SessionEnded } from './api'
import { UnwatchingRoot } from './unwatching.svelte'
import type { Root } from './wire'

const r1: Root = { id: 'r1', path: '/home/u/src', state: 'watching' }

const json = (body: unknown, status = 200, type = 'application/json') =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': type } })

const problem = (status: number, code: string, detail: string) =>
  json({ title: 'Refused', status, detail, code }, status, 'application/problem+json')

/** `GET /v1/roots/{id}/pins` answering with the pinned Snapshots `ids`. */
const pins =
  (...ids: string[]) =>
  () =>
    json({
      snapshots: ids.map((id) => ({ id, taken_at: '2026-10-12T23:45:01.000Z', trigger: 'watcher', incomplete: false })),
    })

const unwatched = () => new Response(null, { status: 204 })

/** One request as the daemon receives it. */
interface Sent {
  method: string
  url: string
}

/** Answers each request in turn with the next of `answers`, recording what was sent. */
const serve = (...answers: (() => Response | Promise<Response>)[]) => {
  const sent: Sent[] = []
  vi.stubGlobal('fetch', (url: string, init?: RequestInit) => {
    sent.push({ method: init?.method ?? 'GET', url })
    const next = answers.shift()
    if (!next) throw new Error(`no answer left for ${url}`)
    return Promise.resolve(next())
  })
  return sent
}

/** The failure text the dialog shows: the daemon's explanation. */
const reason = (e: unknown) => (e instanceof Error ? e.message : String(e))

/** A Root's unwatch with consent given as `erase` and `acknowledged` say. */
const consenting = (erase: boolean, acknowledged: boolean, failure = reason) => {
  const unwatching = new UnwatchingRoot(r1, failure)
  unwatching.chooseErasing(erase)
  unwatching.acknowledge(acknowledged)
  return unwatching
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('what unwatching a Root says of its history', () => {
  it('says the history is kept and pinned when the Root has a pinned Snapshot', async () => {
    const sent = serve(pins('s2', 's1'))
    const unwatching = new UnwatchingRoot(r1, reason)

    await unwatching.readHistory()

    expect(sent).toEqual([{ method: 'GET', url: '/v1/roots/r1/pins' }])
    expect(unwatching.outlook).toBe('kept')
  })

  it('says there is no history to keep when the Root has no pinned Snapshot', async () => {
    serve(pins())
    const unwatching = new UnwatchingRoot(r1, reason)

    await unwatching.readHistory()

    expect(unwatching.outlook).toBe('nothingToKeep')
  })

  it('claims neither while the pins are being read', () => {
    serve(() => new Promise<Response>(() => {}))
    const unwatching = new UnwatchingRoot(r1, reason)

    void unwatching.readHistory()

    expect(unwatching.outlook).toBe('unknown')
  })

  it('claims neither when the pins cannot be read', async () => {
    serve(() => problem(500, 'internal', 'the store is unreadable'))
    const unwatching = new UnwatchingRoot(r1, reason)

    await unwatching.readHistory()

    expect(unwatching.outlook).toBe('unknown')
    expect(unwatching.error).toBe('')
  })

  it('hands an ended session met reading the pins on, and claims neither', async () => {
    serve(() => json({ title: 'Unauthorized', status: 401, code: 'unauthorized' }, 401))
    const failure = vi.fn(reason)
    const unwatching = new UnwatchingRoot(r1, failure)

    await unwatching.readHistory()

    expect(failure).toHaveBeenCalledWith(expect.any(SessionEnded))
    expect(unwatching.outlook).toBe('unknown')
  })

  it('says the history is erased once erasing is chosen, whatever the pins say, and kept again once withdrawn', async () => {
    serve(pins('s1'))
    const unwatching = new UnwatchingRoot(r1, reason)
    await unwatching.readHistory()

    unwatching.chooseErasing(true)
    expect(unwatching.outlook).toBe('erasing')

    unwatching.chooseErasing(false)
    expect(unwatching.outlook).toBe('kept')
  })
})

describe('unwatching a Root', () => {
  it('sends no purge when erasing is not chosen, and is done', async () => {
    const sent = serve(unwatched)
    const unwatching = consenting(false, false)

    await unwatching.go()

    expect(sent).toEqual([{ method: 'DELETE', url: '/v1/roots/r1' }])
    expect(unwatching.done).toBe(true)
  })

  it('sends nothing when erasing is chosen but not acknowledged', async () => {
    const sent = serve()
    const unwatching = consenting(true, false)

    await unwatching.go()

    expect(sent).toEqual([])
    expect(unwatching.canGo).toBe(false)
    expect(unwatching.done).toBe(false)
  })

  it('sends purge=true once erasing is both chosen and acknowledged', async () => {
    const sent = serve(unwatched)
    const unwatching = consenting(true, true)

    await unwatching.go()

    expect(sent).toEqual([{ method: 'DELETE', url: '/v1/roots/r1?purge=true' }])
    expect(unwatching.done).toBe(true)
  })

  it('stays open on a refusal with the daemon’s reason, the choices as they were', async () => {
    serve(() => problem(409, 'root_busy', 'a Restore into this Root is running'))
    const unwatching = consenting(true, true)

    await unwatching.go()

    expect(unwatching.done).toBe(false)
    expect(unwatching.running).toBe(false)
    expect(unwatching.error).toBe('a Restore into this Root is running')
    expect(unwatching.consent).toEqual({ erase: true, acknowledged: true })
    expect(unwatching.canGo).toBe(true)
  })

  it('sends nothing more while a request is on its way', async () => {
    let answer: (response: Response) => void = () => {}
    const sent = serve(() => new Promise<Response>((resolve) => (answer = resolve)))
    const unwatching = consenting(false, false)

    const first = unwatching.go()
    expect(unwatching.running).toBe(true)
    expect(unwatching.canGo).toBe(false)
    await unwatching.go()
    answer(unwatched())
    await first

    expect(sent).toEqual([{ method: 'DELETE', url: '/v1/roots/r1' }])
    expect(unwatching.done).toBe(true)
  })

  it('clears the reason of an earlier refusal when it tries again', async () => {
    serve(() => problem(409, 'root_busy', 'a Restore into this Root is running'), unwatched)
    const unwatching = consenting(false, false)
    await unwatching.go()

    await unwatching.go()

    expect(unwatching.error).toBe('')
    expect(unwatching.done).toBe(true)
  })
})
