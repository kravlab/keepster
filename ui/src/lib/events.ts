/**
 * The daemon's live event stream, `GET /v1/events` (ticket 45), kept open for the life of the page.
 *
 * One connection serves the whole page (ticket 92): every subscriber — a view mode's timeline, the Root
 * list — shares it, each receiving the events it named. A page holding one long-lived connection per
 * subscriber would spend the browser's few connections to the daemon's origin on streams, and with a few
 * tabs open the plain requests would queue behind them. The connection opens with the first subscriber
 * and closes with the last.
 *
 * Two ways a stream ends, handled differently:
 * - The connection drops. The browser reconnects by itself and sends `Last-Event-ID`, so the daemon
 *   replays what was missed, or answers `resync` when it no longer holds that event. Nothing to do here.
 * - The browser gives up (`readyState` CLOSED): the daemon answered something other than an event stream,
 *   such as a 401 or a 5xx. A new `EventSource` is opened after a wait that doubles from 1 s up to 30 s.
 *   A new source cannot send `Last-Event-ID`, so whatever happened in between is lost: once it opens, every
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
  /**
   * Ends this subscription: no event is delivered to it after this. The last subscription to end closes
   * the connection and cancels any pending reopen.
   */
  close(): void
}

/** One subscriber: the names it asked for, and where its events go. */
interface Subscriber {
  names: ReadonlySet<string>
  onEvent: (event: LiveEvent) => void
}

/** The page's one connection and everyone listening to it. */
class SharedStream {
  readonly #subscribers = new Set<Subscriber>()
  #source: EventSourceLike | null = null
  /* The names the current source has a listener for: `EventSource` delivers only names it listens to. */
  #listening = new Set<string>()
  #reopen: ReturnType<typeof setTimeout> | null = null
  #wait = FIRST_WAIT_MS

  /** @param create opens a source: the first subscriber's choice, for the life of this stream */
  constructor(private readonly create: (url: string) => EventSourceLike) {
    this.#open(false)
  }

  /** Starts delivering `subscriber`'s names to it. */
  add(subscriber: Subscriber) {
    this.#subscribers.add(subscriber)
    for (const name of subscriber.names) this.#listen(name)
  }

  /** Stops delivering to `subscriber`; with no one left, closes the connection and cancels any reopen. */
  remove(subscriber: Subscriber) {
    this.#subscribers.delete(subscriber)
    if (this.#subscribers.size > 0) return
    if (this.#reopen !== null) clearTimeout(this.#reopen)
    this.#source?.close()
    this.#source = null
    if (shared === this) shared = null
  }

  /** Delivers one event to every subscriber that asked for it; `resync` goes to all. */
  #deliver(event: LiveEvent) {
    for (const subscriber of [...this.#subscribers]) {
      if (event.name === RESYNC || subscriber.names.has(event.name)) subscriber.onEvent(event)
    }
  }

  /** Adds a listener for `name` to the current source, once. */
  #listen(name: string) {
    const current = this.#source
    if (current === null || this.#listening.has(name)) return
    this.#listening.add(name)
    current.addEventListener(name, (event) => {
      if (this.#source === current) this.#deliver({ name, data: JSON.parse(event.data) as unknown })
    })
  }

  /** Opens a new source; `reopened` when it replaces one the browser gave up on, which owes a `resync`. */
  #open(reopened: boolean) {
    const current = this.create(EVENTS_URL)
    this.#source = current
    this.#listening = new Set()
    this.#listen(RESYNC)
    for (const subscriber of this.#subscribers) for (const name of subscriber.names) this.#listen(name)
    current.addEventListener('open', () => {
      this.#wait = FIRST_WAIT_MS
      if (reopened) {
        reopened = false
        this.#deliver({ name: RESYNC, data: { event: RESYNC } })
      }
    })
    current.addEventListener('error', () => {
      if (current.readyState !== CLOSED || this.#source !== current) return
      this.#reopen = setTimeout(() => {
        this.#reopen = null
        if (this.#source === current) this.#open(true)
      }, this.#wait)
      this.#wait = Math.min(this.#wait * 2, LONGEST_WAIT_MS)
    })
  }
}

/* The page's stream while anyone subscribes to it. */
let shared: SharedStream | null = null

/**
 * Subscribes to the page's stream, opening it if no one else has, until `close`. `create` is used only
 * when this call opens the stream.
 */
export function connectEvents({ names, onEvent, create = (url) => new EventSource(url) }: EventsOptions): LiveEvents {
  const stream = (shared ??= new SharedStream(create))
  const subscriber: Subscriber = { names: new Set(names), onEvent }
  stream.add(subscriber)
  let closed = false
  return {
    close() {
      if (closed) return
      closed = true
      stream.remove(subscriber)
    },
  }
}
