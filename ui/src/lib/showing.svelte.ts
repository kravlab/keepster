/**
 * The Root the shell shows, over the Watched Roots (#33). The shell says which one with `show`; an event
 * never changes which Root is shown. A shown Root that stops being watched stays shown: its last known
 * record is kept, marked, and stays in the picker where it was until another Root is chosen, because the
 * reading endpoints still serve its history by identity. Its history erased is told as an `erasure`, for
 * the screen to say so.
 *
 * It reads the Registry from `WatchedRoots` and never holds a copy of it: whether the shown Root is
 * watched again, and its record while it is, are derived from the Registry as it stands. What it keeps is
 * only what the Registry no longer holds — the shown Root's last known record and place — taken from the
 * Registry as each event arrives. That read is never stale, whichever of the two subscribed to the stream
 * first: the stream delivers an event to every subscriber at once, and `WatchedRoots` answers one only by
 * starting a request, so its `roots` change when the request answers, after every subscriber was told.
 */
import type { Erasure } from './erasure'
import { connectEvents } from './events'
import { REGISTRY_EVENTS, type WatchedRoots } from './roots.svelte'
import type { Root } from './wire'

/** A Root's history erased: the Registry is unchanged, only the shown Root's history is concerned. */
const ERASED = 'history.erased'

/** The fields of `root.unregistered` and `history.erased` this module reads. */
interface RootEvent {
  root?: string
  /** On `root.unregistered`: whether its history was erased with it. */
  purged?: boolean
}

/**
 * Which Root is shown, chosen with `show`, and what the screen says of it: its record (`current`),
 * whether it is no longer watched, whether its history was erased, and the Roots the picker offers.
 */
export class ShownRoot {
  /* The Watched Roots the shown Root is one of. */
  readonly #registry: WatchedRoots
  /* The Root shown, by id; `null` while none is. */
  #id = $state<string | null>(null)
  /* The shown Root's record and place in the Registry, as the Registry held them when an event last arrived. */
  #last = $state.raw<{ root: Root; at: number } | null>(null)
  /*
   * The Registry as it stood when `root.unregistered` named the shown Root: until a load replaces it — every
   * answer is a new list — the shown Root is no longer watched, though that list still holds it.
   */
  #goneFrom = $state.raw<Root[] | null>(null)
  /* Whether the shown Root's history was erased since it was chosen or the banner was last answered. */
  #erased = $state(false)

  /*
   * The derived fields read the Registry, so they are derived in the constructor, once it is given; each
   * is declared here, with what it is.
   */
  /* The shown Root's record in the Registry as it stands; `null` while the Registry does not hold it. */
  readonly #held: Root | null
  /** Whether the shown Root is no longer watched, its last known record kept. */
  readonly unwatched: boolean
  /** The shown Root's record: the Registry's while it holds it, the last known one once it does not. */
  readonly current: Root | null
  /** What the screen offers since the shown Root's history was erased; `null` while it was not. */
  readonly erasure: Erasure | null
  /**
   * The Roots the picker offers: the Registry, and the shown Root, no longer watched, where it was in it;
   * `null` until the Registry is first loaded.
   */
  readonly listed: Root[] | null

  /** @param registry the Watched Roots the shown Root is one of */
  constructor(registry: WatchedRoots) {
    this.#registry = registry
    this.#held = $derived(this.#registry.roots?.find((r) => r.id === this.#id) ?? null)
    this.unwatched = $derived(
      this.#last !== null &&
        (this.#held === null || (this.#goneFrom !== null && this.#registry.roots === this.#goneFrom)),
    )
    this.current = $derived(this.#held ?? this.#last?.root ?? null)
    this.erasure = $derived(this.#erased ? (this.unwatched ? 'manage' : 'refresh') : null)
    this.listed = $derived.by(() => {
      const roots = this.#registry.roots
      const last = this.#last
      if (roots === null || last === null || this.#held !== null) return roots
      return [...roots.slice(0, last.at), last.root, ...roots.slice(last.at)]
    })
  }

  /** The Root shown, by id, as `show` was last told; `null` while none is. */
  get id(): string | null {
    return this.#id
  }

  /**
   * Follows the live stream until the returned function is called: every event that reloads the Registry
   * first has the shown Root's record kept as the Registry holds it, `root.unregistered` naming the shown
   * Root marks it no longer watched before the Registry answers, and that event, purged, or
   * `history.erased` naming it marks its history erased. An event about another Root changes nothing.
   */
  follow(): () => void {
    const live = connectEvents({
      names: [...REGISTRY_EVENTS, ERASED],
      onEvent: ({ name, data }) => {
        const event = data as RootEvent
        const ours = this.#id !== null && event.root === this.#id
        if (name === ERASED) {
          if (ours) this.#erased = true
          return
        }
        this.#remember()
        if (name === 'root.unregistered' && ours) {
          this.#goneFrom = this.#registry.roots
          if (event.purged === true) this.#erased = true
        }
      },
    })
    return () => live.close()
  }

  /**
   * Shows the Root `id`, or none: what the shell calls whenever it chooses one. Choosing another Root
   * drops a kept record from the picker and forgets an erasure.
   */
  show(id: string | null): void {
    if (id === this.#id) return
    this.#id = id
    this.#last = null
    this.#goneFrom = null
    this.#erased = false
  }

  /** The banner was answered by showing the history afresh: what it shows now is what the daemon holds. */
  acknowledgeErasure(): void {
    this.#erased = false
  }

  /* Keeps the shown Root's record and place as the Registry holds them now, if it does. */
  #remember() {
    const roots = this.#registry.roots
    const at = roots?.findIndex((r) => r.id === this.#id) ?? -1
    const root = roots?.[at]
    if (root) this.#last = { root, at }
  }
}
