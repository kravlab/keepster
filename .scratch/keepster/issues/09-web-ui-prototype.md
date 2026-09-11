# 09 — Web UI diff editor: CodeMirror 6 or Monaco

Type: prototype
Status: resolved
Blocked by: 07

## Question

Which diff editor backs the web UI's three modes `Full file`, `Δ Previous`, `Δ Current`: CodeMirror 6 MergeView
or Monaco DiffEditor, judged on real files for bundle size, syntax highlighting quality and behaviour on a large
file?

Build a throwaway prototype against fixture data, keep it on a `prototype/web-ui` branch and link it from this
ticket.

## Answer

**CodeMirror 6.** The prototype lives on the `prototype/web-ui` branch under `ui/` (Vite 8, Svelte 5, TypeScript,
pnpm): `pnpm dev` serves the bake-off page, and `pnpm measure` rebuilds each engine alone and writes
`ui/measure/REPORT.md`.

### Evidence

Bundle weight, one build per engine; "eager" is the static import closure of the entry, what the browser must
download before the editor paints:

| build | eager gzip | total gzip |
| --- | --- | --- |
| CodeMirror 6: MergeView, 13 lazily loaded languages | 96.7 KiB | 250.0 KiB |
| Monaco: package entry | 975.6 KiB | 3457.6 KiB |
| Monaco: `editor.api` plus the same 13 languages | 671.1 KiB | 1119.5 KiB |

Syntax highlighting is good in both, TOML and Markdown included (CodeMirror through a legacy stream mode, Monaco
through `ini`). Monaco's diff marking reads better, but CodeMirror's is a theme default rather than a limit:
`@codemirror/merge` gives a changed line an 8 % background and changed text a 2px underline
(`dist/index.js:1113-1130`), both overridable through `EditorView.theme`. On ordinary files CodeMirror scrolled
smoothly, while Monaco stuttered and applied highlighting later.

Large file (60,000 lines, ~4 MB, 200 scattered edits): MergeView's own diff, at its default `scanLimit: 500`,
gives up and emits a single 59,540-line chunk, and on every viewport move `buildChunkDeco` walks every line of each
chunk intersecting the viewport (`dist/index.js:777, 804-834`), so scrolling lagged. Fed precise hunks through
`diffConfig.override` (199 chunks, the largest 40 lines), it scrolled smoothly. Monaco scrolled smoothly too, but it
always computes its own diff in its worker; its `advanced-external` option is a bundled algorithm, not a hook for
external hunks.

Long line (875,438 characters on one line):

- The lag was Lezer. Parsing that line takes about 1.5 s per side, run on the main thread in idle-callback slices
  of up to 100 ms, up to 100,000 characters ahead of the viewport (`@codemirror/language/dist/index.js:606-618`).
  With no language attached, scrolling was smooth, wrapped or not.
- Unwrapped, a line wider than 2,000,000 px (`VP.MaxHorizGap`, roughly 256,000 characters) gets clamped horizontal
  gaps whose position correction does not survive a scrollbar jump, so the middle never renders
  (`@codemirror/view/dist/index.js:6524, 6553-6581`). Wrapped, the gaps are vertical and unclamped: the whole line
  renders, after a short delay when jumping.
- Monaco renders the first 10,000 characters (`stopRenderingLineAfter`), stops tokenizing at 20,000
  (`maxTokenizationLineLength`) and ends the line with a `Show more (N)` link; clicking past the cut-off sets
  `stopRenderingLineAfter: -1` for the whole editor (`contrib/longLinesHelper`). Its behaviour on this line after
  that click was not measured.

Binary files are not handed to either engine; the UI shows its own card.

### Decisions

- **Hunks always come from the daemon.** The UI hands the hunks of ticket 07's `GET /v1/roots/{r}/diff` to MergeView
  through `diffConfig.override` and never lets MergeView diff on its own. Monaco would have worked against this
  architecture: it has no public way to render hunks computed elsewhere.
- **Line wrapping is optional**: off by default, switched by a button as in the prototype.
- **A file with a line longer than a threshold opens without syntax highlighting.** The threshold is configurable
  with a default of 20,000 characters, Monaco's own default. The rule is per file, not per line, because Lezer parses
  the whole document and cannot skip one line. Where the key lives is part of
  [16 — Web UI themes and appearance settings](16-web-ui-themes.md).
- **Diff contrast is a theme concern**, carried by the same ticket 16.

## Comments

- 2026-09-10: split. This ticket originally also asked about timeline legibility, the presentation of Restore and
  "open in editor", and Snapshots that only add or delete files. Those were never prototyped here and moved to
  [15 — Web UI: timeline, view modes and restore presentation](15-web-ui-timeline-prototype.md).
