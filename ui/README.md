# Keepster web UI — THROWAWAY PROTOTYPE

Two throwaway passes share this scaffold, one page each. Nothing here is production code — no tests,
no error handling beyond what keeps the page running, no abstractions worth keeping.

- **Ticket 09** (`/`, resolved): which diff editor backs the Keepster web UI, CodeMirror 6 MergeView
  or Monaco DiffEditor?
- **Ticket 15** (`/timeline.html`): what should the history browser look like — the timeline, the
  three view modes, and how Restore is presented?

## Run

```sh
pnpm install
pnpm dev        # http://localhost:5177          — ticket 09, the engine bake-off
                # http://localhost:5177/timeline.html  — ticket 15, the UX variants
pnpm measure    # rebuilds each engine alone, writes measure/REPORT.md
```

`pnpm build` only builds the bake-off page: `timeline.html` is a dev-server-only entry, on purpose,
so that the ticket 09 measurements stay comparable with what they measured.

Note: `svelte-check` does not run in this scaffold. It needs TypeScript 6 alongside 7 and a `--tsgo`
flag; the project is on TypeScript 7 alone. Compilation is checked by loading the pages.

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

## Timeline variants (ticket 15)

`/timeline.html?variant=A|B|C`, switchable from the floating bar at the bottom (or ← / →). The three
disagree about structure, not colour:

| | Navigation | Restore lives | What it tests |
| --- | --- | --- | --- |
| **A** Left rail | Snapshots in a permanent vertical column; the Snapshot's changed files as a flat list, switchable to a tree that can widen to the whole Root | A `Restore…` button opening a dry-run dialog | Whether a rail stays legible at hundreds of Snapshots |
| **B** File first | A file tree of the whole Root carrying each file's version count, then a horizontal scrubber of the Snapshots that touched the chosen file, over a faint band of all of them | An action bar that only appears while an older version is selected | Whether "Snapshots where this file changed" is enough navigation |
| **C** Activity feed | One column of Snapshot cards with their changed paths inline; the diff is an overlay | Inside the overlay, behind a disclosure | How a Snapshot that only deleted or only added files reads |

`src/fixtures/timeline.ts` builds ~400 Snapshots across 30 days on ticket 06's Retention ladder, and
plants the cases that break naive layouts: a deletion-only Snapshot, an addition-only one, an
incomplete one, two Withheld paths with their reasons, a Safety Snapshot and two Pins.

Restore is a stub everywhere — the question is what it should look like, not whether it works. In all
three it is deliberately two steps and never adjacent to the controls used for browsing.

The selected **file and Snapshot** survive a variant switch (`selection.svelte.ts`), so flipping between
variants compares two layouts over the same thing rather than restarting. Carrying the file alone was not
enough: A is Snapshot-first, so it opened on its newest Snapshot, whose file column could not contain the
file just chosen in B. A therefore resolves an incoming file to the newest Snapshot that touched it when no
Snapshot is carried, and says "unchanged here" when the selected file is absent from the Snapshot's changes.
C publishes the pair when a path is opened but never consumes it — arriving in a feed should not auto-open
an overlay.
