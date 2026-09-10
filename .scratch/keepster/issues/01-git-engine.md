# 01 — Which git implementation backs the history store

Type: research
Status: resolved

## Question

The history store is a bare git repository we own, written to with plumbing only. Which implementation should
`SnapshotStore` use on Linux, macOS and Windows: `gitoxide`, `git2` (libgit2 bindings), or invoking the `git`
binary?

Judge each candidate against the operations v1 actually needs:

- building a tree from an index we control, never touching any working repository's index;
- creating commits with an explicit parent list (including none) and custom trailers, without moving any branch;
- creating, listing and deleting refs under a custom namespace, including packed-refs behaviour at hundreds of refs;
- disabling reflogs so that deleting a ref makes its objects unreachable;
- garbage collection / pruning of unreachable objects;
- reading a blob at a path in a given commit, and diffing two commits with no common ancestor.

Also weigh: whether a `git` binary may be assumed present on a user's Windows machine, build and cross-compile
complexity (libgit2 needs a C toolchain), licensing, and maturity of each candidate for the operations above.

Deliver a recommendation plus a support matrix with evidence per operation, not impressions.

## Answer

Back the History Store with `git2` (libgit2 bindings), built with `default-features = false` and
`vendored-libgit2`, and assume no `git` binary is present at runtime. `git2` covers six of the seven required
operations natively and maturely: an owner-less in-memory index (`Index::new` + `Index::add` +
`write_tree_to`) that never opens any repository's index file, `Repository::commit(update_ref: None, …)` with an
arbitrary parent slice, full ref create/list/delete plus `refdb_compress` for packed-refs, git-identical
`core.logAllRefUpdates` semantics (off by default in a bare repository), blob-at-path reads, and tree-to-tree
diffs, which need no common ancestor. `gitoxide` is rejected for now because the crates we would lean on are
still pre-1.0 "initial development", packed-refs compaction is reachable only beneath its public API, and its
pack writer has no delta compression; the `git` binary is rejected because it is not present by default on
Windows and depending on it contradicts the single-artifact decision already made for the web UI.

The seventh operation, garbage collection, is unavailable in **both** libraries (libgit2 issue #3247 has been
open since 2015; gitoxide has no gc/prune/repack anywhere).

**Amended after review (user decision).** The research framed a runtime `git` binary as excluded by the
single-artifact decision; that decision was about embedding the web UI assets, and extending it to `git` was the
agent's inference, not a decision anyone made. Corrected: an external `git` is a **mandatory prerequisite** on all
three platforms, and the two engines split by role.

- **Hot path — `git2` in process**: writing a Snapshot from an index we own, reading blobs, tree-to-tree diffs.
  These run on every Trigger and must not pay process-spawn cost or parse text output.
- **Maintenance — the external `git` binary**: `gc`, `repack`, packed-refs compaction. Rare, batch, offline work,
  and precisely the part that is missing from both libraries and most dangerous to write by hand.

Consequently Keepster does **not** implement its own mark-and-sweep in v1: the risk identified above
(a wrong sweep destroys Snapshots, a timid one leaks disk, and it races the live daemon) is delegated to the tool
that has solved it. Nothing is bundled: no MinGit, no shipped git binaries, and therefore no GPLv2 redistribution
obligations. What this buys instead is a prerequisite to detect and validate — see
[11 — External git prerequisite](11-git-prerequisite.md).

Full support matrix, evidence and citations: [`../research/01-git-engine.md`](../research/01-git-engine.md)
