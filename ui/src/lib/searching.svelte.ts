/**
 * One search at a time, as the overlay runs it (ticket 66): a path search across every Root, or a content
 * search of the current Root or of every Root, streamed as it goes.
 *
 * Starting a search aborts the one before it, and only the newest one's results are kept. Cancelling, or
 * a newer search, closes the content stream's connection, which stops the daemon's walk; what was found
 * until then stays shown.
 */
import { searchContent, searchPaths } from './api'
import { emptyContentSearch, foldContent, type ContentSearch } from './search'
import type { PathSearch } from './wire'

/** What to search: paths, or content in `root` or in every Root. */
export interface SearchOptions {
  content: boolean
  /** The Root a content search reads unless `everyRoot`; `null` when there is none. */
  root: string | null
  everyRoot: boolean
}

/** The overlay's search: the newest one's results, its state, and the means to stop it. */
export class SearchRun {
  /** What the newest path search found. */
  paths = $state<PathSearch | null>(null)
  /** The newest content search, as far as its stream has come. */
  content = $state<ContentSearch | null>(null)
  /** Whether the newest search is still running. */
  running = $state(false)
  /** Whether the newest search was cancelled before its end. */
  cancelled = $state(false)
  /** Why the newest search failed. */
  error = $state<string | null>(null)

  readonly #failure: (e: unknown) => string
  #controller: AbortController | null = null

  /** `failure` turns a failed request into its text, and ends the session on a 401. */
  constructor(failure: (e: unknown) => string) {
    this.#failure = failure
  }

  /** Searches for `q`, replacing the search before; an empty query only clears. */
  async start(q: string, options: SearchOptions): Promise<void> {
    this.#controller?.abort()
    this.paths = null
    this.content = null
    this.error = null
    this.cancelled = false
    this.running = false
    if (q.trim() === '') {
      this.#controller = null
      return
    }
    const controller = new AbortController()
    this.#controller = controller
    this.running = true
    try {
      if (options.content) await this.#searchContent(q, options, controller.signal)
      else {
        const found = await searchPaths(q, controller.signal)
        if (!controller.signal.aborted) this.paths = found
      }
    } catch (e) {
      if (!controller.signal.aborted) this.error = this.#failure(e)
    } finally {
      if (this.#controller === controller) {
        this.running = false
        this.cancelled = controller.signal.aborted
      }
    }
  }

  /** Stops the running search, keeping what it found. */
  cancel(): void {
    this.#controller?.abort()
  }

  /** Streams a content search into a fresh `content`, record by record, until it ends or `signal` aborts. */
  async #searchContent(q: string, options: SearchOptions, signal: AbortSignal) {
    const scope = options.everyRoot || options.root === null ? 'all' : { roots: [options.root] }
    this.content = emptyContentSearch()
    // Folded into the reactive copy, so every record shows as it comes.
    const shown = this.content
    for await (const record of searchContent(q, scope, signal)) foldContent(shown, record)
  }
}
