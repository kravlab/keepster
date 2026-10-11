# 46: Diff — computed by the daemon

**Blocked by:** 39 (Read a version back — the derived index, entries, content and the timeline)

**Status:** done

**What to build:** a user can see exactly what changed between two Snapshots, or between a Snapshot and the current
Working Tree, in the terminal and — later — in the browser, with every client seeing the same hunks.

The daemon computes the diff: it holds both blobs locally, tree-to-tree comparison is already the hot path, and
binary detection, rename detection and the "too large to diff" threshold are decisions clients must not each make
differently. Without a path the answer is the changed-path summary; with one it is hunks. "Versus the previous
Snapshot" is not a mode, because the client already has the previous id from the timeline.

- [x] Diffing two Snapshots returns the changed-path summary, and adding a path returns hunks
- [x] The comparison target may be the Working Tree as well as another Snapshot
- [x] Both the structured and the unified output formats exist, so the CLI never reassembles unified text itself
- [x] A binary file, and a file past the diff threshold, are reported as such instead of producing nonsense
- [x] A rename is detected and reported as one
- [x] `keepster diff` mirrors it, and writing its output to a file is possible without the shell re-encoding it
- [x] Diffs are asserted through the harness against a real store

## Comments

- **Decisions taken with the user before implementation.** `keepster diff [PATH] [--from SEL] [--to SEL|working]`,
  `from` defaulting to the latest Snapshot and `to` to the Working Tree, selectors as `cat -s` takes them;
  without a path a summary of changed paths, with one its unified hunks; `--json` the daemon's answer, `-o FILE`
  the exact bytes. The too-large threshold is a fixed 8 MiB (`keepster_store::DIFF_MAX_SIZE`), either side. Test
  seams: `/v1/roots/{r}/diff` through the harness, `keepster diff` as a process, `HistoryStore::diff` in the
  store, and the OpenAPI document.
- **The endpoint** is ticket 07's: `GET /v1/roots/{id}/diff?from=&to=<SnapshotId|working>&path=&format=json|unified`.
  `format=unified` without a path is the whole patch — the "export a diff between two Snapshots" of ticket 19 —
  and `keepster diff --patch` prints it. New codes: `comparison_invalid` (exit 2) and `format_invalid` (exit 1,
  the CLI builds the format itself).
- **The Working Tree side** is what the next Snapshot would hold: built by the very code a capture uses
  (`write_working_tree`, factored out of `pass`), under the Root's Include Rules and size cap, with the stat
  filter vouching and an unreadable file keeping its last content. Its blobs and trees are written into the
  store under the store's lock and never committed; maintenance prunes them, and the next capture finds them
  written. The evidence of the last pass is read, never updated. A file the next Snapshot would withhold still
  reads as deleted — it has no tree entry (ADR 0009) — but says `withheld: too_large`; files that could not be
  read are listed in `omitted`. Between two Snapshots, Withheld paths are ticket 48's records to report.
- **Renames** are git's defaults (half similar, a thousand candidates). Measuring similarity reads each
  candidate whole whatever its size, so when an added or deleted file is past the threshold, renames are found
  by identical content alone. **A mode change alone** carries `old_mode` and `new_mode`.
- **Costs left as they are**: the summary probes every changed blob for binariness, which inflates a packed blob
  whole — at most 8 MiB per file, one at a time; a whole unified patch is as large as its files' patches
  together, each bounded by the threshold. JSON lines are text with replacement characters where the bytes are
  not UTF-8; `unified` is byte-exact.
- **Not here**: `Content-Disposition: attachment` for a browser download of the patch belongs with the
  browser's download path, ticket 58.
