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
  headings and its list kept current (`rail.ts`, `timeline.ts`), the changed-file list (`files.ts`), the
  long-line threshold (`lines.ts`), and every user-facing text (`strings.ts`).
- `src/editor/`: the CodeMirror adapter and the curated grammars, loaded lazily.
- `src/history/`: the History screen.

## Budget

`pnpm measure` reads the Vite manifest of the real build. The eager part, what the page loads before
anything is shown (the entry's static imports and their CSS), stays within 150 KiB gzipped; everything,
grammars included, within 400 KiB. The editor and every grammar are lazy chunks.
