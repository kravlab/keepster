/**
 * PROTOTYPE (ticket 15) — the state that outlives a variant switch.
 *
 * The variants disagree about layout, not about what the user is looking at, so carrying the selection
 * across `?variant=` is what makes flipping between them a comparison rather than a restart.
 *
 * Both halves are carried, and the second one was learned the hard way: carrying only the path left variant
 * A opening on its newest Snapshot, whose file column then could not contain the file the user had just
 * chosen in B — which reads as "it reset", not as "it kept my file". B's version cursor *is* a Snapshot, of
 * exactly the kind A selects, so there was never a model to smuggle. What stays local is genuinely local:
 * B's index into a filtered list, A's list-or-tree view, C's scroll position.
 */
export const selection = $state<{ path: string | null; snapshotId: string | null }>({
  path: null,
  snapshotId: null,
})
