/**
 * The preferences the page shows (ticket 66): what the daemon keeps, shown before the daemon answers and
 * changed optimistically.
 *
 * A page load paints from the cache, then [`load`](PreferencesState.load) takes the daemon's answer. A
 * change is shown at once and sent whole; the daemon's answer is what is kept, and a refusal or failure
 * goes back to what it last accepted and says why. Only the newest change's answer counts, so a slow
 * answer to an older one cannot undo a newer one.
 */
import { createContext } from 'svelte'
import { readPreferences, writePreferences } from './api'
import {
  DEFAULT_PREFERENCES,
  applyTheme,
  browserStorage,
  readCache,
  writeCache,
  type CacheStorage,
  type ThemeTarget,
} from './preferences'
import type { Preferences } from './wire'

/** Where the preferences are remembered and painted; the browser's own unless a test hands in others. */
export interface PreferencesPlaces {
  storage?: CacheStorage | undefined
  root?: ThemeTarget
}

/** The page's preferences: shown, changed, and kept in step with the daemon and the paint cache. */
export class PreferencesState {
  /** The preferences shown: the newest change while it is being stored, otherwise what the daemon has. */
  current = $state<Preferences>({ ...DEFAULT_PREFERENCES })
  /**
   * Why the daemon could not be read, or why the newest change was not stored, as the failure's text;
   * `null` once an answer comes.
   */
  error = $state<{ when: 'read' | 'save'; text: string } | null>(null)

  readonly #failure: (e: unknown) => string
  readonly #storage: CacheStorage | undefined
  readonly #root: ThemeTarget
  /** What the daemon last answered: where a refused change goes back to. */
  #accepted: Preferences
  /** Counts the changes sent; an answer counts only while its change is the newest. */
  #sent = 0

  /** `failure` turns a failed request into its text, and ends the session on a 401. */
  constructor(failure: (e: unknown) => string, places: PreferencesPlaces = {}) {
    this.#failure = failure
    this.#storage = 'storage' in places ? places.storage : browserStorage()
    this.#root = places.root ?? document.documentElement
    this.#accepted = readCache(this.#storage) ?? { ...DEFAULT_PREFERENCES }
    this.#show(this.#accepted)
  }

  /** Takes the daemon's preferences, unless a change was sent meanwhile: its answer will say. */
  async load(): Promise<void> {
    const sent = this.#sent
    try {
      const answer = await readPreferences()
      if (sent === this.#sent) this.#keep(answer)
    } catch (e) {
      if (sent === this.#sent) this.error = { when: 'read', text: this.#failure(e) }
    }
  }

  /** Shows `patch` over the current preferences at once, and stores the whole of them. */
  async change(patch: Partial<Preferences>): Promise<void> {
    const sent = ++this.#sent
    const wanted = { ...this.current, ...patch }
    this.#show(wanted)
    try {
      const answer = await writePreferences(wanted)
      if (sent === this.#sent) this.#keep(answer)
    } catch (e) {
      if (sent !== this.#sent) return
      this.#show(this.#accepted)
      this.error = { when: 'save', text: this.#failure(e) }
    }
  }

  /** The daemon's answer becomes what is shown, accepted and cached. */
  #keep(answer: Preferences) {
    this.#accepted = answer
    this.#show(answer)
    this.error = null
    writeCache(this.#storage, answer)
  }

  /** Shows `preferences` and paints their theme, without storing or caching anything. */
  #show(preferences: Preferences) {
    this.current = preferences
    applyTheme(this.#root, preferences.theme)
  }
}

/**
 * The page's one `PreferencesState`: the shell provides it, and anything that reads or changes a
 * preference, however deep, takes it from here.
 */
export const [usePreferences, providePreferences] = createContext<PreferencesState>()
