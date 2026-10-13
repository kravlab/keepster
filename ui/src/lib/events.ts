/**
 * The daemon's live event stream, `GET /v1/events` (ticket 45), kept open for the life of the page.
 *
 * Two ways a stream ends, handled differently:
 * - The connection drops. The browser reconnects by itself and sends `Last-Event-ID`, so the daemon
 *   replays what was missed, or answers `resync` when it no longer holds that event. Nothing to do here.
 * - The browser gives up (`readyState` CLOSED): the daemon answered something other than an event stream,
 *   such as a 401 or a 5xx. A new `EventSource` is opened after a wait that doubles from 1 s up to 30 s.
 *   A new source cannot send `Last-Event-ID`, so whatever happened in between is lost: once it opens, the
 *   subscriber is told `resync`, exactly as if the daemon had said it.
 */

/** Where the stream is served. */
const EVENTS_URL = '/v1/events'
const FIRST_WAIT_MS = 1_000
const LONGEST_WAIT_MS = 30_000
/** The event that says what is shown may be stale and must be fetched again. */
const RESYNC = 'resync'
/** `EventSource.CLOSED`, spelled out so this module never touches the browser global. */
const CLOSED = 2

/** One event: its name (the `event:` field) and its parsed `data:`, one JSON `Event`. */
export interface LiveEvent {
  name: string
  data: unknown
}

/** The part of the browser's `EventSource` this module uses. */
export interface EventSourceLike {
  readonly readyState: number
  /** `open` and `error` as well as the named events; only a named event's `data` is read. */
  addEventListener(type: string, listener: (event: { data: string }) => void): void
  close(): void
}

/** What to listen for, and how to open the stream. */
export interface EventsOptions {
  /**
   * The event names to deliver besides `resync`, which always is: it says anything shown may be stale.
   * Other names are passed over, as `/v1` asks of a client.
   */
  names: readonly string[]
  /** Called with each delivered event, in order. */
  onEvent: (event: LiveEvent) => void
  /** Opens a source; the browser's `EventSource` unless a test supplies one. */
  create?: (url: string) => EventSourceLike
}

/** A running subscription. */
export interface LiveEvents {
  /** Closes the stream and cancels any pending reopen; no event is delivered after it. */
  close(): void
}

/** Opens the stream and keeps it open until `close`. */
export function connectEvents({ names, onEvent, create = (url) => new EventSource(url) }: EventsOptions): LiveEvents {
  let source: EventSourceLike | null = null
  let reopen: ReturnType<typeof setTimeout> | null = null
  let wait = FIRST_WAIT_MS
  let closed = false

  /** Opens a new source; `reopened` when it replaces one the browser gave up on, which owes a `resync`. */
  const open = (reopened: boolean) => {
    const current = create(EVENTS_URL)
    source = current
    for (const name of new Set([...names, RESYNC])) {
      current.addEventListener(name, (event) => {
        if (source === current) onEvent({ name, data: JSON.parse(event.data) as unknown })
      })
    }
    current.addEventListener('open', () => {
      wait = FIRST_WAIT_MS
      if (reopened) {
        reopened = false
        onEvent({ name: RESYNC, data: { event: RESYNC } })
      }
    })
    current.addEventListener('error', () => {
      if (current.readyState !== CLOSED || source !== current) return
      reopen = setTimeout(() => {
        reopen = null
        if (!closed) open(true)
      }, wait)
      wait = Math.min(wait * 2, LONGEST_WAIT_MS)
    })
  }

  open(false)
  return {
    close() {
      closed = true
      if (reopen !== null) clearTimeout(reopen)
      source?.close()
      source = null
    },
  }
}
