# 03 — Where the Registry and the Snapshot index live

Research for `.scratch/keepster/issues/03-metadata-store.md`. Every claim is cited to a primary source
(sqlite.org, the git-scm.com man pages, the rusqlite repository and docs.rs, the Rust standard library docs).
Anything not backed by a primary source is labelled as an estimate or as unverified. Where the documentation is
silent, this note says so rather than inferring.

## 1. The question is really three questions

The ticket treats "the metadata" as one thing. It is not. Three kinds of state are in play, with opposite
properties:

| State | Authoritative? | Derivable from git objects? | Volume |
| --- | --- | --- | --- |
| **Registry** — Project id, current directory path, per-Project settings | Yes. Nothing else records it. | **No.** No Snapshot records where the directory is. | One record per Project (tens) |
| **SnapshotId → (Segment ref, commit)** | No — a cache | Yes: `SnapshotId` is a commit trailer (ADR 0001) | One row per Snapshot (tens of thousands) |
| **Path → Snapshots that changed it** | No — a cache | Yes: tree diff of each commit against its parent | Several rows per Snapshot |

The right answer is therefore not one storage choice but two, and that split is what makes the recovery story
work. Forcing all three into one home is what makes every option look bad.

A fourth derived thing: ADR 0001 already states that "Ordering across Segments is ours to maintain: there is no
single revision walk over all history" (`docs/adr/0001-segmented-snapshot-chains.md`), so newest-first ordering
across Segments is also something we compute, not something git hands us.

## 2. Option A — SQLite via rusqlite

### 2.1 Crash safety under a killed daemon

This is the bullet SQLite answers most cleanly, and the documentation is unusually blunt about it:

> "Transactions are durable across application crashes regardless of the synchronous setting or journal mode."
> — https://sqlite.org/pragma.html#pragma_synchronous

> "An SQLite database is highly resistant to corruption. If an application crash, or an operating-system crash, or
> even a power failure occurs in the middle of a transaction, the partially written transaction should be
> automatically rolled back the next time the database file is accessed. The recovery process is fully automatic
> and does not require any action on the part of the user or the application."
> — https://sqlite.org/howtocorrupt.html (Overview)

> "SQLite has the important property that transactions appear to be atomic even if the transaction is interrupted
> by an operating system crash or power failure."
> — https://sqlite.org/atomiccommit.html §1

How far this goes: even the most reckless durability setting survives a `kill -9` — "Setting PRAGMA
synchronous=OFF can cause the database to go corrupt if there is an operating-system crash or power failure,
though this setting is safe from damage due to application crashes" (https://sqlite.org/howtocorrupt.html §7).
The only settings that make a process kill corrupting are `journal_mode=OFF` and `journal_mode=MEMORY`: "If the
application using SQLite crashes in the middle of a transaction when the MEMORY journaling mode is set, then the
database file will very likely go corrupt" (https://sqlite.org/pragma.html#pragma_journal_mode). We simply do not
use those.

**Recommended settings: `journal_mode=WAL`, `synchronous=NORMAL`.**

> "WAL mode is safe from corruption with synchronous=NORMAL … WAL mode is always consistent with
> synchronous=NORMAL, but WAL mode does lose durability. A transaction committed in WAL mode with
> synchronous=NORMAL might roll back following a power loss or system crash."
> — https://sqlite.org/pragma.html#pragma_synchronous

Losing the last few commits to a power cut is acceptable **only because the database is a cache** — the Snapshots
themselves are already durable git objects. That is the second reason the split in §1 matters. WAL is also the
more forgiving mode against lying hardware: "In WAL mode, the only time that a failed sync operation can cause
database corruption is during a checkpoint operation. A sync failure during a COMMIT might result in loss of
durability but not in a corrupt database file. Hence, one line of defense against database corruption due to
failed sync operations is to use SQLite in WAL mode and to checkpoint as infrequently as possible."
(https://sqlite.org/howtocorrupt.html §3.1)

WAL mode need only be set once: "PRAGMA journal_mode=WAL is persistent. If a process sets WAL mode, then closes
and reopens the database, the database will come back in WAL mode." (https://sqlite.org/wal.html §3.3)

### 2.2 The corruption modes that actually apply

None of them is "the daemon was killed". They are all environmental, and each one is engineerable:

| Mode | Source | What we must do |
| --- | --- | --- |
| Backup/sync tool copies the DB mid-transaction — "The backup copy then might contain some old and some new content, and thus be corrupt." | howtocorrupt §1.2 | Put the store in the platform data dir, never a synced folder. Document `VACUUM INTO` / `sqlite3_rsync` as the only safe copy. |
| `-wal`/`-journal` moved, deleted or mispaired — "Copying a database file without also copying its journal." | howtocorrupt §1.3, §1.4; wal.html §4 | Treat `.sqlite`, `-wal`, `-shm` as one indivisible unit for every create/delete/move. |
| Broken locking on network filesystems — "This is especially true of network filesystems and NFS in particular." | howtocorrupt §2.1; https://sqlite.org/faq.html (5); https://sqlite.org/whentouse.html §2 | Refuse (or loudly warn) when the store is on a network mount; WAL forbids it outright anyway (§2.3). |
| A non-SQLite reader `open()`/`read()`/`close()`ing the DB file — "the close() system call will cancel all POSIX advisory locks on the same file for all threads and all file descriptors in the process." | howtocorrupt §2.2 | Never read the database file with anything but the SQLite API. No "let me just copy it" helper. |
| Two copies of SQLite linked into one binary — "A close() operation on one connection might unknowingly clear the locks on a different database connection, leading to database corruption." | howtocorrupt §2.3 | A real Rust hazard: `libsqlite3-sys` bundled plus a system `libsqlite3` pulled in by another crate. Audit the dependency graph. |
| Carrying an open connection across `fork()` — "All kinds of locking problems will result and you can easily end up with a corrupt database." | howtocorrupt §2.7 | Directly relevant to daemonizing (ticket 05): open the connection **after** forking, never before. |
| Unlink or rename the DB while open (POSIX only; "Windows does not allow this to occur.") | howtocorrupt §2.5 | Never "atomically replace" the database by rename. To discard it, close all connections first. |
| Multiple hard links to the DB file | howtocorrupt §2.6 | Symlinks are resolved since 3.10.0; hard links are not. Canonicalise the store path once at startup. |
| Disk/flash failure, memory corruption | howtocorrupt §4, §5 | Detectable, not preventable — which is why the index must be rebuildable. |

Two of these (backup tools, network filesystems) are outside our control on a machine we do not administer. That
is the whole argument against putting *authoritative* state in SQLite — and no argument at all against putting a
*cache* there.

### 2.3 Concurrent access

The design already has exactly one writer. The map records "One global daemon + a project registry … CLI and web
UI talk to it", and ticket 07 states "The daemon is the only component that touches the store". SQLite endorses
precisely this shape:

> "SQLite only supports one writer at a time per database file. But in most cases, a write transaction only takes
> milliseconds and so multiple writers can simply take turns."
> — https://sqlite.org/whentouse.html §3

> "WAL provides more concurrency as readers do not block writers and a writer does not block readers. Reading and
> writing can proceed concurrently."
> — https://sqlite.org/wal.html §1

> "for any particular reader, the end mark is unchanged for the duration of the transaction, thus ensuring that a
> single read transaction only sees the database content as it existed at a single point in time."
> — https://sqlite.org/wal.html §2.2

Constraints that must be honoured:

- **Local disk only.** "All processes using a database must be on the same host computer; WAL does not work over
  a network filesystem. This is because WAL requires all processes to share a small amount of memory and
  processes on separate host machines obviously cannot share memory with each other."
  (https://sqlite.org/wal.html §1)
- **Readers must still handle `SQLITE_BUSY`.** "there are some obscure cases where a query against a WAL-mode
  database can return SQLITE_BUSY, so applications should be prepared for that happenstance" — notably during
  another connection's WAL cleanup, and during recovery after a crashed connection, when "An exclusive lock is
  held during recovery" (https://sqlite.org/wal.html §9). This is exactly the daemon-was-killed scenario, so the
  CLI's first read after a crash is the one most likely to hit it. rusqlite's default 5000 ms busy timeout covers
  it (https://docs.rs/rusqlite/latest/rusqlite/struct.Connection.html#method.busy_timeout).
- **Unbounded WAL growth is a long-lived-daemon risk.** "it is possible to get SQLite into a state where the WAL
  file will grow without bound, causing excess disk space usage and slow query speeds", the named causes being
  disabled auto-checkpointing and "Checkpoint starvation. A checkpoint is only able to run to completion, and
  reset the WAL file, if there are no other database connections using the WAL file."
  (https://sqlite.org/wal.html §6) A daemon that holds a long-running read transaction open for an HTTP stream
  can starve the checkpointer. Keep read transactions short.
- **Read-only clients work.** Since 3.22.0 "a read-only WAL-mode database file can be opened if the -shm and -wal
  files already exist or those files can be created" (https://sqlite.org/wal.html §1). The store directory is
  user-owned, so a CLI reading while the daemon is down is fine.
- **`PRAGMA foreign_keys` is per-connection and defaults to OFF** — "the default setting for foreign key
  enforcement is OFF … applications should set the foreign key enforcement flag as required by the application
  and not depend on the default setting" (https://sqlite.org/pragma.html#pragma_foreign_keys). Every connection
  must set it.

rusqlite enforces the single-writer model at the type level — `impl Send for Connection`, `impl !Sync for
Connection` — and is explicit about the rule:

> "Critically, even if this flag is used, the `Connection` is not safe to use across multiple threads
> simultaneously. To access a database from multiple threads, you should either create multiple connections, one
> for each thread (if you have very many threads, wrapping the `rusqlite::Connection` in a mutex is also
> reasonable)."
> — https://docs.rs/rusqlite/latest/rusqlite/struct.OpenFlags.html#associatedconstant.SQLITE_OPEN_FULL_MUTEX

Transactions fail safe: "Transactions will roll back by default. Use `commit` method to explicitly commit the
transaction" (https://docs.rs/rusqlite/latest/rusqlite/struct.Transaction.html). Note that
`PRAGMA journal_mode=WAL` returns the new mode, so it must be issued through `pragma_update_and_check` or a
plain query, not `pragma_update` (https://docs.rs/rusqlite/latest/rusqlite/struct.Connection.html).

### 2.4 The timeline queries

Both are ordinary indexed SQL: `ORDER BY committed_at DESC LIMIT n` for the cross-Segment timeline, and a
`(project_id, path, committed_at DESC)` index for "Snapshots in which this path changed". This is the only option
of the three that answers the second query without a full tree-diff walk of history (§4.2).

SQLite's own positioning is about exactly this substitution: "Many programs use fopen(), fread(), and fwrite() to
create and manage files of data in home-grown formats. SQLite works particularly well as a replacement for these
ad hoc data files. Contrary to intuition, SQLite can be faster than the filesystem for reading and writing
content to disk." (https://sqlite.org/whentouse.html §1) — quantified at "35% faster" for small blobs, with the
authors' own caveat that "The 35% figure above is approximate … Some reviewers of this article report that SQLite
has higher latency than direct I/O on their systems" (https://sqlite.org/fasterthanfs.html). Do not lean on the
number; lean on the index.

### 2.5 Dependency cost on three platforms

- **`bundled` removes the system dependency.** "If you use the `bundled` … features, `libsqlite3-sys` will use
  the `cc` crate to compile SQLite or SQLCipher from source and link against that. This source is embedded in the
  `libsqlite3-sys` crate and is currently SQLite 3.53.2." It "avoids many common build issues, and avoids
  depending on the version of SQLite on the users system (or your system), which may be old or missing. It's the
  right choice for most programs that control their own SQLite databases." (https://github.com/rusqlite/rusqlite
  README)
- **No Clang/LLVM needed.** "As of `rusqlite` 0.10.1, we avoid running `bindgen` at build-time by shipping
  pregenerated bindings for several versions of SQLite. … If you use any of the `bundled` features, you will get
  pregenerated bindings for the bundled version" (same README). Clang is required only for the opt-in
  `buildtime_bindgen`, `preupdate_hook` and `session` features, which we do not want.
- **Three-platform support is proven by the project's own CI**, which builds and tests `--features bundled` on
  `x86_64-pc-windows-msvc`, `x86_64-pc-windows-gnu`, `x86_64-unknown-linux-gnu` and `x86_64-apple-darwin` on
  every push (https://github.com/rusqlite/rusqlite/blob/master/.github/workflows/main.yml).
- **Small runtime tree**: `libsqlite3-sys`, `bitflags`, `fallible-iterator`, `fallible-streaming-iterator`,
  `smallvec`, plus `hashlink` via the default `cache` feature — roughly seven crates, derived from
  https://github.com/rusqlite/rusqlite/blob/master/Cargo.toml and
  https://crates.io/api/v1/crates/rusqlite/0.40.2/dependencies. `libsqlite3-sys` has no non-optional runtime
  dependencies of its own.
- **Licensing is clean for a shipped binary**: rusqlite and libsqlite3-sys are MIT; bundled SQLite is public
  domain (README, licence section).
- **The costs.** A C toolchain must exist on every *build* machine — the README does not say so in prose, but
  `libsqlite3-sys/build.rs` drives `cc::Build` over `sqlite3/sqlite3.c`, and that translation unit is large: the
  vendored `sqlite3.c` on master is 9,515,341 bytes, and sqlite.org describes the amalgamation as "about 238K
  lines of code (or 145K if you omit blank lines and comments)" (https://sqlite.org/amalgamation.html). **No
  primary source states how much build time this adds.** It is gated by `cargo:rerun-if-changed`, so it is a
  first-build and CI-cache-miss cost, not a per-edit cost. Any figure needs local measurement.
- **Cross-link to ticket 01.** If the git engine turns out to be `git2`/libgit2, the C toolchain is already
  required and SQLite adds nothing. If ticket 01 picks `gitoxide`, SQLite becomes the project's only C
  dependency. Either way this is a build-system cost, not a runtime one — the shipped artefact stays one
  self-contained binary, which the map requires ("One artifact, no runtime dependencies").

## 3. Option B — plain files inside the History Store

### 3.1 Where it wins

For the Registry this is the strongest option. The write is: serialise a small struct, write a temp file, fsync,
rename over the target.

- `std::fs::rename` "Renames a file or directory to a new name, replacing the original file if `to` already
  exists", mapping to `rename(2)` on Unix and "`MoveFileExW` with a fallback to `SetFileInformationByHandle` on
  Windows"; "On Windows 10 version 1607 and above, the behavior is the same as Unix if the filesystem supports
  `FileRenameInfoEx`" (https://doc.rust-lang.org/std/fs/fn.rename.html).
- `File::sync_all` "Attempts to sync all OS-internal file content and metadata to disk"
  (https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_all).

Honest gaps: the Rust docs promise *replacement* semantics on Windows, not *atomicity* in those words; and
rename-over-a-file whose handle is held open by an antivirus or indexer failing with a sharing violation is
widely reported but **is not documented on that page** — treat a bounded retry as prudent and the need for it as
unverified until measured on Windows.

Against that: no schema migration, no locking model, no `-wal` sidecar, no C compiler, and a human can read and
repair the file in an editor. For a few dozen records that is the entire story.

### 3.2 Where it fails

It cannot serve the second timeline query. "Snapshots in which a given path changed" over tens of thousands of
Snapshots is a secondary-index lookup; a flat file means loading and scanning everything on every request.
Growing that into per-path files, sort orders and compaction is reimplementing a database — and SQLite's
positioning statement is aimed squarely at this case: "SQLite does not compete with client/server databases.
SQLite competes with fopen()." (https://sqlite.org/whentouse.html §1)

Second failure: blast radius. One large metadata file rewritten on every Snapshot puts the whole Registry at risk
on every Snapshot. Per-Project files fix that; per-Snapshot files reintroduce the scan.

## 4. Option C — git objects only, index built on demand

### 4.1 What git genuinely gives us

- **Trailers are already required.** ADR 0001 puts `SnapshotId` in a commit trailer. Trailers are "trailer lines
  that look similar to RFC 822 e-mail headers, at the end of the otherwise free-form part of a commit message",
  extracted "by looking for a group of one or more lines that (i) is all trailers, or (ii) contains at least one
  Git-generated or user-configured trailer and consists of at least 25% trailers. The group must be preceded by
  one or more empty (or whitespace-only) lines. The group must either be at the end of the input or be the last
  non-whitespace lines before a line that starts with `---`"
  (https://git-scm.com/docs/git-interpret-trailers). Reading them back is a documented placeholder:
  `%(trailers:key=<key>)` with `valueonly`, `unfold` and `separator=<sep>` options
  (https://git-scm.com/docs/pretty-formats), e.g.
  `git log --format='%H %(trailers:key=SnapshotId,valueonly,unfold=true)'`. Verified locally against git 2.55.0.
  **Design constraint:** the trailer block must be the last blank-line-separated paragraph and satisfy the 25%
  rule — free-form text appended after the trailers silently destroys the block. The commit message template is
  therefore load-bearing.
- **Segments enumerate cheaply and without a commit walk.** `git for-each-ref` sorts on any field: "Prefix `-` to
  sort in descending order of the value … You may use the `--sort=<key>` option multiple times, in which case the
  last key becomes the primary key" (https://git-scm.com/docs/git-for-each-ref). Since ADR 0001 names Segment
  refs to sort chronologically, `--sort=-refname` gives newest-Segment-first for free.
- **Bulk object reads are supported.** `--buffer` is "much more efficient when invoking `--batch-check` or
  `--batch-command` on a large number of objects", and `--batch`/`--batch-check` "print information about them in
  the same order as they have been read" (https://git-scm.com/docs/git-cat-file).
- **Refs are git's one transactional primitive.** `git update-ref --stdin`: "If all `<ref>`s can be locked with
  matching `<old-oid>`s simultaneously, all modifications are performed. Otherwise, no modifications are
  performed." The `<old-oid>` check is the only compare-and-swap git offers. With an important caveat: "Note that
  while each individual `<ref>` is updated or deleted atomically, a concurrent reader may still see a subset of
  the modifications." (https://git-scm.com/docs/git-update-ref) Writers get all-or-nothing; readers get no
  isolation.

So git *can* be the source of truth. It is going to be regardless.

### 4.2 Why it cannot be the only store

**The path query is a full tree-diff walk, and git's own answer to it is lossy.** `git log -- <path>` shows "only
commits that are enough to explain how the files that match the specified paths came to be"; the History
Simplification section defines the mechanism — "We shall call commits that modify `foo` !TREESAME, and the rest
TREESAME" — and the default mode "prunes some side branches if the end result is the same"
(https://git-scm.com/docs/git-log). For a local-history timeline that must show **every** capture, that is the
wrong semantics; `--full-history` is required. `--follow` is worse: it "works only for a single file". And there
is no path index in git — the documented accelerator is an optional side structure: "With the `--changed-paths`
option, compute and write information about the paths changed between a commit and its first parent. This
operation can take a while on large repositories. It provides significant performance gains for getting history
of a directory or a file with `git log -- <path>`" (https://git-scm.com/docs/git-commit-graph). Git's own
solution to this problem is to build an index. Note honestly that git **does not document** the complexity of an
unaccelerated path-limited walk; the only explicit performance sentence on git-log concerns `--simplify-merges`.

**Git notes are actively unsuitable here.** Notes attach to an object by its name, stored by default in
`refs/notes/commits` (https://git-scm.com/docs/git-notes). Three disqualifiers:

1. ADR 0001 says thinning a Segment recreates its commits. A note is bound to the old commit SHA, so thinning
   orphans every note in that Segment. Git's mitigation is `notes.rewriteRef`, which "Does not have a default
   value; you must configure this variable to enable note rewriting", and `notes.rewrite.<command>`, which covers
   only `amend` and `rebase` — neither of which we use, since we write with plumbing. We would have to copy notes
   by hand on every prune.
2. "Every notes change creates a new commit at the specified notes ref." (git-notes, DISCUSSION) One extra commit
   per Snapshot, on a ref nobody prunes, in a store where ADR 0001 disables reflogs specifically so unreachable
   objects can be freed. The notes ref becomes a second, unmanaged history.
3. Every note edit is a read-modify-write of one ref, i.e. a lost-update race unless serialised externally. The
   man page documents **no** atomicity guarantee for `git notes add`, and documents merge strategies
   (`manual`, `ours`, `theirs`, `union`, `cat_sort_uniq`) precisely because divergence is expected and resolved
   after the fact.

A trailer, by contrast, lives inside the commit message and survives a rewrite as long as the message is copied
— which is why ADR 0001 chose it.

**Git is candid that it does not promise concurrent-access safety.** The clearest statement in the man pages:
"when git gc runs concurrently with another process, there is a risk of it deleting an object that the other
process is using but hasn't created a reference to. This may just cause the other process to fail or may corrupt
the repository if the other process later adds a reference to the deleted object … these features fall short of a
complete solution, so users who run commands concurrently have to live with some risk of corruption (which seems
to be low in practice)." (https://git-scm.com/docs/git-gc, NOTES) Meanwhile https://git-scm.com/docs/gitfaq has
no section on concurrent access at all, git-update-index never mentions locking, and gitrepository-layout does
not describe loose-object writes as write-then-rename — that last one is implementation lore, not a documented
guarantee. This matters directly for ticket 06: gc must not run concurrently with a Snapshot in flight.

**Ref contention is a failure mode, not a queue.** `git update-ref --stdin prepare` "will create lock files for
all queued reference updates. If one reference could not be locked, the transaction will be aborted"
(https://git-scm.com/docs/git-update-ref); the `reference-transaction` hook documents the state machine
"preparing / prepared / committed / aborted" (https://git-scm.com/docs/githooks). The retry knobs
`core.filesRefLockTimeout` and `core.packedRefsTimeout` exist but **could not be verified against the rendered
git-config man page** (the page truncates before `core.*`); treat their defaults as unverified.

**The Registry is not expressible at all.** A watched directory's path is not a property of any Snapshot. No
index-building strategy recovers it.

**"Build it in memory at startup" does not survive the first HTTP request.** The daemon would have to walk every
commit of every Segment of every Project and tree-diff each one before answering a page load, then hold the
result resident. Estimate, not a cited figure: a heavy user at ~500 Snapshots/day/project across a 90-day
retention ladder is ~45k Snapshots per Project, several path rows each. That is a multi-second cold start
recomputed identically on every daemon restart. The correct name for "an in-memory index we persist so we need
not rebuild it" is a database.

### 4.3 Two incidental findings worth carrying to other tickets

- **Reflogs are already off in a bare store.** `core.logAllRefUpdates` "is true by default in a repository that
  has a working directory associated with it, and false by default in a bare repository", and even when true it
  auto-creates logs only for `refs/heads/`, `refs/remotes/`, `refs/notes/` and `HEAD` — custom hierarchies need
  `always` (https://git-scm.com/docs/git-config/2.21.0). So ADR 0001's requirement
  (`core.logAllRefUpdates=false`) is satisfied by default in a bare store; setting it explicitly is belt and
  braces, not the load-bearing step it reads as.
- **Segment ref names must not nest.** Under the files backend a ref cannot be both a file and a directory, so
  `refs/segments/2026-09` and `refs/segments/2026-09/01` cannot coexist — the reftable design document notes that
  reftable "accepts both refs/heads/foo and refs/heads/foo/bar as distinct references" and that this "may confuse
  versions of Git using $GIT_DIR/refs directory tree to maintain references"
  (https://git-scm.com/docs/reftable). Segment naming (ticket 06) must pick one flat granularity. The same
  document notes packed-refs "Lookup of a single reference requires linearly scanning the file", and
  git-pack-refs adds that "Subsequent updates to branches always create new files under `$GIT_DIR/refs`
  directory hierarchy" — so an active Segment ref is loose again after every Snapshot, and `git pack-refs` is
  housekeeping we must schedule, not something that stays done.

## 5. Recommendation

**Split the state. Neither store wins outright, and the split makes both cheap.**

1. **Registry → plain files, one per Project, in the data directory. Never in SQLite.**
   Layout: `<data-dir>/projects/<project-id>/` holding that Project's History Store plus a small daemon-owned
   `project.toml` (`id`, current `path`, `created_at`, `schema_version`). The Registry is the union of those
   files, not one global file — registering, moving or unwatching a Project rewrites exactly one small file, and
   a damaged file costs one Project instead of all of them. Write it temp-file + `sync_all` + `rename`
   (https://doc.rust-lang.org/std/fs/fn.rename.html), with a bounded retry on Windows.

   Keep daemon-owned state (`project.toml`) in a *different file* from user-edited configuration: a daemon that
   rewrites the user's config on every path change destroys their comments and formatting. That is an input to
   ticket 04, not a decision this ticket should settle alone.

   Also stamp the Project id into each Snapshot commit's trailer block beside `SnapshotId`
   (e.g. `Keepster-Project: <id>`). It costs ~40 bytes per commit and makes every History Store
   self-identifying, so a store found without its `project.toml` still knows whose it is.

2. **SnapshotId index, path-change index and cross-Segment ordering → one SQLite database per data directory**,
   `rusqlite` with `bundled`, `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON` on every connection,
   opened for writing by the daemon only. It is declared — in the schema, in the code and in an ADR — a **derived
   cache**: deletable at any time, rebuilt from git objects. Nothing in it is authoritative.

3. **Hard invariant — git first, index second.** A Snapshot is durable once its commit exists and its Segment ref
   has been updated (`update-ref` is atomic per ref). Only then does the index transaction commit. The index can
   therefore only ever *lag* git; it can never name a commit that does not exist. Writing the index first would
   let a killed daemon leave a `SnapshotId` pointing at nothing — a broken timeline entry no scan can repair.
   This ordering deserves its own ADR line.

4. **Per-Segment watermark.** Store, per Segment ref, the tip commit oid the index was built from. At startup
   compare each ref's current tip to its watermark: equal → skip; watermark is an ancestor → index only the gap;
   anything else (thinning recreated that Segment's commits, per ADR 0001) → drop that Segment's rows and re-scan
   just that Segment. The normal "daemon was killed" case becomes a few-millisecond catch-up over the same code
   path as a full rebuild.

5. **Reject git notes outright** (rewrite-fragile, one commit per note, lost-update race, unmanaged second
   history), and **reject an index built only on demand** (cannot answer the path query without a full
   tree-diff walk, and cannot hold the Registry at all).

Placement rules that fall out of the evidence: the database lives in the platform data directory, never on a
network mount or in a synced folder; its path is canonicalised once at startup (hard-link and symlink hazards);
`.sqlite`, `-wal` and `-shm` are always created and deleted as a set; the connection is opened *after* any
`fork()`; nothing but the SQLite API ever opens that file; and the daemon's single-instance lock (ticket 05) is
what actually enforces the one-writer rule.

## 6. Recovery procedure when the metadata is destroyed

Preconditions: the git objects and Segment refs survive; the store directory is readable.

**Phase 0 — detect.** At startup, and on any `SQLITE_CORRUPT`/`SQLITE_NOTADB` at runtime, run
`PRAGMA quick_check`: "If pragma integrity_check finds no errors, a single row with the value 'ok' is returned",
and quick_check "runs in O(N) time whereas PRAGMA integrity_check requires O(NlogN) time"
(https://sqlite.org/pragma.html#pragma_integrity_check). A missing file, an unknown `schema_version`, or a
non-`ok` result all mean the same thing: the index is absent.

**Phase 1 — discard.** Close every connection, then delete `index.sqlite`, `index.sqlite-wal` and
`index.sqlite-shm` **together**; deleting some but not others is itself a corruption vector
(https://sqlite.org/howtocorrupt.html §1.3, §1.4). Do not unlink or rename the file while a connection is open
(howtocorrupt §2.5). If only the WAL needs clearing rather than the whole file, the documented safe route is
"open the database file using one of the sqlite3_open() interfaces then immediately close the database"
(https://sqlite.org/wal.html §4). Then create a fresh database and apply the schema.

**Phase 2 — enumerate Segments.** `for-each-ref` over the Segment namespace with `--sort=-refname`, which is
newest-first because ADR 0001 names Segment refs to sort chronologically. Rebuild newest Segment first, so the
UI's default view becomes answerable long before the whole history has been scanned.

**Phase 3 — rebuild the SnapshotId index.** For each Segment, walk tip → root (the chain is linear by
construction) and read commit objects in bulk (`cat-file --batch --buffer`, or the chosen engine's equivalent —
ticket 01). From each commit take the `SnapshotId` trailer, the committer timestamp and the commit oid, and
insert `(project_id, snapshot_id, segment_ref, commit_oid, committed_at)`. Parse trailers by the documented
block rule, not by "last line" (https://git-scm.com/docs/git-interpret-trailers).

**Phase 4 — rebuild the path index.** For each commit, tree-diff against its parent; for a Segment's parentless
root commit, diff against the empty tree so the initial capture registers as a change to every file. Insert one
row per changed path. Do **not** rebuild this by shelling out to `git log -- <path>`: that command is documented
as showing "only commits that are enough to explain how the files … came to be", which is deliberately
incomplete. This phase is the expensive one; it is the price of never running a path-limited walk at request
time.

**Phase 5 — record watermarks and verify.** Write each Segment's tip oid as its watermark, then
`PRAGMA integrity_check` (expects `ok`) and compare per-Segment row counts against commits walked.

**Phase 6 — the Registry, which is a separate recovery.** The Registry is not in the database, so a destroyed
index never touches it. If a `project.toml` is itself lost:

- the Project id is recovered from the store directory name and confirmed against the `Keepster-Project`
  trailer in that store's commits;
- the Snapshots are intact and browsable by id;
- **the watched directory path is not recoverable from git objects.** This is the one irreducible loss, and it is
  irreducible under *every* option in this ticket, because no Snapshot records where the directory was. The user
  re-runs `keepster watch <dir>`; the daemon finds an existing store whose id matches and reattaches it
  instead of starting a new history.

**Incremental variant — the common case.** A daemon killed mid-Snapshot needs none of the above. Phase 2 plus the
watermark comparison in §5.4 finds the at-most-one un-indexed commit per Segment and indexes it. Full rebuild is
the fallback, not the routine.

## 7. Consequences for other tickets

- **01 (git engine)** must support bulk commit reads, commit-to-parent tree diffs, and access to raw commit
  messages so trailers can be parsed. The rebuild path depends on all three. If it selects libgit2, the C
  toolchain SQLite needs is already required.
- **04 (configuration schema)** inherits the split between daemon-owned `project.toml` and user-owned config.
- **05 (daemon lifecycle)** owns the single-instance lock that makes "one writer" true, and must open the SQLite
  connection *after* any `fork()` (https://sqlite.org/howtocorrupt.html §2.7).
- **06 (retention)** must invalidate a Segment's index rows when thinning rewrites its commits — the watermark
  makes this automatic. It must also keep `gc` away from a Snapshot in flight (git's own docs concede a
  concurrency risk), pick a flat Segment ref naming scheme (no nesting under the files backend), and schedule
  `git pack-refs`.
- **07 (API surface)** should keep the daemon the only process opening the database for writing, keep read
  transactions short so checkpoints are not starved, and treat read-only CLI access while the daemon is down as
  an explicitly narrow path that handles `SQLITE_BUSY`.
- **Two ADRs are warranted**: "the metadata index is a derived cache, rebuildable from git objects", and "the git
  commit and ref update always precede the index write".

## Sources

SQLite
- https://sqlite.org/atomiccommit.html
- https://sqlite.org/howtocorrupt.html
- https://sqlite.org/wal.html
- https://sqlite.org/lockingv3.html
- https://sqlite.org/faq.html
- https://sqlite.org/pragma.html (`synchronous`, `journal_mode`, `foreign_keys`, `integrity_check`, `quick_check`)
- https://sqlite.org/whentouse.html
- https://sqlite.org/fasterthanfs.html
- https://sqlite.org/amalgamation.html

rusqlite / libsqlite3-sys
- https://github.com/rusqlite/rusqlite (README)
- https://github.com/rusqlite/rusqlite/blob/master/Cargo.toml
- https://github.com/rusqlite/rusqlite/blob/master/libsqlite3-sys/build.rs
- https://github.com/rusqlite/rusqlite/blob/master/.github/workflows/main.yml
- https://docs.rs/rusqlite/latest/rusqlite/struct.Connection.html
- https://docs.rs/rusqlite/latest/rusqlite/struct.OpenFlags.html
- https://docs.rs/rusqlite/latest/rusqlite/struct.Transaction.html
- https://crates.io/api/v1/crates/rusqlite/0.40.2/dependencies

git
- https://git-scm.com/docs/git-interpret-trailers
- https://git-scm.com/docs/pretty-formats
- https://git-scm.com/docs/git-notes
- https://git-scm.com/docs/git-log
- https://git-scm.com/docs/git-commit-graph
- https://git-scm.com/docs/git-for-each-ref
- https://git-scm.com/docs/git-cat-file
- https://git-scm.com/docs/git-update-ref
- https://git-scm.com/docs/git-pack-refs
- https://git-scm.com/docs/gitrepository-layout
- https://git-scm.com/docs/githooks (`reference-transaction`)
- https://git-scm.com/docs/git-gc
- https://git-scm.com/docs/git-prune
- https://git-scm.com/docs/git-config/2.21.0 (`core.logAllRefUpdates`)
- https://git-scm.com/docs/reftable

Rust standard library
- https://doc.rust-lang.org/std/fs/fn.rename.html
- https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_all

Local verification
- `git version 2.55.0` in this repository, for the `%(trailers:key=…)` and `for-each-ref --sort` syntax.

Unverified / documented gaps, stated as such above
- `core.filesRefLockTimeout` and `core.packedRefsTimeout` defaults (git-config page truncates before `core.*`).
- Windows rename-over-open-file sharing violations (not in the `std::fs::rename` docs).
- Build-time cost of compiling the SQLite amalgamation (no primary source states a figure).
- The complexity of an unaccelerated path-limited `git log` walk (git documents no bound).
