/**
 * What a Snapshot changed, as a shape the page can say in words, and whether it is routine.
 *
 * A Snapshot that only deleted or only added files must say so, never leave it to an empty-looking diff
 * (ticket 15): deleting files is the accident the tool exists for. The words are in `strings.shape`.
 */
import type { Snapshot } from './wire'

/** What a Snapshot changed against the one before it. */
export type ChangeShape =
  | { kind: 'uncounted' }
  | { kind: 'none' }
  | { kind: 'onlyDeleted'; count: number }
  | { kind: 'onlyAdded'; count: number }
  | { kind: 'mixed'; added: number; modified: number; deleted: number }
  /** The total is known, the counts by kind are not. */
  | { kind: 'changed'; count: number }

/** The shape of a Snapshot's changes, from the timeline's counts. */
export function changeShape(snapshot: Snapshot): ChangeShape {
  const { changed_paths: changed, added_paths: added, deleted_paths: deleted } = snapshot
  if (changed === undefined) return { kind: 'uncounted' }
  if (changed === 0) return { kind: 'none' }
  if (added === undefined || deleted === undefined) return { kind: 'changed', count: changed }
  if (deleted === changed) return { kind: 'onlyDeleted', count: changed }
  if (added === changed) return { kind: 'onlyAdded', count: changed }
  return { kind: 'mixed', added, modified: changed - added - deleted, deleted }
}

/**
 * Whether the "hide routine Snapshots" filter hides it: the watcher's ordinary churn. Anything a person
 * would call an event stays: another Trigger (an unknown one too), a Pin, an incomplete Snapshot, and any
 * deletion — including one not counted yet, since a Snapshot that cannot be judged is not hidden.
 */
export function isRoutine(snapshot: Snapshot): boolean {
  return snapshot.trigger === 'watcher' && !snapshot.pinned && !snapshot.incomplete && snapshot.deleted_paths === 0
}
