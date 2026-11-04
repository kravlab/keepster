/**
 * Unwatching one Root (#31), as the state the unwatch dialog draws, as `adding.svelte.ts` is for the add
 * dialog: the consent `unwatch.ts` rules on, what the panel says of the history, the request on its way,
 * and the daemon's reason for a refusal.
 *
 * What becomes of the history is said as `keepster unwatch` says it: it is kept and its last state pinned
 * only for a Root with a pinned Snapshot, and a Root with none has no history to keep. Unlike the CLI,
 * which reads the pins after unwatching, the dialog reads them as it opens, so it can say so before the
 * person decides; until they answer, or when they cannot be read, the panel claims neither.
 */
import { readPins, SessionEnded, unwatchRoot } from './api'
import type { Root } from './wire'
import { acknowledge, chooseErasing, noConsent, purges, ready, type Consent } from './unwatch'

/** What is known of the Root's history before it is unwatched. */
type History =
  /** The pins are not read yet, or could not be: nothing is claimed either way. */
  | 'unknown'
  /** The Root has a pinned Snapshot: its history is kept, its last state pinned. */
  | 'kept'
  /** The Root has no pinned Snapshot: it never captured anything, so there is no history to keep. */
  | 'none'

/** What the panel says becomes of the history, a key of `strings.unwatch.outlook`. */
export type Outlook = 'kept' | 'nothingToKeep' | 'unknown' | 'erasing'

/**
 * One Root's unwatch, from the dialog opening to the Root unwatched. A refusal leaves the consent as it
 * was, to try again or back out; a request on its way holds every further one back.
 */
export class UnwatchingRoot {
  /** What the person has chosen and acknowledged so far. */
  /** The id of the Root to unwatch. */
  readonly id: string
  consent = $state<Consent>(noConsent())
  /** Whether the request is on its way. */
  running = $state(false)
  /** The daemon's reason for the last refusal, as `failure` words it; empty while there is none. */
  error = $state('')
  /** Whether the Root is unwatched: the dialog closes. */
  done = $state(false)
  /**
   * The Root as the live list last had it: the one the dialog was opened for until the page hands over a
   * fresher one, so the dialog still has a Root to name once the list drops it.
   */
  lastSeen: Root
  #history = $state<History>('unknown')

  /** Whether the request may go: every acknowledgement the choice needs is given, and none is on its way. */
  readonly canGo = $derived(ready(this.consent) && !this.running)

  /** What the panel says becomes of the history: erased once that is chosen, otherwise what the pins say. */
  readonly outlook: Outlook = $derived.by(() => {
    if (this.consent.erase) return 'erasing'
    if (this.#history === 'kept') return 'kept'
    if (this.#history === 'none') return 'nothingToKeep'
    return 'unknown'
  })

  /**
   * @param root the Root to unwatch, as the list shows it now
   * @param failure the text a failed request shows, as `failureText` gives it; an ended session is
   *   handed to it too, since it ends every screen
   */
  constructor(
    root: Root,
    private readonly failure: (e: unknown) => string,
  ) {
    this.id = root.id
    this.lastSeen = $state.raw(root)
  }

  /**
   * Reads the Root's pins, to say whether there is a history to keep. A read that fails says nothing
   * either way, so the panel stays neutral; only an ended session is passed on.
   */
  async readHistory(): Promise<void> {
    try {
      const pins = await readPins(this.id)
      this.#history = pins.snapshots.length > 0 ? 'kept' : 'none'
    } catch (e) {
      if (e instanceof SessionEnded) this.failure(e)
    }
  }

  /** Chooses erasing the history as well, or withdraws it, as `unwatch.chooseErasing` rules. */
  chooseErasing(erase: boolean): void {
    this.consent = chooseErasing(this.consent, erase)
  }

  /** Gives or withdraws the acknowledgement that erasing is irreversible, as `unwatch.acknowledge` rules. */
  acknowledge(acknowledged: boolean): void {
    this.consent = acknowledge(this.consent, acknowledged)
  }

  /**
   * Sends `DELETE /v1/roots/{id}`, with `purge=true` only if erasing was chosen and acknowledged, and is
   * `done` once the daemon unwatched the Root. A refusal sets `error` and leaves the consent as it was.
   * Nothing is sent before the consent is ready or while a request is on its way.
   */
  async go(): Promise<void> {
    if (!this.canGo) return
    this.running = true
    this.error = ''
    try {
      await unwatchRoot(this.id, purges(this.consent))
      this.done = true
    } catch (e) {
      this.error = this.failure(e)
    } finally {
      this.running = false
    }
  }
}
