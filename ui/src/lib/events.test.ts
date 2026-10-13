import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { connectEvents, type EventSourceLike, type LiveEvent } from './events'

const CONNECTING = 0
const OPEN = 1
const CLOSED = 2

/** A stand-in for the browser's EventSource that the test drives by hand. */
class FakeSource implements EventSourceLike {
  readyState = CONNECTING
  closed = false
  private readonly listeners = new Map<string, ((e: { data: string }) => void)[]>()

  constructor(readonly url: string) {}

  addEventListener(type: string, listener: (e: { data: string }) => void) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener])
  }

  close() {
    this.closed = true
    this.readyState = CLOSED
  }

  open() {
    this.readyState = OPEN
    this.emit('open', '')
  }

  send(name: string, data: object) {
    this.emit(name, JSON.stringify(data))
  }

  private emit(type: string, data: string) {
    for (const listener of this.listeners.get(type) ?? []) listener({ data })
  }

  /** The connection dropped and the browser is reconnecting on its own, with `Last-Event-ID`. */
  drop() {
    this.readyState = CONNECTING
    this.emit('error', '')
  }

  /** The browser gave up on this source: a non-200 answer, or a body that is not an event stream. */
  fail() {
    this.readyState = CLOSED
    this.emit('error', '')
  }
}

let sources: FakeSource[]
let seen: LiveEvent[]
const latest = () => sources[sources.length - 1]!

const connect = () =>
  connectEvents({
    names: ['snapshot.created'],
    onEvent: (e) => seen.push(e),
    create: (url) => {
      const source = new FakeSource(url)
      sources.push(source)
      return source
    },
  })

beforeEach(() => {
  vi.useFakeTimers()
  sources = []
  seen = []
})

afterEach(() => {
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

    expect(sources).toHaveLength(1)
    expect(seen).toEqual([])
  })

  it('reopens a stream the browser gave up on, and says to resync once it is open', () => {
    connect()
    latest().open()
    latest().fail()

    vi.advanceTimersByTime(999)
    expect(sources).toHaveLength(1)
    vi.advanceTimersByTime(1)
    expect(sources).toHaveLength(2)
    expect(seen).toEqual([])

    latest().open()
    expect(seen).toEqual([{ name: 'resync', data: { event: 'resync' } }])
  })

  it('waits twice as long after each failed reopen, up to 30 seconds', () => {
    connect()
    latest().fail()
    const waits: number[] = []
    for (let i = 0; i < 7; i++) {
      const before = sources.length
      let waited = 0
      while (sources.length === before) {
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
    expect(sources).toHaveLength(4)
  })

  it('stops for good when closed, even with a reopen pending', () => {
    const live = connect()
    latest().fail()
    live.close()
    vi.advanceTimersByTime(60_000)

    expect(sources).toHaveLength(1)
    expect(latest().closed).toBe(true)
  })

  it('closes the open stream when closed', () => {
    const live = connect()
    latest().open()
    live.close()
    latest().send('snapshot.created', { event: 'snapshot.created' })

    expect(latest().closed).toBe(true)
    expect(seen).toEqual([])
  })
})
