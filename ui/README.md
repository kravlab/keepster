# Keepster web UI — THROWAWAY PROTOTYPE

Wayfinder ticket 09, first pass: **which diff editor backs the Keepster web UI, CodeMirror 6
MergeView or Monaco DiffEditor?** Nothing here is production code — no tests, no error handling
beyond what keeps the page running, no abstractions worth keeping. The UX variants of the timeline
(the other half of ticket 09) are a separate pass.

## Run

```sh
pnpm install
pnpm dev        # http://localhost:5177
pnpm measure    # rebuilds each engine alone, writes measure/REPORT.md
```

URL state: `?file=<fixture id>&mode=diff|full&engine=cm6|monaco|both&cmdiff=builtin|daemon&wrap=on|off&lang=on|off`.

`cmdiff` only affects CodeMirror. `builtin` lets MergeView diff on the main thread with its default
`scanLimit: 500`, which on a large file gives up and emits a single document-sized chunk; MergeView
re-walks every chunk intersecting the viewport on each viewport move, so that one chunk is re-decorated
while scrolling. `daemon` models ticket 07 (the daemon computes diffs): precise hunks are computed in
`src/editors/diff.worker.ts` off the main thread and handed to MergeView through `diffConfig.override`.

`wrap` also only affects CodeMirror (`EditorView.lineWrapping`). Unwrapped, a line wider than
2,000,000 px (`VP.MaxHorizGap`, roughly 256k characters) is laid out with clamped horizontal gaps whose
position correction does not survive a scrollbar jump, so the middle of `bundle.min.js` renders off-screen.
Wrapped, the gaps are vertical and unclamped. Monaco is left as-is: it renders only the first 10,000
characters of a line (`stopRenderingLineAfter`) and stops highlighting at 20,000 (`maxTokenizationLineLength`).

`lang` also only affects CodeMirror: off, no language package is attached, so no Lezer parse runs. On
`bundle.min.js` that parse costs ~1.5 s per side in node, and CodeMirror runs it in idle-callback slices of
up to 100 ms on the main thread, parsing up to 100,000 characters past the viewport as you scroll.

## What is being compared

| | CodeMirror 6 | Monaco |
| --- | --- | --- |
| diff | `@codemirror/merge` `MergeView`, computed on the main thread | `createDiffEditor`, computed in a web worker |
| languages | one lazily imported package per language (13 in `src/editors/languages.ts`) | every Monarch grammar ships with the package entry; `measure/monaco-core.ts` shows the curated alternative |
| setup | deliberately *not* `basicSetup` — a history viewer needs no autocomplete, search panel or edit keymap | `readOnly`, `hideUnchangedRegions` |

## Fixtures

Deterministic (fixed seed, never `Math.random`), materialised on demand. The two heavy ones are
mounted only on an explicit click, because a 4 MB diff mounted on hover is how a tab dies.

See `src/fixtures/index.ts` for what each one is meant to expose.
