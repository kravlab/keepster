/**
 * Test support, imported by tests only: the browser's `EventSource` as the live stream opens it, driven by
 * hand, and a way to end every subscription a test started, so that the page-wide stream a test leaves
 * behind never reaches the next test.
 */
import type { EventSourceLike } from './events'

const CONNECTING = 0
const OPEN = 1
const CLOSED = 2

/** A stand-in for the browser's EventSource; every one opened is kept, newest last. */
export class FakeSource implements EventSourceLike {
  static opened: FakeSource[] = []
  readyState = CONNECTING
  closed = false
  private readonly listeners = new Map<string, ((e: { data: string }) => void)[]>()

  constructor(readonly url: string) {
    FakeSource.opened.push(this)
  }

  addEventListener(type: string, listener: (e: { data: string }) => void) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener])
  }

  close() {
    this.closed = true
    this.readyState = CLOSED
  }

  /** The connection is established. */
  open() {
    this.readyState = OPEN
    this.emit('open', '')
  }

  /** The daemon sends one event. */
  send(name: string, data: object) {
    this.emit(name, JSON.stringify(data))
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

  private emit(type: string, data: string) {
    for (const listener of this.listeners.get(type) ?? []) listener({ data })
  }
}

/** The stream opened last. */
export const stream = (): FakeSource => {
  const source = FakeSource.opened.at(-1)
  if (!source) throw new Error('no stream was opened')
  return source
}

const stops: (() => void)[] = []

/** Records a subscription's stop function, so that `stopFollowing` ends it; returns it unchanged. */
export const following = (stop: () => void): (() => void) => {
  stops.push(stop)
  return stop
}

/** Ends every subscription `following` recorded and forgets the streams opened; for `afterEach`. */
export const stopFollowing = () => {
  for (const stop of stops.splice(0)) stop()
  FakeSource.opened = []
}
