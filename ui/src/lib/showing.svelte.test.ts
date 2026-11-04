import { afterEach, describe, expect, it, vi } from 'vitest'
import { WatchedRoots } from './roots.svelte'
import { ShownRoot } from './showing.svelte'
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
 * The Registry `registry()` answers, loaded and followed from the live stream, with `id` shown over it.
 * Each event sent then has the Registry read afresh; change what `registry` returns before sending one.
 * `shownFirst` subscribes the shown Root to the stream before the Registry: the other order than the
 * shell's.
 */
const showing = async (id: string, registry: () => Root[], { shownFirst = false } = {}) => {
  const asked = serve(registry)
  vi.stubGlobal('EventSource', FakeSource)
  const roots = new WatchedRoots(String)
  const shown = new ShownRoot(roots)
  await roots.load()
  if (shownFirst) following(shown.follow())
  following(roots.follow())
  if (!shownFirst) following(shown.follow())
  shown.show(id)
  return { roots, shown, asked }
}

afterEach(() => {
  stopFollowing()
  vi.unstubAllGlobals()
})

describe('the Root shown', () => {
  it('is the one chosen, with its record and its place in the picker', async () => {
    const { shown } = await showing('b', () => [root('a'), root('b')])

    expect(shown.id).toBe('b')
    expect(shown.current).toEqual(root('b'))
    expect(shown.unwatched).toBe(false)
    expect(shown.erasure).toBeNull()
    expect(shown.listed?.map((r) => r.id)).toEqual(['a', 'b'])
  })

  it('stays shown, with its last known record, when it is unwatched elsewhere', async () => {
    let registry = [root('a'), root('b', { state: 'degraded' }), root('c')]
    const { roots, shown } = await showing('b', () => registry)

    registry = [root('a'), root('c')]
    stream().send('root.unregistered', { event: 'root.unregistered', root: 'b', purged: false })
    await settled()

    expect(shown.id).toBe('b')
    expect(shown.current).toEqual(root('b', { state: 'degraded' }))
    expect(shown.unwatched).toBe(true)
    expect(shown.erasure).toBeNull()
    expect(roots.roots?.map((r) => r.id)).toEqual(['a', 'c'])
    expect(shown.listed?.map((r) => r.id)).toEqual(['a', 'b', 'c'])
  })

  it('is marked no longer watched as soon as the event says so, before the Registry answers', async () => {
    const { shown } = await showing('b', () => [root('a'), root('b')])
    vi.stubGlobal('fetch', () => new Promise<Response>(() => {}))

    stream().send('root.unregistered', { event: 'root.unregistered', root: 'b', purged: false })

    expect(shown.unwatched).toBe(true)
    expect(shown.current).toEqual(root('b'))
  })

  it('keeps its last known record whichever follows the stream first', async () => {
    let registry = [root('a'), root('b', { state: 'degraded' })]
    const { shown } = await showing('b', () => registry, { shownFirst: true })

    registry = [root('a')]
    stream().send('root.unregistered', { event: 'root.unregistered', root: 'b', purged: false })
    await settled()

    expect(shown.unwatched).toBe(true)
    expect(shown.current).toEqual(root('b', { state: 'degraded' }))
    expect(shown.listed?.map((r) => r.id)).toEqual(['a', 'b'])
  })

  it('leaves the picker once another Root is chosen', async () => {
    let registry = [root('a'), root('b')]
    const { shown } = await showing('b', () => registry)
    registry = [root('a')]
    stream().send('root.unregistered', { event: 'root.unregistered', root: 'b', purged: false })
    await settled()

    shown.show('a')

    expect(shown.listed?.map((r) => r.id)).toEqual(['a'])
    expect(shown.unwatched).toBe(false)
    expect(shown.current).toEqual(root('a'))
  })

  it('is kept when a resync finds it gone, and watched again once the Registry holds it again', async () => {
    let registry = [root('a'), root('b')]
    const { shown } = await showing('b', () => registry)

    registry = [root('a')]
    stream().send('resync', { event: 'resync' })
    await settled()

    expect(shown.unwatched).toBe(true)
    expect(shown.listed?.map((r) => r.id)).toEqual(['a', 'b'])

    registry = [root('a'), root('b', { path: '/mnt/b' })]
    stream().send('resync', { event: 'resync' })
    await settled()

    expect(shown.unwatched).toBe(false)
    expect(shown.current).toEqual(root('b', { path: '/mnt/b' }))
    expect(shown.listed?.map((r) => r.id)).toEqual(['a', 'b'])
  })

  it('keeps nothing for a Root it never held, as one just added and not yet listed', async () => {
    const { shown } = await showing('new', () => [root('a')])

    stream().send('resync', { event: 'resync' })
    await settled()

    expect(shown.current).toBeNull()
    expect(shown.unwatched).toBe(false)
    expect(shown.listed?.map((r) => r.id)).toEqual(['a'])
  })

  it('says its history was erased, to be refreshed while it is still watched', async () => {
    const { shown, asked } = await showing('b', () => [root('a'), root('b')])

    stream().send('history.erased', { event: 'history.erased', root: 'b' })
    await settled()

    expect(shown.erasure).toBe('refresh')
    expect(shown.id).toBe('b')
    expect(asked).toEqual(['/v1/roots'])

    shown.acknowledgeErasure()

    expect(shown.erasure).toBeNull()
  })

  it('leads to the management page once its history is erased and it is no longer watched', async () => {
    let registry = [root('a'), root('b')]
    const { shown } = await showing('b', () => registry)
    registry = [root('a')]
    stream().send('root.unregistered', { event: 'root.unregistered', root: 'b', purged: false })
    await settled()

    stream().send('history.erased', { event: 'history.erased', root: 'b' })
    await settled()

    expect(shown.erasure).toBe('manage')
    expect(shown.listed?.map((r) => r.id)).toEqual(['a', 'b'])
  })

  it('turns the banner to the management page when a Root whose history was erased is unwatched', async () => {
    let registry = [root('a'), root('b')]
    const { shown } = await showing('b', () => registry)
    stream().send('history.erased', { event: 'history.erased', root: 'b' })

    registry = [root('a')]
    stream().send('root.unregistered', { event: 'root.unregistered', root: 'b', purged: false })
    await settled()

    expect(shown.erasure).toBe('manage')
  })

  it('says its history was erased when it is unwatched with its history', async () => {
    let registry = [root('a'), root('b')]
    const { shown } = await showing('b', () => registry)

    registry = [root('a')]
    stream().send('root.unregistered', { event: 'root.unregistered', root: 'b', purged: true })
    await settled()

    expect(shown.id).toBe('b')
    expect(shown.unwatched).toBe(true)
    expect(shown.erasure).toBe('manage')
    expect(shown.listed?.map((r) => r.id)).toEqual(['a', 'b'])
  })

  it('forgets an erasure once another Root is chosen', async () => {
    const { shown } = await showing('b', () => [root('a'), root('b')])
    stream().send('history.erased', { event: 'history.erased', root: 'b' })

    shown.show('a')

    expect(shown.erasure).toBeNull()
  })

  it('is not changed by an event about another Root', async () => {
    let registry = [root('a'), root('b'), root('c')]
    const { shown } = await showing('b', () => registry)

    stream().send('history.erased', { event: 'history.erased', root: 'a' })
    registry = [root('b'), root('c')]
    stream().send('root.unregistered', { event: 'root.unregistered', root: 'a', purged: true })
    await settled()

    expect(shown.id).toBe('b')
    expect(shown.current).toEqual(root('b'))
    expect(shown.unwatched).toBe(false)
    expect(shown.erasure).toBeNull()
    expect(shown.listed?.map((r) => r.id)).toEqual(['b', 'c'])
  })
})
