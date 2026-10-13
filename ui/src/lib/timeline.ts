/** Keeping a view mode's list of Snapshots current as new ones are taken, and reading it. */

/**
 * The shown Snapshots with those of a freshly fetched first page that are newer put on top; both lists
 * newest first. `null` when the fresh page does not reach back to the newest shown Snapshot: what lies
 * between is unknown, and the list has to be loaded afresh.
 */
export function withNewer<S extends { id: string }>(shown: readonly S[], fresh: readonly S[]): S[] | null {
  const newest = shown[0]
  const at = newest ? fresh.findIndex((s) => s.id === newest.id) : -1
  return at === -1 ? null : [...fresh.slice(0, at), ...shown]
}

/**
 * The Snapshot before the one at `index` of an unfiltered list shown newest first, as far as the loaded
 * pages tell: the next row; `null` past the Root's first Snapshot, once no older page remains (`more` is
 * false); `undefined` when the list cannot tell, past its last loaded row or for an index it does not hold.
 */
export function loadedPredecessor(
  list: readonly { id: string }[],
  index: number,
  more: boolean,
): string | null | undefined {
  if (index < 0 || index >= list.length) return undefined
  const next = list[index + 1]
  if (next) return next.id
  return more ? undefined : null
}
