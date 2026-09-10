# Research 01 — Which git implementation backs the History Store

Answers `.scratch/keepster/issues/01-git-engine.md`.
Date: 2026-09-08. Platforms in scope: Linux, macOS, Windows.

Sources pinned at the revisions inspected:

- gitoxide `0cd9319555ff2b30d8eca95eb3befba018b15de5` (2026-09-09), crate `gix 0.87.1`
- git2-rs `f42a01267a3042b26d30e9d8acf286c6c739bd8a` (2026-09-02), crate `git2 0.21.0` over `libgit2-sys 0.18.7+1.9.6`
- libgit2 `0551dfd4ad989b6a3d5683c0d4cf326c6efef929` (2026-08-15)
- git 2.55.0 (the binary used for the local experiments below)

---

## Verdict

**Use `git2` (libgit2 bindings) in-process, and do not require a `git` binary at runtime.**
Garbage collection is the one operation no library provides; Keepster must implement it itself as a
mark-and-sweep over its own refs, for which `git2` exposes every primitive needed.

Ranking on the ticket's operation list: `git2` covers six of seven natively and maturely; `gix` covers the same
six but with a younger implementation and one operation done differently (tree editor instead of an index);
the `git` binary covers seven of seven but cannot be assumed present on Windows.

---

## Support matrix

Legend: **yes** = a documented API exists and was located in the implementation's own source;
**partial** = achievable but not through the obvious API, or only via a lower layer; **no** = no such API.

| # | Required operation | `gix` 0.87.1 | `git2` 0.21 / libgit2 1.9.6 | `git` binary |
|---|---|---|---|---|
| 1 | Build a tree from an index we control, never touching a working repository's index | **partial** — no index→tree writer; equivalent is `Repository::edit_tree` (`gix_object::tree::Editor`), which builds trees by path with no index at all | **yes** — `Index::new()` (owner-less, in memory) + `Index::add` + `Index::write_tree_to(&repo)` | **yes** — `GIT_INDEX_FILE` + `update-index --index-info` + `write-tree` |
| 2 | Commit with an explicit parent list (including none) and custom trailers, without moving any branch | **yes** — `Repository::new_commit_as` / `write_object` (`commit` moves a ref; `new_commit*` and `write_object` do not) | **yes** — `Repository::commit(update_ref: None, …)` and `commit_create_buffer` | **yes** — `commit-tree` (never touches a ref) |
| 3 | Create / list / delete refs under a custom namespace, incl. packed-refs at hundreds of refs | **yes** — `edit_references` (`RefEdit`/`Change::Delete`), `references().prefixed(…)`; packed-refs deletion transparent; write-through to `packed-refs` only via `repo.refs` (`gix_ref::file::Store::transaction().packed_refs(…)`) | **yes** — `Repository::reference` / `reference_matching`, `references_glob`, `Reference::delete`, `Repository::refdb_compress` (= `pack-refs`) | **yes** — `update-ref`, `for-each-ref`, `update-ref -d`, `pack-refs` |
| 4 | Disable reflogs so deleting a ref makes objects unreachable | **yes** — `WriteReflog::Disable`; honours `core.logAllRefUpdates`; defaults to `Disable` for a bare repo | **yes** — same semantics as git; defaults off when bare; never logs refs outside `heads/`, `remotes/`, `notes/`, `HEAD` | **yes** — `core.logAllRefUpdates=false`; already false by default in a bare repo |
| 5 | Garbage collection / pruning of unreachable objects | **no** | **no** — but every mark-and-sweep primitive exists (`Odb::foreach`, `Revwalk::push_glob`, `PackBuilder::insert_walk`/`write`) | **yes** — `gc --prune=now` |
| 6 | Read a blob at a path in a given commit | **yes** — `Tree::lookup_entry_by_path` / `peel_to_entry_by_path` | **yes** — `Tree::get_path` + `TreeEntry::to_object` | **yes** — `cat-file blob <commit>:<path>` |
| 7 | Diff two commits with no common ancestor | **yes** — `Repository::diff_tree_to_tree` (feature `blob-diff`); unified output via `gix_diff::blob::UnifiedDiff` | **yes** — `Repository::diff_tree_to_tree` | **yes** — `diff-tree -r` (verified: no ancestry required) |
| — | Present on every user machine without our help | **yes** (linked in) | **yes** (linked in) | **no** on Windows; not guaranteed on Linux |
| — | Build / cross-compile cost | pure Rust, no C toolchain | C toolchain required (`cc`) unless a system libgit2 is used | none at build time; a runtime dependency instead |
| — | Licence of the engine | MIT OR Apache-2.0 | GPLv2 **with linking exception** (bindings MIT/Apache-2.0) | GPLv2 — separate process, so no linking question |
| — | Maturity for these operations | pre-1.0; `gix` and `gix-object` classed "initial development"; `gix-ref` a "stabilization candidate" | the mainstream embedded git; used by Cargo | reference implementation |

---

## Evidence, per row

### 1. Tree from an index we control

**`git2` — yes, and genuinely detached from any working index.**
`git_index_new()` creates an index with no owning repository
([`index.c:495`](https://github.com/libgit2/libgit2/blob/0551dfd4ad989b6a3d5683c0d4cf326c6efef929/src/libgit2/index.c#L495)),
`git_index_add` accepts entries on such an index — it asserts only on the entry mode, not on ownership
([`index.c:1721`](https://github.com/libgit2/libgit2/blob/0551dfd4ad989b6a3d5683c0d4cf326c6efef929/src/libgit2/index.c#L1721)) —
and `git_index_write_tree_to(oid, index, repo)` writes the tree into an explicitly named repository
([`index.c:884`](https://github.com/libgit2/libgit2/blob/0551dfd4ad989b6a3d5683c0d4cf326c6efef929/src/libgit2/index.c#L884)).
In Rust: `Index::new`, `Index::add`, `Index::write_tree_to`
([`index.rs:101,196,674`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/src/index.rs#L101)).

One trap worth recording: `git_index_add_from_buffer` — the convenient "add a path with this content" call —
*does* require an owning repository and errors with "Index is not backed up by an existing repository"
([`index.c:1529`](https://github.com/libgit2/libgit2/blob/0551dfd4ad989b6a3d5683c0d4cf326c6efef929/src/libgit2/index.c#L1529)).
The way round it is to write the blob first with `Repository::blob`/`blob_path`
([`repo.rs:1157,1177`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/src/repo.rs#L1157))
and put the resulting `Oid` into the entry. That keeps the whole Snapshot in an in-memory index owned by nobody,
so no repository's `index` file — ours or the Project's — is ever opened.

**`gix` — no index→tree writer; a different and arguably better mechanism.**
`gix-index` contains a decoder and an index-file writer only (`gix-index/src/write.rs` writes the *index file*),
and `gix::Repository` has `index_from_tree` (tree → index) but nothing in the other direction
([`gix/src/repository/index.rs:206`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix/src/repository/index.rs#L206)).
What gitoxide offers instead is `Repository::edit_tree` → `gix_object::tree::Editor`, with `upsert(rela_path, kind, id)`,
`remove`, and `write`, explicitly documented as usable from the empty tree "to build a tree from scratch"
([`gix/src/repository/object.rs:19`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix/src/repository/object.rs#L19),
[`gix-object/src/tree/editor.rs:48,83,128`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix-object/src/tree/editor.rs#L48)).
`crate-status.md` lists "edit trees efficiently and write changes back" as done.
For our purposes an index is only a means to a tree, so this satisfies the requirement — but it is a different
data structure, and the ticket's wording should be read as "produce a tree without touching a working index",
which all three do.

**`git` — yes, verified locally.** With `GIT_DIR` pointed at a bare store and `GIT_INDEX_FILE` at our own file,
`update-index --add --cacheinfo` + `write-tree` produced a tree containing `a.txt` (100644) and `dir/b.sh` (100755)
with no worktree in existence. `GIT_INDEX_FILE` is documented as "an alternate index file" (git(1), ENVIRONMENT).

### 2. Commits with explicit parents and trailers, without moving a branch

**`git2`:** `Repository::commit(update_ref: Option<&str>, author, committer, message, tree, parents: &[&Commit])`
— passing `None` writes the commit and updates nothing
([`repo.rs:1340`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/src/repo.rs#L1340)).
`parents` is a slice, so an empty slice gives a parentless commit — exactly what a new Segment needs.

**`gix`:** `Repository::commit` *does* move a ref and enforces that the first parent is the ref's current tip
([`object.rs:448`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix/src/repository/object.rs#L448)),
so it is the wrong call for us. `new_commit`/`new_commit_as` write the commit and "do not, however, update any
references" ([`object.rs:470,486`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix/src/repository/object.rs#L470)),
and `write_object` takes a fully-specified `gix_object::Commit` including `parents` and `extra_headers`
([`gix-object/src/lib.rs:169`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix-object/src/lib.rs#L169)).

**Trailers are not a git object feature.** They are text in the last paragraph of the commit message
(git-interpret-trailers(1)), so *any* engine that lets us set the message supports writing them; only *parsing*
differs. Verified locally: a commit written by `commit-tree` with a body of `Snapshot-Id: 01J\nProject-Id: p1`
is read back by `git show -s --format='%(trailers:key=Snapshot-Id,valueonly)'` as `01J`.
`gix` can parse them via `CommitRef::message_trailers`
([`gix-object/src/commit/message/mod.rs:21`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix-object/src/commit/message/mod.rs#L21));
note `crate-status.md` still lists "parse trailers" unchecked under gix-object, so the two disagree — the code is there.
`git2` has no trailer parser; we would split the message ourselves, which is a handful of lines.

Since `SnapshotId` is deliberately not the commit hash (CONTEXT.md), the trailer is how a commit carries its
`SnapshotId`, so parsing it back matters. This is not a differentiator: it is trivially ours to own either way.

### 3. Refs in a custom namespace, and packed-refs at scale

**`git2`:** `Repository::reference(name, id, force, log_message)` creates
([`repo.rs:1540`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/src/repo.rs#L1540)),
`reference_matching` gives compare-and-swap semantics (`GIT_EMODIFIED` if the ref moved under us)
([`repo.rs:1593`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/src/repo.rs#L1593)),
`references_glob` lists ([`repo.rs:931`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/src/repo.rs#L931)),
`Reference::delete` deletes, and `refdb_compress` is the `pack-refs` equivalent: "for on-disk reference databases …
this may pack all loose references"
([`repo.rs:1243`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/src/repo.rs#L1243)).

**`gix`:** ref changes go through a transaction — `edit_reference`/`edit_references`/`edit_references_as`
([`gix/src/repository/reference.rs:121`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix/src/repository/reference.rs#L121)) —
built from `RefEdit` with `Change::Update`/`Change::Delete` and a `PreviousValue` precondition
([`gix-ref/src/transaction/mod.rs`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix-ref/src/transaction/mod.rs)).
Listing by prefix is `references().prefixed(…)`
([`gix/src/reference/iter.rs:47`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix/src/reference/iter.rs#L47)).
`crate-status.md` marks as done: "transparent handling of packed-refs during deletion" and "writing loose refs into
packed-refs and optionally delete them" — the latter being a `pack-refs --prune`
([`crate-status.md`, gix-ref](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/crate-status.md)).

The caveat: `Repository::edit_references` always uses the *default* packed-refs policy, `PackedRefs::DeletionsOnly`
([`reference.rs:142`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix/src/repository/reference.rs#L142),
[`gix-ref/src/store/file/transaction/mod.rs:13`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix-ref/src/store/file/transaction/mod.rs#L13)).
To compact loose refs we would have to go under the ergonomic API to `repo.refs` — which is a public field
([`gix/src/types.rs:161`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix/src/types.rs#L161)) —
and drive `gix_ref::file::Store::transaction().packed_refs(PackedRefs::DeletionsAndNonSymbolicUpdatesAndDeletionsOfLooseRefs(…))`.
Possible, but a layering violation we would carry forever.

**`git` — measured at scale.** In a bare store with 502 refs under `refs/keepster/`: 502 loose ref files
before `pack-refs --all`, 0 loose files and a 503-line `packed-refs` after; `for-each-ref refs/keepster/`
returned all 502 either way; `update-ref -d` then removed refs that existed only inside `packed-refs`.
Hundreds of refs is a non-event for the on-disk format; the only real question is which library drives it.

### 4. Disabling reflogs

The git rule, verbatim from git-config(1) on `core.logAllRefUpdates`: a missing log file is auto-created only for
`refs/heads/`, `refs/remotes/`, `refs/notes/` and `HEAD` when true, for any ref under `refs/` when `always`, and
"This value is true by default in a repository that has a working directory associated with it, and **false by
default in a bare repository**". The History Store is bare, and our refs live under a custom namespace, so two
independent rules already keep reflogs out. Setting `core.logAllRefUpdates=false` explicitly makes it intentional
rather than incidental.

- **git (verified):** after `git config core.logAllRefUpdates false` and ~500 ref updates and deletions, `$GIT_DIR/logs`
  was never created. Deleting `refs/keepster/p1/seg-0001` and running `gc --prune=now` removed its commit
  (`git cat-file -e` → absent), while a commit still referenced by another ref survived.
- **git2/libgit2 (source):** `git_refdb_should_write_reflog` reads `core.logallrefupdates`, "Defaults to the opposite
  of the repo being bare", and even when true only logs `heads/`, `remotes/`, `notes/`, `HEAD` or refs that already
  have a log ([`refdb.c:325`](https://github.com/libgit2/libgit2/blob/0551dfd4ad989b6a3d5683c0d4cf326c6efef929/src/libgit2/refdb.c#L325)).
  `git_repository_init` writes `core.logallrefupdates=true` only when the repo is **not** bare
  ([`repository.c:2402`](https://github.com/libgit2/libgit2/blob/0551dfd4ad989b6a3d5683c0d4cf326c6efef929/src/libgit2/repository.c#L2402)).
  git2 also exposes `reflog_delete` and `reference_has_log` should we ever need to clean up
  ([`repo.rs:2747,2768`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/src/repo.rs#L2747)).
- **gix (source):** `WriteReflog::{Always, Normal, Disable}`
  ([`gix-ref/src/lib.rs:76`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix-ref/src/lib.rs#L76)),
  wired from `core.logAllRefUpdates`
  ([`gix/src/config/tree/sections/core.rs:55`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix/src/config/tree/sections/core.rs#L55)),
  defaulting to `Disable` when there is no worktree
  ([`gix/src/config/cache/util.rs:88-95`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix/src/config/cache/util.rs#L88)),
  and per-edit control through `LogChange`. `crate-status.md` marks "reflog changes can be entirely disabled
  (i.e. for bare repos)" as done. Reflog **expire** is listed as not done — irrelevant if we never write one.

This row is a tie, and it is the row the whole segmented-chain design depends on (ADR 0001).

### 5. Garbage collection — the deciding row

**Neither library has it.**

- libgit2: [issue #3247 "APIs for git-gc and git-reflog"](https://github.com/libgit2/libgit2/issues/3247) has been
  **open since 23 June 2015**. There is no `gc.h` or `repack.h` among libgit2's public headers
  (`include/git2/` contains `pack.h`, `odb.h`, `refdb.h`, but nothing for gc, prune or repack).
- gitoxide: grepping the pinned tree for `gc`, `prune`, `repack` and `garbage` across `gix/src` and `crate-status.md`
  turns up only unrelated hits (`remote.prune`, promisor notes). A GitHub issue search over the repository returns
  only [#15 "Smart-GC — build a big pack from smaller ones"](https://github.com/GitoxideLabs/gitoxide/issues/15), closed.

**The `git` binary has it, and it does exactly what ADR 0001 assumes.** Verified: with reflogs off, deleting the
Segment ref and running `gc --prune=now` dropped the unreachable commit and repacked the rest (`count-objects -v`
went from `count: 8, in-pack: 0` to `count: 0, in-pack: 7`). Note the default is *not* immediate: `--prune=<date>`
"Prune loose objects older than date (default is 2 weeks ago, overridable by … `gc.pruneExpire`)", and git-gc(1)
warns that `--prune=now` "increases the risk of corruption if another process is writing to the repository
concurrently" — with a live daemon writing Snapshots, that warning is aimed straight at us.

**What we can build ourselves.** Mark-and-sweep is not exotic, and `git2` exposes every piece:
`Revwalk::push_glob`/`push_ref` to seed from our namespace
([`revwalk.rs:116,140`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/src/revwalk.rs#L116)),
`Odb::foreach` to enumerate every object present
([`odb.rs:124`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/src/odb.rs#L124)),
and `PackBuilder::insert_walk`/`insert_recursive`/`write(path, mode)` to rewrite the reachable set into a fresh pack
([`packbuilder.rs:63,72,92`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/src/packbuilder.rs#L63)).
gitoxide has counterparts — `gix_odb` object iteration
([`gix-odb/src/store_impls/dynamic/iter.rs:225`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix-odb/src/store_impls/dynamic/iter.rs#L225)),
`gix-traverse`, and pack encoding in `gix-pack` — but `crate-status.md` still lists pack **delta compression** as not
done, so a gitoxide-written pack would be materially larger than git's.

Since a v1 History Store that never reclaims disk is not shippable and Retention is a first-class concept
(CONTEXT.md), this row decides the answer as much by what it forces us to own as by what it rules out.

### 6. Blob at a path in a commit

`git2`: `Tree::get_path(&Path)` then `TreeEntry::to_object(&repo)` → `Blob`. `gix`: `Tree::lookup_entry_by_path`
and `peel_to_entry_by_path`
([`gix/src/object/tree/mod.rs:174,192`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix/src/object/tree/mod.rs#L174)).
`git`: verified, `cat-file blob <commit>:dir/b.sh` printed the content and `rev-parse <commit>:a.txt` the blob id.
No differentiator.

### 7. Diffing two commits with no common ancestor

This row is a non-issue in every candidate, because a *tree* diff has no notion of ancestry — only merge-base
computation does, and we never need one. Verified locally: `merge-base` between two parentless commits exits 1
("no common ancestor"), while `diff-tree -r --name-status` between the same two commits succeeds.

`git2`: `Repository::diff_tree_to_tree(old, new, opts)`
([`repo.rs:2877`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/src/repo.rs#L2877)),
plus libgit2's `patch.h` for rendering.
`gix`: `Repository::diff_tree_to_tree` behind the `blob-diff` feature
([`gix/src/repository/diff.rs`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix/src/repository/diff.rs)),
with `gix_diff::blob::UnifiedDiff` for the rendered hunks
([`gix-diff/src/blob/mod.rs:9`](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/gix-diff/src/blob/mod.rs#L9)).
`crate-status.md` marks gix-diff "tree: changes needed to obtain other tree" as done and `imara-diff`-backed line
diffs as done, while "patches: text/binary/`git-apply` compatibility" are not — for a web UI that renders its own
hunks that is sufficient, but it does mean gitoxide will not hand us a `git diff`-shaped patch.

---

## Cross-cutting factors

### Can a `git` binary be assumed present?

**Windows: no.** Git is not a Windows component; git-scm.com's Windows install page offers a standalone installer,
a portable build and `winget install --id Git.Git -e --source winget` — every route is a deliberate user action
(<https://git-scm.com/install/windows>). Keepster's own target audience makes this less bad than it sounds
(a developer machine usually has Git), but "usually" is the wrong foundation for a tool whose entire purpose is
being there when something else has already gone wrong.

**macOS: only after a prompt.** Apple ships Git inside the Xcode Command Line Tools, installed with
`xcode-select --install` (<https://git-scm.com/install/mac>) — present on most developer machines, absent on a fresh one.

**Linux: usually, but not guaranteed** — a minimal container or a non-developer desktop may not have it.

This collides directly with a decision already on the map: the web UI is embedded in the binary to give
"one artifact, no runtime dependencies". Requiring `git` at runtime contradicts the posture the project has
already chosen for the other half of the same deliverable.

There is a second, subtler cost to the CLI: we inherit the user's git. Version skew across a 10-year spread of
distro packages, and ambient configuration — `core.hooksPath`, `credential.helper`, aliases, `core.autocrlf` —
leaking into our store. It is *containable*: `GIT_DIR`, `GIT_INDEX_FILE`, and `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM`
(git(1), ENVIRONMENT: "Can be set to `/dev/null` to skip reading configuration files of the respective level"; note
`NUL` on Windows) neutralise nearly all of it, and every experiment in this document ran under exactly that isolation.
But it is a permanent tax on every call site, and `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` require git ≥ 2.32,
which is itself a version floor we would have to detect and enforce.

**Performance is not an argument against the CLI.** Measured on this machine (Linux, git 2.55.0, warm cache):
a first Snapshot of 2000 files / 5.4 MB across 50 directories, done as three processes
(`hash-object -w --stdin-paths` → `update-index --index-info` → `write-tree` + `commit-tree` + `update-ref`),
took **0.41 s wall**; a follow-up Snapshot touching 5 files, reusing the same index file, took **0.063 s**.
Process count is O(1) per Snapshot, not O(files). Windows process creation is materially more expensive than
Linux's and was not measured here, so treat the Windows figure as unknown rather than equivalent.

### Build and cross-compilation

`libgit2-sys` declares `cc` as a build dependency and vendors the libgit2 sources, exposed through git2's
`vendored-libgit2` feature
([`libgit2-sys/Cargo.toml`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/libgit2-sys/Cargo.toml),
[`Cargo.toml`](https://github.com/rust-lang/git2-rs/blob/f42a01267a3042b26d30e9d8acf286c6c739bd8a/Cargo.toml)).
So a C toolchain is required for the target — a real cost for cross-compilation, and the reason `cross`/`cargo-zigbuild`
or a per-target CI runner enters the picture. It is also a *well-trodden* cost: Cargo itself ships this way on all
three platforms. Vendoring (rather than linking a system libgit2) also pins the libgit2 version, which removes a
whole class of "works on my distro" bugs.

Note the default features we want are small: `git2` with `default-features = false` and no `ssh`/`https` avoids
libssh2 and OpenSSL entirely, since the History Store has no remote (map.md: "Any remote … out of scope").
That removes the worst part of libgit2's build story.

`gix` is pure Rust and cross-compiles with no toolchain beyond `rustup target add`. If build simplicity were the
only axis, gitoxide would win outright.

### Licensing

| Component | Licence | Consequence for us |
|---|---|---|
| gitoxide (`gix` and sub-crates) | MIT OR Apache-2.0 (`gix/Cargo.toml`) | none |
| `git2` / `libgit2-sys` bindings | MIT OR Apache-2.0 | none |
| libgit2 itself | GPLv2 **with a linking exception**: "the authors give you unlimited permission to link the compiled version of this library into combinations with other programs, and to distribute those combinations without any restriction" ([COPYING](https://github.com/libgit2/libgit2/blob/0551dfd4ad989b6a3d5683c0d4cf326c6efef929/COPYING)) | statically linking libgit2 into a closed or permissively-licensed Keepster binary is explicitly permitted; the GPL still governs modifications to libgit2 itself |
| `git` binary | GPLv2 | invoked as a separate process, so no linking question arises at all |

No candidate is licence-blocked. libgit2's exception exists precisely for this use.

### Maturity

- **libgit2** is the mainstream embedded git and the engine behind Cargo's own git support; `libgit2-sys 0.18.7+1.9.6`
  tracks libgit2 1.9.6.
- **gitoxide** is explicitly pre-1.0 and says so. Its own README places `gix` and `gix-object` under
  **"Initial Development … may be missing some features and thus are somewhat incomplete"**, `gix-ref` and `gix-config`
  among **"Stabilization Candidates"**, and only `gix-lock` and `gix-tempfile` at a production stability tier
  ([README.md](https://github.com/GitoxideLabs/gitoxide/blob/0cd9319555ff2b30d8eca95eb3befba018b15de5/README.md)).
  The pieces we lean on hardest (`gix-ref`, `gix-object`) are among the better-developed ones, but `gix 0.87.1`
  means 87 breaking-change opportunities so far, and we would be tracking a moving API for the lifetime of the project.
  `SHORTCOMINGS.md` records nothing that bites this use case (its `gix-index` and `gix-pack` entries concern split
  indexes, 32-bit address limits and mmap'd IO errors).
- **git** is the reference implementation, and is also the only candidate whose *behaviour* we would be reading
  out of a manual page rather than a header.

---

## Recommendation

**Build the History Store on `git2` (libgit2), with `default-features = false` and `vendored-libgit2`, and treat the
`git` binary as absent.** Implement pruning ourselves as mark-and-sweep over `refs/keepster/…` using
`Revwalk` + `Odb::foreach` + `PackBuilder`, run from the daemon's Retention pass.

Why this and not the alternatives:

- **Not the `git` binary**, because it cannot be assumed on Windows, and because depending on it contradicts the
  single-artifact posture the project already chose for the web UI. Its GC advantage is real but buys one operation
  at the price of a runtime dependency on all three platforms and a permanent isolation tax on every call.
- **Not `gix`**, despite pure-Rust builds and a friendlier licence, because it is pre-1.0 with the exact crates we
  need still classed "initial development", because packed-refs compaction is reachable only by dropping beneath its
  public API, and because it lacks delta compression in pack writing — which matters precisely in the one area
  (our own GC) where we already have to do the work by hand. Revisit at gitoxide 1.0.
- **`git2`** wins on the six operations it covers natively and maturely, loses nothing on licensing, and its one
  genuine cost — a C toolchain per target — is a build-time problem with a known solution rather than a runtime
  problem on the user's machine.

**Main risk: we own garbage collection, and getting it wrong is destructive.** A sweep that misjudges reachability
deletes Snapshots the user still expects; one that is too timid leaks disk forever. Compounding it: the store is
written by a live daemon, so the sweep races its own writer — the same hazard git-gc(1) warns about for
`--prune=now`. Mitigations to carry into the spec: take an exclusive store lock for the duration of a sweep;
sweep only objects older than a grace period rather than everything unreachable; and, before deleting anything,
verify the freshly built pack contains every object reachable from every ref.

Second risk, smaller: the C toolchain. It shows up as CI complexity per target and as a longer cold build, not as
anything the user ever sees. Budget for a per-target CI runner rather than a single cross-compiling host.

Worth flagging to ticket 06 (retention), not decided here: if each Segment were packed into its own packfile,
retention could drop a whole file and sidestep reachability GC almost entirely — at the cost of losing blob
deduplication *between* Segments. That trade (disk vs. the risk above) belongs to the retention design.

---

## Explicitly uncertain

Stated as uncertainty rather than guessed:

- **No candidate was compiled or executed.** `gix` and `git2` were verified at the level of their own source and
  documentation only. The `git` rows are the only ones backed by execution. Before the spec hardens, a throwaway
  prototype should prove, in `git2`: an owner-less `Index` → `write_tree_to` round trip; a parentless commit with a
  trailer; ~500 refs created, packed via `refdb_compress`, and deleted; and a mark-and-sweep prune that a
  subsequent `git fsck` (run manually, not by us) declares clean.
- **`refdb_compress` at hundreds of refs** is documented as a *suggestion* to the backend ("this mechanism is
  implementation specific"). That it produces a `packed-refs` file equivalent to `git pack-refs` was not verified.
- **gitoxide's packed-refs write-through** (`PackedRefs::DeletionsAndNonSymbolicUpdates…`) was read in source but not
  exercised; whether driving it through `repo.refs` alongside the high-level API is safe is unknown.
- **Windows behaviour was not tested at all** — neither process-spawn cost for the CLI option, nor libgit2's
  file-locking and rename semantics under a live editor, nor `packed-refs` rewriting while another process reads.
  This is the largest untested surface in the whole comparison and applies to whichever engine is chosen.
- **`crate-status.md` and gitoxide's code disagree** on commit trailer parsing (the checkbox is unticked, the code
  exists). Treat that file as a strong hint, not as a contract.
- **No performance figure for `git2` or `gix`** — the 0.41 s / 0.063 s numbers above are the `git` CLI on Linux only,
  and say nothing about how the libraries compare.

## How this was verified

- gitoxide read from a shallow clone pinned at `0cd9319`; libgit2 and git2-rs read from raw sources at the SHAs listed
  at the top. Every API claim cites a file and line in the implementation's own tree, not a third-party summary.
- The `git` rows come from a scripted experiment in a throwaway bare repository under the session scratchpad, run with
  `GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null` and an explicit `GIT_DIR`/`GIT_INDEX_FILE`.
  This repository's own `.git` was never touched.
