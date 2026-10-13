# 83: The diff pane never asks for a Snapshot with the previous selection's predecessor or file

**Blocked by:** none

**Status:** done

**What to build:** moving the selection to another Snapshot, in the rail or on the file-first mode's scrubber, makes
the diff pane ask the daemon only about that Snapshot, its own predecessor and a file it lists. Today, for one moment,
the pane asks about the new Snapshot together with the previous one's predecessor and file: the daemon answers 404
(a diff of a Snapshot against itself, `diff?from=X&to=X`, or the content of a file the Snapshot does not hold), and the
right diff follows straight after. Nothing wrong stays on screen, but every such step costs failed requests, and a
slower answer could show the wrong diff or an error first.

The cause is in `RailMode.svelte` and `FileMode.svelte` (ticket 65). The selected Snapshot (`toId`) is derived and
changes at once; its predecessor (`fromId`, `fromKnown`), its file list (`files`) and the file shown (`file`,
`untouchedFile`) are state that effects rewrite afterwards, and none of it records which Snapshot it was found for. So
for one update the pane receives a new `to` with a stale `from` and `file`, and its effect runs before the parent's
effects reset them: the bad request goes out before the request for the new Snapshot's list of changes. The activity
feed is not affected, since it hands the pane its Snapshot, predecessor and file as one object.

- [x] Every value the pane is given alongside the selected Snapshot (predecessor, file list, file) records the
      Snapshot it was found for, and is treated as unknown while that differs from the selection
- [x] The rail and the file-first mode give the pane a Snapshot only together with its own predecessor and file
- [x] Regression tests show that a loaded result names the Snapshot it belongs to and is never handed to the pane
      while another Snapshot is selected; a browser run through both modes sees no request that mixes two Snapshots
- [x] The comments in both modes say why the values are tagged with their Snapshot

## Comments

- Found during ticket 66 by a browser run that clicked through the rail and the scrubber; reproduced in both modes.
  The two components are unchanged by ticket 66, so the defect is on `main` since ticket 65.
