# 15 — Web UI: timeline, view modes and restore presentation

Type: prototype
Status: open
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
