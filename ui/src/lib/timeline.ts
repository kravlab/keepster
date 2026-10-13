/** Keeping the rail's list of Snapshots current as new ones are taken. */

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
