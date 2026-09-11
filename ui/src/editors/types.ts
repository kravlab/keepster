/** PROTOTYPE (wayfinder ticket 09) — shared shape both engine adapters implement. */
import type { Fixture } from '../fixtures'

export type Engine = 'cm6' | 'monaco'

/** The three modes ticket 09 asks about, minus `delta-current` (no Working Tree in a fixture). */
export type ViewMode = 'diff' | 'full'

/**
 * Where CodeMirror's hunks come from. `builtin` lets MergeView diff on the main thread with its default
 * `scanLimit: 500`, which on large inputs gives up and emits one document-sized chunk that is then
 * re-decorated on every viewport move. `daemon` stands in for ticket 07's contract: precise hunks computed
 * elsewhere (a worker here, the daemon in v1) and handed over through `diffConfig.override`.
 * Monaco ignores it: it has no public way to accept externally computed hunks.
 */
export type CmDiffSource = 'builtin' | 'daemon'

export interface MountRequest {
  el: HTMLElement
  fixture: Fixture
  mode: ViewMode
  cmdiff: CmDiffSource
  /**
   * CodeMirror only: wrap long lines. Unwrapped, a line wider than 2,000,000 px (`VP.MaxHorizGap`) gets
   * clamped horizontal gaps whose position correction breaks on scrollbar jumps, so the middle of the line
   * renders off-screen; wrapped, the gaps are vertical and never clamped. Monaco ignores it.
   */
  wrap: boolean
  /**
   * CodeMirror only: attach the language package. Off, no Lezer parse runs; on a pathological line that
   * parse costs seconds and is sliced into ~100 ms idle-callback chunks on the main thread while scrolling.
   */
  lang: boolean
}

export interface Mounted {
  destroy: () => void
  /** ms from the adapter being called to the browser painting the first frame of the editor */
  paintMs: number
  /** ms spent inside the engine's own construction call (diff computation included, where synchronous) */
  constructMs: number
  /** ms spent obtaining externally computed hunks before construction (CodeMirror `daemon` source only) */
  hunksMs?: number
  /** the language actually applied, or 'plaintext' when nothing matched */
  language: string
}

export type MountFn = (req: MountRequest) => Promise<Mounted>

/** Resolves once the browser has painted whatever the engine just built. */
export function nextPaint(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())))
}
