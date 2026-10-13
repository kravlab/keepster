# Keepster web UI

The single-page app the daemon embeds and serves on its loopback listener. `keepster ui` opens it in the
browser, already signed in. Svelte 5, Vite, TypeScript, and CodeMirror 6 for the diff.

The built app in `dist/` is committed: `cargo build` embeds it and never needs node. Rebuild it after
any change here and commit the result; CI fails when `dist/` is stale.

## Commands

Node 22.12 or newer and pnpm. From the repository root:

```sh
make ui              # pnpm install, then build dist/
make check-ui        # svelte-check, Prettier, ESLint, Vitest
make check-ui-size   # build, then hold the bundle to its budget
```

Or here, with `pnpm install` first: `pnpm build`, `pnpm check`, `pnpm format`, `pnpm lint`, `pnpm test`,
`pnpm measure`.

## Layout

- `src/lib/`: everything that is not drawing, with unit tests beside it. The `/v1` client and its narrow
  types (`api.ts`, `wire.ts`), what a failure says on screen (`failures.ts`), daemon hunks to editor ranges
  (`hunks.ts`, and its failure in `mismatch.ts`), the live event stream (`events.ts`), the rail's day
  headings (`rail.ts`), a view mode's pages of the timeline kept current (`pages.svelte.ts`, `timeline.ts`),
  Root-relative paths (`paths.ts`), the changed-file list (`files.ts`), a
  Snapshot's changes in words and what counts as routine (`summary.ts`), the selection carried across view
  modes (`selection.ts`), the lazy file tree (`tree.ts`), Snapshot ids as moments (`ids.ts`), Restore's
  questions and request (`restore.ts`), the long-line threshold (`lines.ts`), the preferences and their
  paint cache (`preferences.ts`, and the page's copy kept with the daemon in `preferences.svelte.ts`),
  newline-delimited JSON off a stream (`ndjson.ts`), search results and a content search's stream folded
  (`search.ts`, and one search run at a time in `searching.svelte.ts`), and every user-facing text
  (`strings.ts`).
- `src/editor/`: the CodeMirror adapter, its colors read from the theme's variables (`palette.ts`), and the
  curated grammars, loaded lazily.
- `src/search/`: the search overlay the header's field opens (`SearchOverlay.svelte`).
- `src/history/`: the History screen. `History.svelte` holds the three view modes, which share a selection
  the shell (`App.svelte`) keeps so that a search result can set it: the rail (`RailMode.svelte`), file first (`FileMode.svelte`) and the activity feed
  (`FeedMode.svelte`). The diff pane
  (`DiffPane.svelte`) is common to all three, and so is the Restore dialog it opens
  (`RestoreDialog.svelte`); `FileTree.svelte` is the tree the rail and file first both show.

## Budget

`pnpm measure` reads the Vite manifest of the real build. The eager part, what the page loads before
anything is shown (the entry's static imports and their CSS), stays within 150 KiB gzipped; everything,
grammars included, within 400 KiB. The editor and every grammar are lazy chunks.
