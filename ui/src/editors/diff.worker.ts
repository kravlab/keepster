/**
 * PROTOTYPE (ticket 09) — stands in for the daemon's diff endpoint (ticket 07: the daemon computes diffs).
 *
 * Computes a precise diff (no `scanLimit`, no timeout) off the main thread and replies with plain ranges,
 * because `Change` instances do not survive structured cloning.
 */
import { presentableDiff } from '@codemirror/merge'

export interface HunkRange {
  fromA: number
  toA: number
  fromB: number
  toB: number
}

self.onmessage = (e: MessageEvent<{ before: string; after: string }>) => {
  const ranges: HunkRange[] = presentableDiff(e.data.before, e.data.after).map(({ fromA, toA, fromB, toB }) => ({
    fromA,
    toA,
    fromB,
    toB,
  }))
  self.postMessage(ranges)
}
