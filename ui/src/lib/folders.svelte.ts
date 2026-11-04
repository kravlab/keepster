/**
 * A folder browser over the daemon's machine (ticket 93), through `GET /v1/fs`: the page can name no path
 * of its own, so every directory it shows is one the daemon listed. Kept apart from Root management
 * because choosing where an Export writes (ticket 90) browses the same way.
 *
 * It starts where it is told to start, or the nearest directory above that can be listed, or else in the
 * home directory; it goes into a subdirectory, up to the parent, or to an absolute path typed or pasted.
 * The directory chosen is always the one listed: a place that cannot be listed is refused with the
 * daemon's reason and leaves the browser where it was.
 */
import { listDirectories } from './api'
import type { DirectoryListing, Subdirectory } from './wire'

/** How one listing went: now the one listed, refused for `error`, or overtaken by a newer listing. */
type ListOutcome = { kind: 'listed' } | { kind: 'refused'; error: unknown } | { kind: 'overtaken' }

/**
 * The browser's place on the daemon's machine: the directory listed, the path field over it, and why the
 * newest listing was refused. Only the newest listing asked for is kept, and the directory chosen, to add
 * a Root or to record where one has moved, is only ever one listed, as the path field shows it.
 */
export class FolderBrowser {
  /** The directory listed, as the daemon answered; `null` until a listing first succeeds. */
  listing = $state<DirectoryListing | null>(null)
  /** The directory chosen: the one listed. */
  path = $derived(this.listing?.path ?? null)
  /** Whether hidden directories are shown: off until the person asks for them. */
  showHidden = $state(false)
  /** The directories of the one listed, by name, hidden ones left out unless `showHidden`. */
  directories = $derived<Subdirectory[]>(
    (this.listing?.directories ?? []).filter((directory) => this.showHidden || !directory.hidden),
  )
  /** Whether the directory listed has one above it. */
  canGoUp = $derived(Boolean(this.listing?.parent))
  /**
   * Why the newest listing was refused, as `failure` words it — for a path the daemon finds nothing at,
   * no directory at, or cannot list, its own reason; empty once a listing succeeds.
   */
  error = $state('')
  /** What the path field holds: the directory listed, until the person types or pastes over it. */
  typed = $state('')
  /**
   * Whether the path field can be added from: something is listed and typed, and the newest listing was
   * not refused — a refused path stays in the field, and must not be taken for the directory listed.
   */
  canChoose = $derived(this.listing !== null && this.error === '' && this.typed.trim() !== '')
  /* Counts the listings asked for, so an answer to an earlier one, arriving late, is dropped. */
  #generation = 0

  /**
   * @param failure the text a failed request shows, as `failureText` gives it
   * @param start the absolute directory to list first, such as the one above a Root being moved; when
   *   it cannot be listed, the nearest directory above it that can, and the home directory without it or
   *   when none can
   */
  constructor(
    private readonly failure: (e: unknown) => string,
    private readonly start?: string,
  ) {}

  /**
   * Lists where the browser starts: `start`, when given and it can be listed, else the nearest directory
   * above it that can, climbing one level at a time, and the home directory when none can. A place
   * refused here is not the person's choice, so its refusal is passed over unsaid. The climb ends: each
   * `parentDirectory` is shorter than the path it is taken from, and a top has none. A newer listing
   * asked for meanwhile ends it too, as it is then the one kept.
   */
  async begin(): Promise<void> {
    for (let place = this.start ?? null; place !== null; place = parentDirectory(place)) {
      const outcome = await this.#list(place)
      if (outcome.kind !== 'refused') return
    }
    await this.open()
  }

  /**
   * Lists the absolute directory `path`, typed, pasted or picked, or the home directory without one; only
   * the newest answer is kept. A refusal keeps the directory listed before, with `error` saying why.
   * Resolves to whether this listing is now the one listed: not when refused or overtaken by a newer one.
   */
  async open(path?: string): Promise<boolean> {
    const outcome = await this.#list(path)
    if (outcome.kind === 'refused') this.error = this.failure(outcome.error)
    return outcome.kind === 'listed'
  }

  /**
   * Lists `path`, or the home directory without one, keeping the answer only while this is the newest
   * listing asked for. Says how it went and leaves a refusal to the caller, to show or pass over: a
   * refusal of a listing a newer one overtook is `overtaken`, as its answer is.
   */
  async #list(path: string | undefined): Promise<ListOutcome> {
    const mine = ++this.#generation
    try {
      const listing = await listDirectories(path)
      if (mine !== this.#generation) return { kind: 'overtaken' }
      this.listing = listing
      this.typed = listing.path
      this.error = ''
      return { kind: 'listed' }
    } catch (error) {
      return mine === this.#generation ? { kind: 'refused', error } : { kind: 'overtaken' }
    }
  }

  /**
   * The directory to add: the one the path field shows, never another. When the field, trimmed, is the
   * directory listed it is given as it is; otherwise it is listed first, and given only once that listing
   * succeeds. `null` for an empty field, a listing refused, or one a newer listing overtook.
   */
  async chosenPath(): Promise<string | null> {
    const path = this.typed.trim()
    if (path === '') return null
    if (path === this.listing?.path && this.error === '') return path
    return (await this.open(path)) ? this.path : null
  }

  /**
   * Takes the browser back to the directory listed for a new choice, as a new add starts: a refusal from
   * before is forgotten and the path field shows the listing again. The listing itself is kept.
   */
  startOver(): void {
    this.error = ''
    this.typed = this.listing?.path ?? ''
  }

  /** Lists a subdirectory of the one listed; one the daemon said cannot be listed is not asked for. */
  async enter(directory: Subdirectory): Promise<void> {
    if (directory.readable) await this.open(directory.path)
  }

  /** Lists the directory above the one listed; nothing at the top. */
  async up(): Promise<void> {
    const parent = this.listing?.parent
    if (parent) await this.open(parent)
  }
}

/** A Windows network share, `\\server\share`, or a server alone, with no trailing separator: a top. */
const UNC_SHARE = /^\\\\[^\\]+(\\[^\\]+)?$/

/** A top with its separator, as `parentDirectory` gives one: `/`, `C:\` or `\\server\share\`. */
const TOP = /^(\/|[A-Za-z]:[/\\]|\\\\[^\\]+\\[^\\]+\\)$/

/**
 * The directory above the absolute `path` on the daemon's machine, whose separator is `/`, or `\` on
 * Windows; `null` for a top such as `/`, `C:\` or the network share `\\server\share`, or a path with no
 * separator. A top is given with its separator, as `C:\` and `\\server\share\` are; a trailing
 * separator is not a level.
 */
export function parentDirectory(path: string): string | null {
  const trimmed = path.replace(/[/\\]+$/, '')
  if (UNC_SHARE.test(trimmed)) return null
  const cut = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
  if (cut < 0) return null
  const parent = trimmed.slice(0, cut + 1)
  return TOP.test(parent) ? parent : parent.slice(0, -1)
}
