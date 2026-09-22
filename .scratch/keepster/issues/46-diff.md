# 46: Diff — computed by the daemon

**Blocked by:** 39 (Read a version back — the derived index, entries, content and the timeline)

**Status:** ready-for-agent

**What to build:** a user can see exactly what changed between two Snapshots, or between a Snapshot and the current
Working Tree, in the terminal and — later — in the browser, with every client seeing the same hunks.

The daemon computes the diff: it holds both blobs locally, tree-to-tree comparison is already the hot path, and
binary detection, rename detection and the "too large to diff" threshold are decisions clients must not each make
differently. Without a path the answer is the changed-path summary; with one it is hunks. "Versus the previous
Snapshot" is not a mode, because the client already has the previous id from the timeline.

- [ ] Diffing two Snapshots returns the changed-path summary, and adding a path returns hunks
- [ ] The comparison target may be the Working Tree as well as another Snapshot
- [ ] Both the structured and the unified output formats exist, so the CLI never reassembles unified text itself
- [ ] A binary file, and a file past the diff threshold, are reported as such instead of producing nonsense
- [ ] A rename is detected and reported as one
- [ ] `keepster diff` mirrors it, and writing its output to a file is possible without the shell re-encoding it
- [ ] Diffs are asserted through the harness against a real store
