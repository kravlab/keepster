/**
 * The Watched Roots as the page knows them (ticket 92): the Registry, loaded once and then kept current
 * from the live stream, so that a Root registered, unwatched or moved from the CLI or another tab shows
 * here at once. The header's Root picker and the Root management page both read it.
 *
 * Choosing which Root is shown is not this module's business: an event changes which Roots are held,
 * never which Root a screen shows.
 */
import { listRoots } from './api'
import { connectEvents } from './events'
import type { Root } from './wire'

/** The events that change the Registry: a Root added, gone, moved, or its state. */
const REGISTRY_EVENTS = ['root.registered', 'root.unregistered', 'root.moved', 'root.state_changed']

export class WatchedRoots {
  /** Every registered Root, in registration order; `null` until a load first succeeds. */
  roots = $state<Root[] | null>(null)
  /**
   * Why the newest load failed, as `failure` words it; empty once a load succeeds. A failure keeps the
   * Roots last loaded, so the page goes on showing them with this text beside them.
   */
  error = $state('')
  /* Counts the loads begun, so an answer to an earlier one, arriving late, is dropped. */
  #generation = 0

  /** @param failure the text a failed request shows, as `failureText` gives it */
  constructor(private readonly failure: (e: unknown) => string) {}

  /** Reads the Registry afresh; only the newest load's answer is kept. */
  async load(): Promise<void> {
    const mine = ++this.#generation
    try {
      const roots = await listRoots()
      if (mine !== this.#generation) return
      this.roots = roots
      this.error = ''
    } catch (e) {
      if (mine === this.#generation) this.error = this.failure(e)
    }
  }

  /**
   * Keeps `roots` current from the live stream until the returned function is called. Every change to
   * the Registry, and `resync`, reads it afresh rather than patching what is held: `root.registered` and
   * `root.moved` carry no record to patch with, and one way of following them all keeps `roots` exactly
   * what the daemon answers.
   */
  follow(): () => void {
    const live = connectEvents({
      names: REGISTRY_EVENTS,
      onEvent: () => void this.load(),
    })
    return () => live.close()
  }
}
