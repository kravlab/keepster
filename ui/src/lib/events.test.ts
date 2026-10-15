import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { connectEvents, type LiveEvent } from './events'
import { FakeSource, following, stopFollowing, stream } from './testing'

const sources = () => FakeSource.opened
let seen: LiveEvent[]
const latest = () => stream()

/** Subscribes to `names` (`snapshot.created` unless given), recording what is delivered into `into`. */
const connect = (names: readonly string[] = ['snapshot.created'], into: LiveEvent[] = seen) =>
  following(
    connectEvents({
      names,
      onEvent: (e) => into.push(e),
      create: (url) => new FakeSource(url),
    }).close,
  )

beforeEach(() => {
  vi.useFakeTimers()
  seen = []
})

afterEach(() => {
  stopFollowing()
  vi.useRealTimers()
})

describe('connectEvents', () => {
  it('opens the daemon stream and delivers the events asked for', () => {
    connect()
    latest().open()
    latest().send('snapshot.created', { event: 'snapshot.created', root: 'r', snapshot: 's' })
    latest().send('root.moved', { event: 'root.moved', root: 'r' })

    expect(latest().url).toBe('/v1/events')
    expect(seen).toEqual([{ name: 'snapshot.created', data: { event: 'snapshot.created', root: 'r', snapshot: 's' } }])
  })

  it("delivers the daemon's own resync without being asked", () => {
    connect()
    latest().send('resync', { event: 'resync' })

    expect(seen).toEqual([{ name: 'resync', data: { event: 'resync' } }])
  })

  it('leaves a dropped connection to the browser, which resumes with Last-Event-ID', () => {
    connect()
    latest().open()
    latest().drop()
    vi.advanceTimersByTime(60_000)

    expect(sources()).toHaveLength(1)
    expect(seen).toEqual([])
  })

  it('reopens a stream the browser gave up on, and says to resync once it is open', () => {
    connect()
    latest().open()
    latest().fail()

    vi.advanceTimersByTime(999)
    expect(sources()).toHaveLength(1)
    vi.advanceTimersByTime(1)
    expect(sources()).toHaveLength(2)
    expect(seen).toEqual([])

    latest().open()
    expect(seen).toEqual([{ name: 'resync', data: { event: 'resync' } }])
  })

  it('waits twice as long after each failed reopen, up to 30 seconds', () => {
    connect()
    latest().fail()
    const waits: number[] = []
    for (let i = 0; i < 7; i++) {
      const before = sources().length
      let waited = 0
      while (sources().length === before) {
        vi.advanceTimersByTime(1000)
        waited += 1000
      }
      waits.push(waited)
      latest().fail()
    }

    expect(waits).toEqual([1000, 2000, 4000, 8000, 16000, 30000, 30000])
  })

  it('starts the wait over once a reopened stream has opened', () => {
    connect()
    latest().fail()
    vi.advanceTimersByTime(1000)
    latest().fail()
    vi.advanceTimersByTime(2000)
    latest().open()
    latest().fail()

    vi.advanceTimersByTime(1000)
    expect(sources()).toHaveLength(4)
  })

  it('stops for good when closed, even with a reopen pending', () => {
    const close = connect()
    latest().fail()
    close()
    vi.advanceTimersByTime(60_000)

    expect(sources()).toHaveLength(1)
    expect(latest().closed).toBe(true)
  })

  it('closes the open stream when closed', () => {
    const close = connect()
    latest().open()
    close()
    latest().send('snapshot.created', { event: 'snapshot.created' })

    expect(latest().closed).toBe(true)
    expect(seen).toEqual([])
  })

  describe('one stream for the whole page', () => {
    it('serves every subscriber from one connection, each with the events it asked for', () => {
      const rootEvents: LiveEvent[] = []
      connect(['snapshot.created'])
      connect(['root.moved'], rootEvents)
      latest().open()

      latest().send('snapshot.created', { event: 'snapshot.created' })
      latest().send('root.moved', { event: 'root.moved' })

      expect(sources()).toHaveLength(1)
      expect(seen).toEqual([{ name: 'snapshot.created', data: { event: 'snapshot.created' } }])
      expect(rootEvents).toEqual([{ name: 'root.moved', data: { event: 'root.moved' } }])
    })

    it('tells every subscriber to resync', () => {
      const rootEvents: LiveEvent[] = []
      connect(['snapshot.created'])
      connect(['root.moved'], rootEvents)

      latest().send('resync', { event: 'resync' })

      expect(seen).toEqual([{ name: 'resync', data: { event: 'resync' } }])
      expect(rootEvents).toEqual([{ name: 'resync', data: { event: 'resync' } }])
    })

    it('keeps the connection while any subscriber remains, and closes it with the last', () => {
      const rootEvents: LiveEvent[] = []
      const first = connect(['snapshot.created'])
      const second = connect(['root.moved'], rootEvents)

      first()
      latest().send('root.moved', { event: 'root.moved' })
      expect(latest().closed).toBe(false)
      expect(rootEvents).toHaveLength(1)

      second()
      expect(latest().closed).toBe(true)
    })

    it('opens a new connection for a subscriber that comes after the last one left', () => {
      connect()()
      connect()

      expect(sources()).toHaveLength(2)
      expect(latest().closed).toBe(false)
    })

    it('carries every subscriber over to a reopened stream, each told to resync once', () => {
      const rootEvents: LiveEvent[] = []
      connect(['snapshot.created'])
      connect(['root.moved'], rootEvents)
      latest().fail()
      vi.advanceTimersByTime(1000)
      latest().open()
      latest().send('root.moved', { event: 'root.moved' })

      expect(sources()).toHaveLength(2)
      expect(seen).toEqual([{ name: 'resync', data: { event: 'resync' } }])
      expect(rootEvents).toEqual([
        { name: 'resync', data: { event: 'resync' } },
        { name: 'root.moved', data: { event: 'root.moved' } },
      ])
    })
  })
})
