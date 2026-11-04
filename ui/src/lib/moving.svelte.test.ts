import { afterEach, describe, expect, it, vi } from 'vitest'
import { SessionEnded } from './api'
import { MovingRoot } from './moving.svelte'
import type { Root } from './wire'

const missing: Root = { id: 'r1', path: '/home/u/src', state: 'unavailable', state_code: 'root_missing' }
const moved = (path: string): Root => ({ id: 'r1', path, state: 'watching' })

const json = (body: unknown, status = 200, type = 'application/json') =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': type } })

const problem = (status: number, code: string, detail: string, details?: Record<string, string>) =>
  json({ title: 'Refused', status, detail, code, ...(details && { details }) }, status, 'application/problem+json')

const throughLink = () =>
  problem(400, 'confirmation_required', 'leads through a link', {
    path: '/home/u/proj',
    real_path: '/mnt/work/proj',
    missing: 'path_mode',
  })

/** One request as the daemon receives it. */
interface Sent {
  method: string
  url: string
  body: unknown
}

/** Answers each request in turn with the next of `answers`, recording what was sent. */
const serve = (...answers: (() => Response)[]) => {
  const sent: Sent[] = []
  vi.stubGlobal('fetch', (url: string, init?: RequestInit) => {
    sent.push({ method: init?.method ?? 'GET', url, body: init?.body ? JSON.parse(String(init.body)) : undefined })
    const next = answers.shift()
    if (!next) throw new Error(`no answer left for ${url}`)
    return Promise.resolve(next())
  })
  return sent
}

/** The failure text the dialog shows: the daemon's explanation. */
const reason = (e: unknown) => (e instanceof Error ? e.message : String(e))

/** The live list's lookup when it knows no Root the daemon names. */
const unknown = () => undefined

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('recording a Move', () => {
  it('starts with the person choosing where the directory is now', () => {
    const moving = new MovingRoot(missing, reason, unknown)

    expect(moving.root).toEqual(missing)
    expect(moving.step).toEqual({ kind: 'choosing' })
  })

  it('patches the Root with the path chosen, and is done with the Root the daemon describes', async () => {
    const sent = serve(() => json(moved('/home/u/code/src')))
    const moving = new MovingRoot(missing, reason, unknown)

    await moving.move('/home/u/code/src')

    expect(sent).toEqual([{ method: 'PATCH', url: '/v1/roots/r1', body: { path: '/home/u/code/src' } }])
    expect(moving.step).toEqual({ kind: 'done', root: moved('/home/u/code/src') })
  })

  it.each([
    [400, 'path_invalid', '/home/u/notes.txt is not a directory that can be watched'],
    [409, 'root_overlaps', '/home/u/notes.txt overlaps the Root 0190a3c4'],
    [404, 'root_not_found', 'no Root with the identity r1 is registered'],
  ])(
    "shows the daemon's reason for a refusal (%i %s), keeping the path, and can be tried again",
    async (status, code, detail) => {
      const sent = serve(
        () => problem(status, code, detail),
        () => json(moved('/home/u/code/src')),
      )
      const moving = new MovingRoot(missing, reason, unknown)

      await moving.move('/home/u/notes.txt')
      expect(moving.step).toEqual({ kind: 'failed', path: '/home/u/notes.txt', reason: detail })
      await moving.move('/home/u/code/src')

      expect(sent.map((s) => s.body)).toEqual([{ path: '/home/u/notes.txt' }, { path: '/home/u/code/src' }])
      expect(moving.step).toEqual({ kind: 'done', root: moved('/home/u/code/src') })
    },
  )

  it('names an overlapped Root by its path when it is in the live list', async () => {
    serve(() =>
      problem(409, 'root_overlaps', '/home/u/work/app overlaps the Root 0190a3c4', {
        path: '/home/u/work/app',
        root: '0190a3c4',
      }),
    )
    const known: Root = { id: '0190a3c4', path: '/home/u/work', state: 'watching' }
    const moving = new MovingRoot(missing, reason, (id) => (id === known.id ? known : undefined))

    await moving.move('/home/u/work/app')

    expect(moving.step).toEqual({
      kind: 'failed',
      path: '/home/u/work/app',
      reason: '/home/u/work/app overlaps the Root at /home/u/work',
    })
  })

  it('hands an ended session on', async () => {
    serve(() => json({ title: 'Unauthorized', status: 401, code: 'unauthorized' }, 401))
    const failure = vi.fn((e: unknown) => (e instanceof SessionEnded ? 'ended' : 'other'))
    const moving = new MovingRoot(missing, failure, unknown)

    await moving.move('/home/u/code/src')

    expect(failure).toHaveBeenCalledWith(expect.any(SessionEnded))
    expect(moving.step).toEqual({ kind: 'failed', path: '/home/u/code/src', reason: 'ended' })
  })

  it('sends nothing more while a Move is on its way', async () => {
    let answer: (response: Response) => void = () => undefined
    const sent: string[] = []
    vi.stubGlobal('fetch', (url: string) => {
      sent.push(url)
      return new Promise<Response>((resolve) => (answer = resolve))
    })
    const moving = new MovingRoot(missing, reason, unknown)

    const first = moving.move('/home/u/code/src')
    expect(moving.step).toEqual({ kind: 'moving', path: '/home/u/code/src' })
    await moving.move('/home/u/other')
    answer(json(moved('/home/u/code/src')))
    await first

    expect(sent).toEqual(['/v1/roots/r1'])
    expect(moving.step).toEqual({ kind: 'done', root: moved('/home/u/code/src') })
  })

  describe('a path reached through a link', () => {
    it('asks whether the Root is the link or the real directory, recording nothing yet', async () => {
      const sent = serve(throughLink)
      const moving = new MovingRoot(missing, reason, unknown)

      await moving.move('/home/u/proj')

      expect(sent).toHaveLength(1)
      expect(moving.step).toEqual({ kind: 'askingLink', path: '/home/u/proj', realPath: '/mnt/work/proj' })
    })

    it.each(['link', 'canonical'] as const)('sends the same path again as the %s when told to', async (mode) => {
      const sent = serve(throughLink, () => json(moved('/home/u/proj')))
      const moving = new MovingRoot(missing, reason, unknown)
      await moving.move('/home/u/proj')

      await moving.choose(mode)

      expect(sent[1]).toEqual({ method: 'PATCH', url: '/v1/roots/r1', body: { path: '/home/u/proj', path_mode: mode } })
      expect(moving.step).toEqual({ kind: 'done', root: moved('/home/u/proj') })
    })

    it('shows the reason when recording it as chosen is refused', async () => {
      serve(throughLink, () => problem(409, 'root_overlaps', '/mnt/work/proj overlaps the Root 0190a3c4'))
      const moving = new MovingRoot(missing, reason, unknown)
      await moving.move('/home/u/proj')

      await moving.choose('canonical')

      expect(moving.step).toEqual({
        kind: 'failed',
        path: '/home/u/proj',
        reason: '/mnt/work/proj overlaps the Root 0190a3c4',
      })
    })

    it('records nothing when the question is cancelled, and goes back to choosing', async () => {
      const sent = serve(throughLink)
      const moving = new MovingRoot(missing, reason, unknown)
      await moving.move('/home/u/proj')

      moving.cancel()

      expect(sent).toHaveLength(1)
      expect(moving.step).toEqual({ kind: 'choosing' })
    })

    it('sends nothing for an answer when no question is asked', async () => {
      const sent = serve()
      const moving = new MovingRoot(missing, reason, unknown)

      await moving.choose('link')

      expect(sent).toEqual([])
      expect(moving.step).toEqual({ kind: 'choosing' })
    })
  })
})
