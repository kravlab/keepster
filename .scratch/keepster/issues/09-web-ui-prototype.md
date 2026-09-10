# 09 — Web UI: timeline and the three view modes

Type: prototype
Status: open
Blocked by: 07

## Question

Does the interaction actually work: a snapshot timeline on one side, and for a selected file the three modes
`Full file`, `Δ Previous`, `Δ Current` (the last being the default)?

Build a throwaway prototype against fixture data to answer:

- whether the timeline stays legible at hundreds of snapshots, and whether filtering it to "snapshots where this
  file changed" is enough navigation;
- CodeMirror 6 MergeView versus Monaco DiffEditor on real files: bundle size, syntax highlighting quality, and
  behaviour on a large file;
- how restore and "open in editor" are presented so that a destructive action is never one stray click away;
- how a snapshot that only deleted or added files is shown.

Keep the prototype on a `prototype/web-ui` branch and link it from this ticket.
