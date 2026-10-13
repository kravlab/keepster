/**
 * The selection the History screen keeps: a file and a Snapshot, carried across view modes (ticket 15).
 */

/**
 * The Snapshot to keep selected in a list shown newest first. One that a filter left out of the list gives
 * way to the newest one the list still contains, rather than to nothing.
 */
export function keptSelection(list: readonly { id: string }[], selected: string | null): string | null {
  if (selected !== null && list.some((s) => s.id === selected)) return selected
  return list[0]?.id ?? null
}

/** The History screen's view modes, in the order its tabs show them; the first is the default. */
export const MODES = ['rail', 'files', 'feed'] as const
/** One of [`MODES`]. */
export type Mode = (typeof MODES)[number]

/** What the History screen carries across a switch of view modes: a file and a Snapshot, either unset. */
export interface Selection {
  path: string | null
  snapshotId: string | null
}

/**
 * The Snapshot a view mode opens on. Carrying the file alone is not enough (ticket 15): the rail would open
 * on its newest Snapshot, whose files need not include the one just chosen, which reads as a reset. So a
 * file that arrives without a Snapshot resolves to the newest Snapshot that changed it.
 */
export async function arrivingSnapshot(
  selection: Selection,
  newestTouching: (path: string) => Promise<string | null>,
): Promise<string | null> {
  if (selection.snapshotId !== null) return selection.snapshotId
  return selection.path === null ? null : newestTouching(selection.path)
}

/**
 * The version of a file that the selected Snapshot holds, from the file's versions shown newest first: the
 * Snapshot itself when it changed the file, else the newest version taken before it; the newest version
 * when no Snapshot is selected, and the oldest for a Snapshot taken before the file existed. A Snapshot
 * carried into file first need not be one of its versions (ticket 15): it is shown as the version in
 * effect then, and stays the screen's selection until the user picks another version.
 */
export function versionAt(versions: readonly { id: string }[], selected: string | null): string | null {
  if (selected === null) return versions[0]?.id ?? null
  return (versions.find((v) => v.id <= selected) ?? versions.at(-1))?.id ?? null
}
