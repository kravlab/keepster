/**
 * A Root's timeline as a view mode holds it: the pages loaded so far, newest first, kept current. The rail,
 * the file's versions and the activity feed all list Snapshots this way; only what they draw differs.
 *
 * Pages are fetched by cursor. A load that starts afresh outdates every answer still on its way, so a
 * filter switched while a page loads never mixes two lists. The live stream keeps the list current while a
 * mode follows it: a new Snapshot of the Root goes on top, and `resync` loads the list afresh.
 */
import { timeline } from './api'
import { connectEvents } from './events'
import { withNewer } from './timeline'
import type { Snapshot } from './wire'

export class TimelinePages {
  snapshots = $state<Snapshot[]>([])
  /** Fetches the next, older page; `undefined` once the oldest is loaded. */
  cursor = $state<string | undefined>()
  /** Whether a load has ended, in success or not. */
  loaded = $state(false)
  /**
   * Whether a fresh load is on its way, the pages it walks back through included. The list may still lack
   * the Snapshot that load is after, so a selection must not be judged against it meanwhile.
   */
  loading = $state(false)
  /** Whether the next, older page is on its way. */
  paging = $state(false)
  error = $state('')
  /* Counts the fresh loads begun, so an answer to an earlier one is dropped. */
  #generation = 0

  /**
   * @param rootId the Root, fixed for the life of the list
   * @param failure the text a failed request shows, as `failureText` gives it
   * @param path with the per-file filter, the path the list is filtered by; read at each request
   */
  constructor(
    private readonly rootId: string,
    private readonly failure: (e: unknown) => string,
    private readonly path: () => string | undefined = () => undefined,
  ) {}

  /**
   * Loads the newest page afresh, then, if `until` is set, older pages until the list reaches back to it:
   * until it holds that Snapshot or an older one. A Snapshot the list does not hold (one the filter leaves
   * out) so stops the walk where it would have been, instead of at the oldest page. Snapshot ids order as
   * the Snapshots were taken.
   */
  async reload(until: string | null = null): Promise<void> {
    const mine = ++this.#generation
    const path = this.path()
    this.loading = true
    try {
      const page = await timeline(this.rootId, undefined, path)
      if (mine !== this.#generation) return
      this.snapshots = page.snapshots
      this.cursor = page.next_cursor
      while (until !== null && this.cursor !== undefined && !this.snapshots.some((s) => s.id <= until)) {
        const next = await timeline(this.rootId, this.cursor, path)
        if (mine !== this.#generation) return
        this.snapshots = [...this.snapshots, ...next.snapshots]
        this.cursor = next.next_cursor
      }
      this.error = ''
    } catch (e) {
      if (mine === this.#generation) this.error = this.failure(e)
    } finally {
      if (mine === this.#generation) {
        this.loading = false
        this.loaded = true
      }
    }
  }

  /** Puts Snapshots taken since the newest loaded one on top, or loads afresh when too many were. */
  async refresh(): Promise<void> {
    const mine = this.#generation
    try {
      const page = await timeline(this.rootId, undefined, this.path())
      if (mine !== this.#generation) return
      const merged = withNewer(this.snapshots, page.snapshots)
      if (merged === null) await this.reload()
      else this.snapshots = merged
    } catch (e) {
      this.error = this.failure(e)
    }
  }

  /** Appends the next, older page; nothing while there is none, or while one or a fresh load is on its way. */
  async older(): Promise<void> {
    if (this.cursor === undefined || this.paging || this.loading) return
    const mine = this.#generation
    this.paging = true
    try {
      const page = await timeline(this.rootId, this.cursor, this.path())
      if (mine !== this.#generation) return
      this.snapshots = [...this.snapshots, ...page.snapshots]
      this.cursor = page.next_cursor
    } catch (e) {
      if (mine === this.#generation) this.error = this.failure(e)
    } finally {
      this.paging = false
    }
  }

  /**
   * Keeps the list current from the live stream until the returned function is called: a new Snapshot of
   * this Root goes on top; on `resync`, when anything shown may be stale, `onResync` is told first and the
   * list is loaded afresh, back to `until`'s Snapshot as `reload` does.
   */
  follow({ until = () => null, onResync = () => {} }: FollowOptions = {}): () => void {
    const live = connectEvents({
      names: ['snapshot.created'],
      onEvent: ({ name, data }) => {
        if (name === 'resync') {
          onResync()
          void this.reload(until())
        } else if ((data as { root?: string }).root === this.rootId) void this.refresh()
      },
    })
    return () => live.close()
  }
}

/** How a mode follows the live stream. */
export interface FollowOptions {
  /** The Snapshot a reload after `resync` walks back to, read when it happens. */
  until?: () => string | null
  /** Told of a `resync` before the list is loaded afresh, to drop what else the mode keeps. */
  onResync?: () => void
}
