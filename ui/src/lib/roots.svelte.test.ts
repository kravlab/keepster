import { afterEach, describe, expect, it, vi } from 'vitest'
import { WatchedRoots } from './roots.svelte'
import { FakeSource, following, stopFollowing, stream } from './testing'
import type { Root } from './wire'

const root = (id: string, over: Partial<Root> = {}): Root => ({ id, path: `/home/u/${id}`, state: 'watching', ...over })

/** Answers every `GET /v1/roots` with what `list` holds at that moment, recording the URLs asked for. */
const serve = (list: () => Root[]) => {
  const asked: string[] = []
  vi.stubGlobal('fetch', async (url: string) => {
    asked.push(url)
    return new Response(JSON.stringify(list()), { status: 200, headers: { 'content-type': 'application/json' } })
  })
  return asked
}

/** Lets the requests an event started settle. */
const settled = () => new Promise((resolve) => setTimeout(resolve, 0))

/**
 * The Registry `registry()` answers, loaded and followed from the live stream. Each event sent then has
 * the Registry read afresh; change what `registry` returns before sending one.
 */
const followed = async (registry: () => Root[]) => {
  const asked = serve(registry)
  vi.stubGlobal('EventSource', FakeSource)
  const roots = new WatchedRoots(String)
  await roots.load()
  const stop = following(roots.follow())
  return { roots, asked, stop }
}

afterEach(() => {
  stopFollowing()
  vi.unstubAllGlobals()
})

describe('the Watched Roots as the page knows them', () => {
  it('says why when the Registry cannot be read', async () => {
    vi.stubGlobal('fetch', async () => new Response('', { status: 500 }))
    const roots = new WatchedRoots(() => 'the Registry could not be read')

    await roots.load()

    expect(roots.roots).toBeNull()
    expect(roots.error).toBe('the Registry could not be read')
  })

  it('holds every registered Root once loaded', async () => {
    const asked = serve(() => [root('a'), root('b')])
    const roots = new WatchedRoots(String)

    await roots.load()

    expect(asked).toEqual(['/v1/roots'])
    expect(roots.roots?.map((r) => r.id)).toEqual(['a', 'b'])
  })

  describe('following the live stream', () => {
    it('shows a Root registered elsewhere', async () => {
      let registry = [root('a')]
      const { roots } = await followed(() => registry)

      registry = [root('a'), root('b')]
      stream().send('root.registered', { event: 'root.registered', root: 'b' })
      await settled()

      expect(roots.roots?.map((r) => r.id)).toEqual(['a', 'b'])
    })

    it('shows a Root at the path a Move recorded', async () => {
      let registry = [root('a'), root('b')]
      const { roots } = await followed(() => registry)

      registry = [root('a'), root('b', { path: '/mnt/b' })]
      stream().send('root.moved', { event: 'root.moved', root: 'b' })
      await settled()

      expect(roots.roots?.map((r) => r.path)).toEqual(['/home/u/a', '/mnt/b'])
    })

    it('drops a Root unwatched elsewhere and keeps the others', async () => {
      let registry = [root('a'), root('b'), root('c')]
      const { roots } = await followed(() => registry)

      registry = [root('a'), root('c')]
      stream().send('root.unregistered', { event: 'root.unregistered', root: 'b', purged: false })
      await settled()

      expect(roots.roots?.map((r) => r.id)).toEqual(['a', 'c'])
    })

    it('shows a Root’s new state and the reason behind it', async () => {
      let registry = [root('a'), root('b')]
      const { roots } = await followed(() => registry)

      registry = [root('a'), root('b', { state: 'unavailable', state_code: 'root_missing' })]
      stream().send('root.state_changed', {
        event: 'root.state_changed',
        root: 'b',
        state: 'unavailable',
        state_code: 'root_missing',
      })
      await settled()

      expect(roots.roots).toEqual([root('a'), root('b', { state: 'unavailable', state_code: 'root_missing' })])
    })

    it('reads the Registry afresh when the stream says what is shown may be stale', async () => {
      let registry = [root('a')]
      const { roots } = await followed(() => registry)

      registry = [root('b')]
      stream().send('resync', { event: 'resync' })
      await settled()

      expect(roots.roots?.map((r) => r.id)).toEqual(['b'])
    })

    it('asks the Registry nothing more once it stops following', async () => {
      const { asked, stop } = await followed(() => [root('a')])

      stop()
      stream().send('root.registered', { event: 'root.registered', root: 'b' })
      await settled()

      expect(asked).toEqual(['/v1/roots'])
    })

    it('keeps the newest answer when an earlier load answers last', async () => {
      const answers: ((list: Root[]) => void)[] = []
      vi.stubGlobal(
        'fetch',
        () =>
          new Promise<Response>((resolve) =>
            answers.push((list) =>
              resolve(new Response(JSON.stringify(list), { headers: { 'content-type': 'application/json' } })),
            ),
          ),
      )
      /** Answers the `nth` request made, counting from 0. */
      const answer = (nth: number, list: Root[]) => {
        const send = answers[nth]
        if (!send) throw new Error(`request ${nth} was not made`)
        send(list)
      }
      vi.stubGlobal('EventSource', FakeSource)
      const roots = new WatchedRoots(String)
      following(roots.follow())

      stream().send('root.registered', { event: 'root.registered', root: 'a' })
      stream().send('root.registered', { event: 'root.registered', root: 'b' })
      answer(1, [root('a'), root('b')])
      await settled()
      answer(0, [root('a')])
      await settled()

      expect(roots.roots?.map((r) => r.id)).toEqual(['a', 'b'])
    })
  })
})
