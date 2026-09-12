# 15 — Web UI: timeline, view modes and restore presentation

Type: prototype
Status: resolved
Blocked by: 09

## Question

Does the interaction actually work: a Snapshot timeline on one side, and for a selected file the three modes
`Full file`, `Δ Previous`, `Δ Current` (the last being the default)?

Build a throwaway prototype against fixture data to answer:

- whether the timeline stays legible at hundreds of Snapshots, and whether filtering it to "Snapshots where this
  file changed" is enough navigation;
- how Restore and "open in editor" are presented so that a destructive action is never one stray click away;
- how a Snapshot that only deleted or added files is shown;
- where the line-wrapping toggle sits, since ticket 09 made wrapping opt-in and off by default.

Split out of [09 — Web UI diff editor: CodeMirror 6 or Monaco](09-web-ui-prototype.md), which settled the editor
and left these questions unprototyped. Reuse the `ui/` scaffold on the `prototype/web-ui` branch.

## Answer

**All three presentations ship, as three view modes of one History screen, defaulting to the rail.** The
prototype is the primary source: branch `prototype/web-ui`, commit `70c3c64`, second dev-server entry
`ui/timeline.html?variant=A|B|C` (`pnpm dev`, the page is deliberately not in `pnpm build`).

| | Navigation | What it answers |
| --- | --- | --- |
| **A** Left rail (default) | Snapshots in a permanent vertical column, files scoped to the selected one | "Something broke — when?" |
| **B** File first | A file tree of the Root, then a horizontal scrubber of the Snapshots that touched the chosen file | "This file — what happened to it?" |
| **C** Activity feed | Snapshot cards with their changed paths inline, the diff as an overlay | "What happened here while I was away?" |

### Modes of one screen, not three screens

The prototype carried the selected file *and* Snapshot across a `?variant=` switch, and that read as
continuity rather than as navigation — which is what a view mode is. Three separate screens would duplicate
the header, the Root picker and the diff pane, and would force every user to first answer "which of these
three screens is the right one", a question the product has no good answer to. A is the default because the
tool is opened after an accident: the user knows *when*, not *which file*, so the Snapshot-first question is
the common one.

Carried across a switch: the **file** and the **Snapshot**. Local to each mode: B's version cursor, A's
list-or-tree choice, C's scroll position. Carrying the file alone was tried first and is not enough — A
then opens on its newest Snapshot, whose file column cannot contain the file just chosen in B, which reads
as a reset. Where no Snapshot is carried (arriving from C, or a cold start), A resolves an incoming file to
the newest Snapshot that touched it.

### The four questions

- **Legibility at hundreds of Snapshots.** The rail needs sticky day headings and the per-file filter; with
  both, 400 rows are navigable. Filtering to "Snapshots where this file changed" **is** enough navigation
  for the file-history case — confirmed in use, and it is why B is a mode rather than the answer. One
  consequence found by using it: while that filter is on, the selected Snapshot can fall out of the list, so
  the selection moves to the newest Snapshot the filter still contains.
- **Restore and "open in editor".** Never one click, and **the presentation is shared by all three modes**:
  a mode decides navigation, not destructive affordances, and the same irreversible action wearing three
  different faces is indefensible. The shared form is A's: a preflight naming exactly what will be
  overwritten, stated as a dry run, with the Safety Snapshot promise on the same panel. This is the surface
  ticket 10's `dry_run`, per-reason confirmations and `skip_unresolvable` will populate.
- **A Snapshot that only deleted or only added files.** It must be stated in words, never inferred from an
  empty-looking diff. C's card does this best ("N files deleted, nothing edited"), and that wording folds
  into A's rail row, which currently says only "3 deleted" in red — too quiet for the tool's central
  scenario. C's "hide routine watcher snapshots" filter folds into the rail for the same reason it was
  mandatory in the feed.
- **The line-wrapping toggle** sits with the other per-file view controls in the diff pane's own bar, not in
  a settings screen: it is a property of looking at *this* file, and ticket 09 made it off by default.

### The file column

The flat list of the Snapshot's changes is the baseline; **the tree is a view option**, not a replacement.
The tree may widen to the whole Root, and selecting a file the current Snapshot never touched is legitimate
— the rail's per-file filter is what re-scopes time, and until it is switched on the file column says
"unchanged here" against the selected file rather than leaving a column that looks unrelated to the editor.

### Left for other tickets

The v1 SPA stack is still unconfirmed (the prototype runs Vite 8, Svelte 5 and TypeScript, picked for the
prototype); themes, diff contrast and the long-line threshold key belong to
[16 — Web UI themes and appearance settings](16-web-ui-themes.md).
