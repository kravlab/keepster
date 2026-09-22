# Keepster v1 — specification

Status: ready-for-agent

Synthesised from the wayfinder map (`map.md`, 35 resolved decision tickets) and ADRs 0001–0014. Where this
document and a ticket disagree, the ticket's answer is the record and this document is the bug. Vocabulary is
`CONTEXT.md`'s: Watched Root, Registry, Working Tree, Include Rules, Snapshot, Withheld, SnapshotId, Segment,
History Store, Trigger, Reconciliation, Capture Preview, Empty Directory, Retention, Segment Period, Retention
Ladder, Tier, Pin, Maintenance Pass, Forget, Degraded, Unavailable, Restore, Safety Snapshot.

## Problem Statement

Work is destroyed between commits, and version control does not help. An AI agent rewrites twelve files to fix
one; a script deletes a directory; a stray `git checkout` discards an afternoon; an editor's own local history
stops at the editor's boundary and knows nothing about files changed by anything else. The gap is not durability —
the machine is fine — it is **the last few hours of a directory's life**, which nothing keeps unless the person
remembered to commit, and which is exactly the window in which modern tooling does the most damage the fastest.

Existing answers all miss it. A backup tool optimises for surviving the loss of the machine: chunked, deduplicated,
encrypted archives from which reading one file means walking a tree or mounting a snapshot. An editor's local
history covers only what that editor touched. Committing more often is advice, not a mechanism, and it is advice
the person is busy ignoring precisely when an agent is editing their tree.

What is missing is insurance that runs **below the editor and beside version control**: something that notices
changes in a directory by itself, keeps every version of every file for a bounded window, and answers "show me
this file yesterday, and put it back" in the time it takes to look.

## Solution

**Keepster is a background daemon that captures Snapshots of Watched Roots into its own git-backed History Store,
with a CLI and a localhost web UI for browsing, diffing, searching and restoring them.**

- **Any directory can be a Watched Root**, whether or not it is a git repository. Keepster never touches the
  directory's own `.git`.
- **Snapshots happen by themselves.** A filesystem watcher gives low latency, a periodic floor is available, and
  Reconciliation — a full comparison against the latest Snapshot — is the guarantee, because every platform loses
  filesystem events and several losses are never announced.
- **The History Store is read-optimised.** Every version is addressable by (Snapshot, path); two versions diff with
  no common ancestor; nothing must be unpacked to read one file. This is why git backs it, and it is the property
  that separates Keepster from a backup tool.
- **Retention is a ladder**: everything for 24 hours, hourly to 7 days, daily to 30 days, then discarded — bounded
  by design, with Pins for anything that must outlive it.
- **Restore is undoable.** It writes atomically over the Working Tree, and takes a Safety Snapshot of the current
  state first, so the restore itself can be rolled back.
- **The daemon is never elevated, never sends anything anywhere, and holds no remote.** Everything it does happens
  on one machine, as one user.
- **The HTTP API is the product's public contract**; the CLI and the embedded SPA are two clients of it with no
  privileged endpoints between them.

Coverage is honest about its own limits: a Root the daemon cannot capture is `Unavailable`, one it can only watch on
worse terms is `Degraded`, a Snapshot that could not read everything is marked incomplete and names what it omits,
and a path whose content was deliberately not held is Withheld with a reason. `keepster status` is the single
authority on all of it and exits non-zero whenever any Root is not `Watching`.

## User Stories

### Registering and knowing what is covered

1. As a developer, I want to register any directory with one command, so that history starts without me configuring
   anything first.
2. As a developer, I want a directory that is not a git repository to be watchable, so that `~/Documents/notes` is
   insured exactly like a code tree.
3. As a developer, I want the registration to survive renaming or moving the directory, so that my history is not
   attached to a path I am free to change.
4. As a developer, I want a move to be something I state explicitly, so that no heuristic ever grafts a stranger's
   history onto my directory.
5. As a developer, I want Roots forbidden to overlap, so that no file belongs to two histories and no command has to
   ask me which Root I meant.
6. As a developer registering a directory through a symlink, I want to be asked whether the link or its real path is
   the Root, so that I know which one later retargeting follows.
7. As a developer, I want to see, before registering a large directory, what would be captured and what would be
   excluded, so that I do not discover the answer as disk usage.
8. As a developer, I want one command that tells me whether my work is being captured right now, so that I never
   believe I am covered when I am not.
9. As a developer, I want that command to exit non-zero when anything is wrong, so that I can put it in a shell
   prompt or a periodic check.
10. As a developer, I want a Root whose directory is missing or unmounted to stay registered with its history kept,
    so that unplugging a drive does not delete my insurance.
11. As a developer, I want a Root whose configuration failed to load to keep running on the last good configuration
    and to say so, so that a typo never silently changes what is captured.
12. As a developer on a machine where startup Reconciliation is switched off, I want the Root reported as "coverage
    not guaranteed", so that the trade-off I made is visible afterwards.
13. As a developer, I want to deregister a Root without losing its history, so that unwatching is reversible.
14. As a developer, I want a deregistered Root's history to decay down the normal ladder to its last pinned state,
    so that it neither vanishes in thirty days nor holds disk forever.
15. As a developer, I want erasing history to require an explicit command that says so, so that no install,
    uninstall or downgrade can do it for me.

### Capture

16. As a developer, I want a Snapshot shortly after I stop typing, so that the version I just lost is in history
    before I notice it is gone.
17. As a developer, I want a rate limit I control, so that a noisy build does not turn into a Snapshot per second.
18. As a developer, I want Reconciliation to be the guarantee rather than the watcher, so that the changes my OS
    never announced are still captured.
19. As a developer, I want Reconciliation to run at daemon start and after every OS rescan signal, so that downtime
    and queue overflow are not silent holes.
20. As a developer, I want an editor's atomic save (write-temp-then-rename) captured as an edit, so that the common
    case is not the broken one.
21. As a developer, I want my own include and exclude rules to be authoritative, so that `node_modules` and build
    output never enter history and `.gitignore` is an option rather than the law.
22. As a developer, I want an excluded directory never to be entered, so that excluding `node_modules` costs nothing
    to walk and nothing to watch.
23. As a developer, I want to ask why a specific path is or is not captured and which rule decided it, so that "why
    wasn't my file saved" has an answer instead of a theory.
24. As a developer, I want secrets refused by default, so that `.env` and private keys do not enter history because
    I forgot to exclude them.
25. As a developer, I want to opt a specific secret-looking path in explicitly, so that a deliberate choice is
    possible without weakening the default.
26. As a developer, I want a file over the size cap recorded as Withheld with a reason rather than silently missing,
      so that history never pretends a file was not there.
27. As a developer, I want binary files stored as they are, so that a `~/Documents` Root full of `.docx` is insured
    like any other.
28. As a developer, I want symlinks stored as links by default, so that a Restore gives me back a link and not a
    copy.
29. As a developer, I want to opt specific symlinks into being followed, so that a deliberately linked-in tree is
    captured.
30. As a developer, I want FIFOs, sockets and devices Withheld rather than read, so that a capture can never block
    forever on a pipe.
31. As a developer on Windows, I want a junction restored as a junction and a real symlink refused clearly when the
    OS will not let us create one, so that nothing is silently written as the wrong kind of file.
32. As a developer using cloud storage, I want placeholders never hydrated, so that Keepster does not download my
    entire cloud drive on the first pass.
33. As a developer, I want full POSIX permission bits captured on Unix, so that an accidental `chmod` is something I
    can undo.
34. As a developer, I want empty directories captured, so that a Restore hands back a tree that still works.
35. As a developer, I want a file that is being written while read to be retried and, if it never settles, carried
    forward with the Snapshot marked incomplete, so that no torn version ever enters history.
36. As a developer, I want a partly unreadable tree to still produce a Snapshot, marked incomplete and naming what
    it omitted, so that one root-owned file does not freeze my history.
37. As a developer whose external disk is unmounted, I want the daemon to refuse to read an empty tree as a mass
    deletion, so that thirty days of "empty" captures cannot age my real content off the ladder.
38. As a developer with a Root on a network share, I want it accepted and marked `Degraded` where the watcher is
    genuinely blind, so that I know Reconciliation is doing all the work.

### Reading, browsing, searching

39. As a developer, I want a chronology of one path, so that I can see every version of the file I broke.
40. As a developer, I want to open a version and diff it against the previous one or against the Working Tree, so
    that I can see exactly what changed.
41. As a developer, I want diffs computed by the daemon, so that every client shows the same hunks and a five-megabyte
    file is not shipped to draw three lines.
42. As a developer, I want the timeline to stay legible at hundreds of Snapshots, so that "when did this break" is a
    navigable question.
43. As a developer, I want to filter the timeline to the Snapshots that touched one file, so that a file's own
    history is short.
44. As a developer, I want a Snapshot that only deleted or only added files to say so in words, so that I do not
    read an empty-looking diff as "nothing happened".
45. As a developer, I want to find a file by a fragment of its name across all my Roots, so that a file I deleted
    and cannot locate is still findable.
46. As a developer, I want the search result to tell me whether the path still exists in the newest Snapshot, so
    that "found" is distinguishable from "found, and already gone".
47. As a developer, I want to search the content of every version in a Root, so that I can find the code I lost when
    I cannot remember which file held it.
48. As a developer, I want content search results grouped by content version with the Snapshot range they were
    current for, so that a long-lived file does not produce a thousand identical rows.
49. As a developer, I want content search to tell me what it skipped — binaries, Withheld paths — so that an empty
    result cannot be confused with an unsearched one.
50. As a developer, I want content search to be cancellable and to stream, so that an expensive question does not
    look like a hang.
51. As a developer, I want to open a historical version in my editor, so that I can read it with my own tools.
52. As a developer, I want to export a version or a whole Snapshot into a directory I choose, so that I can look at
    an old state beside the current one without overwriting anything.
53. As a developer, I want a single file downloadable from the web UI, so that handing someone one old version is
    one gesture.

### Restoring

54. As a developer, I want to restore a file over the Working Tree atomically, so that a crash cannot leave me with
    a half-written file.
55. As a developer, I want a Safety Snapshot taken before every Restore, so that the restore itself is undoable.
56. As a developer, I want the whole Restore request checked before anything is written, so that I see the entire
    plan rather than discovering it half-applied.
57. As a developer, I want a dry run of a Restore, so that I can read exactly what would happen before it happens.
58. As a developer, I want structural surprises — a missing parent, a type change, a hardlinked file, an
    unrecoverable overwrite — to require explicit confirmation, so that nothing irreversible happens by accident.
59. As a developer, I want name collisions and names invalid on this platform to be resolved by me per path, so that
    restoring onto a case-insensitive filesystem does not silently pick a winner.
60. As a developer, I want Restore never to delete anything, so that a mode built on rules the tool cannot see can
    never erase what those rules hide.
61. As a developer, I want a per-path outcome from a Restore, so that I know exactly which files came back and which
    failed.
62. As a developer, I want restored files to carry the Snapshot's permission bits, or to keep the current ones,
    according to a choice I make, so that permissions are never changed behind my back.
63. As a developer, I want to be asked whether to recreate empty directories a Snapshot recorded, so that a
    restored tree matches what the tools expect.

### Retention, space and secrets

64. As a developer, I want history bounded by a ladder rather than growing forever, so that insurance has a
    predictable cost.
65. As a developer, I want the last 24 hours kept completely, so that the window the tool exists for is never
    thinned.
66. As a developer, I want the Retention Ladder expressed in durations I can change, so that I can trade disk for
    depth.
67. As a developer, I want to pin a Snapshot so that Retention can never remove it, so that a known-good state
    survives any policy.
68. As a developer, I want an optional per-Root disk cap that drops the oldest whole Segments, so that one Root
    cannot fill my disk.
69. As a developer, I want capture never to stop because of Retention or the cap, so that the tool does not go quiet
    exactly when the tree is churning hardest.
70. As a developer, I want space actually reclaimed rather than merely unreferenced, so that "Retention ran" and "I
    have space" mean the same thing.
71. As a developer, I want maintenance to wait for an idle moment and never overlap a capture, so that reclaiming
    space does not compete with insuring my work.
72. As a developer who leaked a secret into history, I want one command that removes every version of that path from
    the whole history, so that the leak is gone rather than hidden.
73. As a developer running that command, I want to choose explicitly whether the path is also excluded from now on,
    so that neither default silently does the wrong thing.
74. As a developer running that command, I want to be warned when the live file still matches what was removed, so
    that I know the next Snapshot would re-capture the secret.
75. As a developer, I want Forget never to touch my Working Tree, so that removing a secret from history does not
    edit my files.

### Configuration

76. As a developer, I want everything configurable in two TOML levels with clear precedence, so that per-machine and
    per-project choices do not fight.
77. As a developer, I want to see the effective configuration with, per key, the level that set it, so that a
    layered rule list is readable.
78. As a developer, I want configuration reloaded automatically, so that editing a file is the whole workflow.
79. As a developer, I want an invalid configuration never applied, with the Root marked `Degraded`, so that a typo
    cannot quietly widen what is captured.
80. As a developer, I want per-project settings expressible in my own global configuration, so that I need not put a
    Keepster file inside the watched directory at all.
81. As a developer cloning someone's repository, I want their file unable to choose which binary my daemon executes,
    where my history is stored, or what may enter it, so that a `git pull` cannot change my exposure.
82. As a developer, I want a key that cannot take effect live marked as restart-required and reported as such, so
    that an edit never appears to apply when it did not.

### Daemon, tray, platforms

83. As a developer, I want the daemon to start at login and be restarted if it dies, so that coverage does not
    depend on my remembering.
84. As a developer, I want installation to need no administrator rights on any of the three platforms, so that I can
    install it on a machine I do not own.
85. As a developer, I want autostart installed and verified by the tool itself, so that "running now but gone after
    reboot" is not a silent failure.
86. As a developer on macOS, I want the logout coverage gap stated plainly in status and in the docs, so that I am
    not told a guarantee that does not hold.
87. As a developer, I want exactly one daemon per user, with a second invocation refusing and naming the live one, so
    that two processes never write one store.
88. As a developer, I want the CLI to reach the daemon over a permission-protected socket, so that the boundary is
    the OS rather than a secret in a file.
89. As a developer who only uses the CLI, I want the loopback listener switchable off, so that no network surface
    exists on my machine.
90. As a developer, I want shutdown to finish the capture in flight and flush pending debounce windows, so that a
    reboot does not cost me my most recent edits.
91. As a developer, I want a tray icon showing live state, so that I can see at a glance that I am covered.
92. As a developer, I want to be notified when the insurance stops — a Root gone, capture halted, the daemon dead —
    and never for routine work, so that notifications remain worth reading.
93. As a developer, I want muting reported in status, so that a muted alarm cannot become a silent failure.
94. As a developer, I want the tray's own failures (an icon that cannot draw, a dead notification channel) reported
    as separate capability rows, so that a half-working tray is not read as a working one.

### Web UI

95. As a developer, I want the web UI served by the daemon on loopback with no runtime dependencies, so that
    browsing history needs nothing installed.
96. As a developer, I want the browser session established without a token in the address bar, so that my token does
    not land in history, `Referer` or `window.opener`.
97. As a developer, I want one History screen with three view modes — a Snapshot rail, a file-first tree and
    scrubber, an activity feed — so that I pick a question rather than a screen.
98. As a developer, I want my selected file and Snapshot carried when I switch modes, so that switching reads as
    continuity.
99. As a developer, I want Restore presented identically in all three modes, so that an irreversible action does not
    wear three faces.
100. As a developer, I want light and dark themes following the system by default, with a pin, so that the UI matches
     the rest of my desktop.
101. As a developer, I want my appearance preferences stored by the daemon, so that they survive clearing browser
     data and do not travel in a repository.
102. As a developer, I want the UI to update live as Snapshots happen, and to tell me honestly when it had to
     resynchronise, so that I never read a stale screen as a current one.
103. As a developer, I want a long-line file to open without syntax highlighting rather than freeze, so that a
     minified bundle is still viewable.

### Third parties, packaging, contributors

104. As a third-party UI author, I want a documented, additive-only HTTP contract with a generated OpenAPI document,
     so that I can build a client without reverse engineering.
105. As a third-party client author, I want machine-readable error codes with a stable meaning, so that I branch on
     codes rather than parsing prose.
106. As a third-party client author, I want to treat an unknown error code as its HTTP status, so that the code set
     can grow without breaking me.
107. As a scripter, I want a CLI exit-code table, so that a script can tell "no such Snapshot" from "the daemon is
     dead".
108. As a scripter, I want `status --json` to be exactly the API's status payload, so that there is one schema to
     learn.
109. As a person installing Keepster, I want an archive plus one install command on every platform, so that the
     mechanism is the same everywhere.
110. As a person on Debian or Fedora, I want a package that declares the external `git` prerequisite, so that a
     missing dependency is the package manager's problem and not mine.
111. As a person on macOS, I want a Homebrew formula from our own tap, so that the tray app is not quarantined.
112. As a packager for a community channel, I want a stable release tarball and a committed lockfile, so that
     packaging needs nothing from us.
113. As a person building from a plain checkout, I want `cargo build` never to require node, so that a source build
     works with a Rust toolchain alone.
114. As a maintainer, I want every release gate that can run mechanically to run on every change, so that the gates
     do not decay silently.
115. As a maintainer, I want the tests to say per platform what actually ran, what is impossible there and what was
     never run, so that a skip is never mistaken for coverage.
116. As a user filing a bug, I want one command that collects the daemon's state into a local file and shows me
     exactly what it collected, so that I can decide what to share.
117. As a user, I want to be certain nothing is ever uploaded, so that a diagnostics command is not a data-export
     command.
118. As a user, I want logs bounded in size and a raised log level that expires by itself, so that debugging cannot
     fill the disk the tool is protecting.

## Implementation Decisions

### Workspace and boundaries

- **Ten crates**, and Cargo — not review — enforces what each may not know: `keepster-core` (domain types, policy,
  Retention, Include Rules, the change-evidence rule, search matching), `keepster-api` (`/v1` DTOs, RFC 9457
  Problem, the error-code table, an optional default-off `openapi` feature), `keepster-store` (git2 + rusqlite; all
  git plumbing and the derived index), `keepster-watch` (`notify` → core events), `keepster-app` (synchronous
  use-cases over the concrete store), `keepster-service` (platform autostart), `keepster-ui` (embeds the built SPA),
  `keepster-daemon` (tokio + axum, wiring, two listeners, jobs, SSE), `keepster-cli` (clap + HTTP client),
  `keepster-tray` (a `/v1` client in the user's session), plus a dev-only `keepster-testkit`.
- **The core owns no ports** (ADR 0003): no traits, no I/O, no DI. Policy is
  `decide(state, event, now) -> (Vec<Action>, PolicyState)` over values — a shape two closed decisions force,
  because the load-bearing test is "given this sequence of events at these times, exactly these Snapshots happen",
  and because shutdown must flush debounce windows through the same function rather than a second code path.
- `core` depends on `std` plus `regex` — a pure, I/O-free dependency accepted so that content matching, the change
  rule and the glob matcher all stay testable as values.
- **`keepster-store` owns git and the derived index together**, because the index is safe only if the commit and ref
  update always precede the index write; sequencing those in `app` would put the ordering in policy code.
- **The CLI links neither git2 nor rusqlite nor core**, which turns "UI must not own persistence" into a build fact.
  CLI selectors (`latest`, `~N`, `2h`) are arithmetic over API types.
- **Nothing below `keepster-daemon` reads a clock** (ADR 0012). Every timestamp the store records derives from a
  passed `now`; `SystemTime::now` and `git2::Signature::now` are banned below the daemon by a lint.
- **A failed Root is a value, not a startup error**: the daemon holds a runtime per Root whose state may be
  `Watching`, `Degraded` or `Unavailable`, retried with backoff.

### History Store

- **Segmented chains** (ADR 0001): one ref per Segment, each starting from a parentless commit, Snapshots chained
  within it; leaf-only ref names; reflogs off so ref deletion can actually free objects; `core.filemode = false`;
  the `SnapshotId` and the Root id carried as commit trailers.
- **`SnapshotId` and `RootId` are ULIDs**, lexicographically ordered by time, and the public identity — commits may
  be rewritten without invalidating anything.
- **Segment Period** is a per-Segment property fixed at creation (hourly default); a store holding Segments of
  differing periods is a supported steady state, never a migration.
- **A Pin is the Snapshot re-created as a parentless commit** under its own deterministic ref (ADR 0002), so `gc`
  physically cannot collect it, it survives an index rebuild, it costs one commit object, and Retention, thinning
  and the disk cap stay pin-unaware.
- **`.keepster-meta`, one blob at the tree root, is the single home for per-path facts git cannot express** (ADRs
  0006, 0009): `m` (POSIX modes as deviations from the Root's modal defaults), `w` (Withheld, with reason and kind,
  and deliberately no size), `i` (incomplete, with `unstable` or the errno), `p` (a cloud placeholder's mtime), `j`
  (a Windows junction), `d` (Empty Directory). **The record alphabet is append-only**: an unknown record type is
  skipped *and reported*, and redefining an existing type is forbidden.
- **A Withheld path has no tree entry at all** (ADR 0009) — an absent entry says only "no content here", which is
  true, and the sidecar says why.
- **Four independent version numbers** (ADR 0010): the container version in the Root's registry file (the only one
  rewritten automatically), the content format in the Segment's root-commit trailer, the sidecar grammar in its
  header, and the index schema (whose every mismatch is a rebuild). **There is no migration of history**: new
  Segments are written in the new format, old ones stay readable, and the ladder retires them within about a month.
  A newer container makes the Root `Unavailable` (`store_too_new`).
- **Store location** defaults to a global per-user store; a Root may keep its store inside its own directory,
  settable only from the global configuration, with three stated obligations (exclude it from its own watch, add it
  to the directory's ignore rules, accept that `git clean -xdf` destroys it).
- **Metadata split by derivability**: the Registry is a small daemon-owned file per Root replaced atomically,
  because the watched path is the one fact git cannot reproduce; the `SnapshotId`, path-change and ordering indexes
  live in SQLite (bundled, WAL, daemon as sole writer) declared a derived cache that may be deleted and rebuilt.
  Git notes are rejected outright.

### Capture path

- **The baseline is re-seeded from the previous Snapshot's tree at every capture** (ADR 0004); the daemon holds no
  index between Snapshots. The vehicle is an owner-less `git2::Index` filled by `read_tree`, which populates the
  TREE cache so `write_tree` rewrites only changed ancestry.
- **Entries must be applied in lexicographic order**, and strict object validation stays off on an uncached index —
  both are performance-carrying invariants with measured multiples behind them.
- **An unreadable file's path enters a retry queue with backoff**, because its carried-forward entry is
  indistinguishable from "unchanged" and no second event is coming.
- **A capture meeting the maintenance lock neither blocks nor drops** — it coalesces into one Snapshot when the lock
  is released.
- **Volume identity gates capture** (ADR 0011): the Registry records the volume's persistent identity at
  registration and every capture verifies it. Where identity cannot be determined and a capture would delete every
  path, the capture is held and the Root goes `Degraded`. A remounted volume is followed automatically when
  identity and the in-volume path both match, with the no-overlap rule re-run. There is no fallback store: nowhere
  to write means no capture.
- **File kinds**: binaries stored as-is (binariness is a display flag); a default 100 MiB cap plus a per-pattern
  override, over-cap files Withheld `too_large`; symlinks stored as links with an opt-in follow list covered by
  periodic Reconciliation of targets rather than extra watches; FIFOs, sockets and devices Withheld `special_file`;
  `.git` and its aliases Withheld `reserved_name` and never descended into; unrepresentable Windows names Withheld
  `unrepresentable_path`; cloud placeholders never hydrated.
- **Modes**: all twelve POSIX bits on Unix, held in the sidecar as deviations from the Root's modal defaults; the
  executable bit also lives in the tree mode. Ownership, ACLs, xattrs and Windows security descriptors are out,
  because the daemon is never elevated and could not restore them. A permission-only change is a real change:
  Reconciliation takes the mode from the `stat` it performs anyway, and the watcher `stat`s on an attribute event.
- **Empty Directories are captured by default** as sidecar records, every level of a chain carrying its own mode;
  detection costs no system call, being a prefix comparison over the sorted index.
- **Paths travel as bytes**: tree names are raw bytes, `path=` carries percent-encoded bytes, JSON carries a lossy
  UTF-8 `path` plus `path_bytes` whenever the name is not UTF-8.
- **Nothing torn reaches history**: files are opened `O_RDONLY|O_NOFOLLOW|O_NONBLOCK`, read only if `fstat` says
  regular, and `stat` is compared before and after with retries.

### Triggers, watching and Reconciliation

- **The watcher is a latency optimisation, never the correctness mechanism.** Every platform loses events; several
  losses are never announced (a populated directory moved in, a write through another hardlink, `mmap` writes,
  network mounts, a watcher that stops silently), and an atomic save arrives with no modify event on the saved path.
- **Include Rules are applied before watches are installed**, and **an excluded directory is never entered**
  (ADR 0007): pruning wins over negation, git's rule, adopted because a pruned directory can produce no events for
  a re-included file to arrive on. Git's silence is not adopted with it — the explain endpoint names the pruning
  ancestor, and configuration load warns for every negation an ancestor makes unreachable.
- **Reconciliation is configurable and on by default**; disabling it drops only the startup walk, and the response
  to an OS rescan signal is never optional. A Root with the startup walk off is reported as "coverage not
  guaranteed".
- **Reconciliation's evidence of change is a filter, not a verdict** (ADR 0014). The verdict is always the OID.
  Compared are size and mtime **by inequality** plus the mode from the same `stat`; inode, device and ctime are
  excluded (ctime means creation time on Windows). Inputs live in a stat-cache table in the derived index, derived
  from *observation* rather than from git, so losing it costs exactly one full hashing pass. Git's racy rule
  applies: an entry whose mtime is not strictly older than the pass's start is re-hashed next pass. Where mtime
  cannot be trusted, a `reconcile.verify = "hash"` key and a one-off flag exist, with the weakness reported in
  status; escalation is never automatic, because on a network Root it would mean reading the whole Root hourly.
- **Five occasions share one rule and differ only in the area walked**: daemon start, an OS rescan signal (the named
  subtree where the OS names one, the whole Root where it does not), the periodic pass, a configuration reload or
  Registry change, and a remount.
- **A daemon-wide watch ceiling** of half the per-user watch limit, read at startup; exceeding it moves the next
  Root onto periodic Reconciliation with the reason in status.

### Retention, maintenance, Forget

- **Ladder in durations**: everything for 24 h, hourly to 7 d, daily to 30 d, then discarded. The newest Tier is
  bounded by age alone. This is also the shipped default:

  ```toml
  [[retention.tier]]
  age  = "24h"
  keep = "all"

  [[retention.tier]]
  age  = "7d"
  keep = "hourly"

  [[retention.tier]]
  age  = "30d"
  keep = "daily"
  ```

- **The survivor of a thinned period is its last Snapshot**, and **Retention only ever acts on a Segment lying
  wholly past a Tier boundary**, which makes thinning happen at most once per Segment and the pass idempotent.
- **An opt-in per-Root disk cap** drops whole Segments oldest-first *after* maintenance, because deleting a ref frees
  nothing until space is reclaimed; it floors at the newest Tier, overshoots and reports rather than blocking
  capture.
- **An exclusive per-store lock** is held by a capture or a Maintenance Pass and never both; reads stay unlocked.
  Retention is checked once per Segment rollover, the Maintenance Pass runs at most daily or immediately at the cap,
  both deferred to an idle window with a hard ceiling.
- **Forget rewrites every Segment and pin containing a path**, preserves every `SnapshotId`, never touches the
  Working Tree, takes no Safety Snapshot, and requires an explicit choice between excluding the path afterwards and
  continuing to watch it — warning when the live file still matches what was removed. It is affordable precisely
  because the ladder bounds the store to roughly 1,600 commits.

### Configuration

- **Two TOML levels**: a global daemon configuration and an optional `.keepster.toml` in the watched directory.
  Precedence: `default`, `global`, `global-pattern`, `global-root`, `root` — which is also the provenance enum on
  the wire.
- **The global configuration carries per-Root blocks** addressed by Root id, and pattern blocks addressed by a path
  glob, both accepting the whole schema. An id block outranks a pattern block; among pattern blocks the last match
  wins. **This makes `.keepster.toml` optional** — it is the way to share rules through a repository, not the way to
  configure a Root.
- **Three classes of key can never come from the Root file**: keys describing the daemon (the HTTP port, the
  loopback listener, log format and ceiling), keys that make the daemon execute something (the editor template), and
  keys deciding where the user's data goes or what may enter it (the store location, the secret opt-in list). A Root
  file setting one is ignored with a warning, not made `Degraded`.
- **Merging is by key class**: pattern lists (include, exclude, secret opt-in, size-by-pattern, follow list) layer
  with negation and last-match-wins; sets and scalars are replaced wholesale.
- **Reload is automatic**; keys that cannot take effect live are marked restart-required and reported as such. **An
  invalid configuration is never applied** — the last known-good stays in force and the Root is `Degraded`; with no
  known-good version the Root is not watched at all.
- **Defaults**: watcher with a ~3 s debounce; one Snapshot per 60 s; Reconciliation at start and on every rescan
  signal; periodic floor off; ignore files not honoured; secrets refused; empty directories captured.
- A configuration reload touching classification inputs, and a Registry change, queue the Root for Reconciliation.

### HTTP API and CLI

- **`/v1` is a public contract**: additive-only inside `/v1`, breaking changes only via `/v2`, and **the embedded
  SPA gets no privileged endpoints**. The CLI is a thin client and is stable too — subcommands, flags, exit codes
  and `--json` shapes.
- **Addressing**: a Root is a ULID in the URL segment and nothing else; resolving a path is its own query.
  Selectors are CLI conveniences reducing to timeline queries; `latest` never appears in a URL.
- **One path parameter form** across every endpoint: `path=`, Root-relative, `/`-separated, with `..`, leading `./`
  and absolute paths rejected.
- **Surface**: Root registry (list, resolve by path, register, deregister with optional purge, move); timeline as
  one cursor-paginated endpoint that also answers "which Snapshots changed this path"; `entries` with `depth`;
  `content` as raw bytes with `Content-Length`, `ETag` and `Range`; daemon-computed `diff` with
  `to=<SnapshotId>|working` and `format=json|unified`; Restore; export to a daemon-written destination plus a
  streamed archive endpoint for the browser download gesture; a filesystem browse endpoint for picking that
  destination; Capture Preview as streamed NDJSON; path search (synchronous, all Roots by default) and content
  search (streamed, one Root by default, all Roots on request); pins; Forget and Maintenance as jobs with a job
  resource; `status`; read-only configuration with per-key provenance plus `explain?path=`; open-in-editor; UI
  preferences; events; version; and the generated OpenAPI document served before authentication.
- **Errors are RFC 9457 Problem Details plus a stable `code`**, with HTTP status as the coarse layer and machine-
  readable details so the CLI composes its own message. **The code set is open** (ADR 0005): it is rendered as a
  string with a generated description, a client must treat an unknown code as its HTTP status, and the CLI maps
  codes to exit codes through an exhaustive match with no wildcard.
- **Exit codes are a table**: success, usage error, daemon unreachable, not found, state conflict, partial success,
  incompatible version, a catch-all, and a dedicated code for "some Root is not `Watching`".
- **Authentication**: the socket relies on OS permissions; the loopback listener mints a per-start token in a `0600`
  file, exchanged once through a single-use bootstrap code for an `HttpOnly`, `SameSite=Strict` cookie, with `Host`
  and `Origin` checked on every request and no wildcard CORS. With the listener off the token path disappears
  entirely.
- **Live updates are SSE**, with ULID event ids, `Last-Event-ID` replay from a bounded ring buffer and an honest
  `resync` when the requested id has been evicted. Payloads are ids and flags, never content.
- **`status --json` is a verbatim passthrough** of the status payload, so there is one schema; the human-readable
  rendering is the CLI's own.
- **The OpenAPI document is generated from the API crate's types** and committed at the repo root, with a test
  comparing it byte for byte and a route macro making the axum route and the documented path one literal.
  "Additive-only" is enforced against the **last released tag**, not the PR base.
- **Jobs versus synchronous**: pins are synchronous; Forget and Maintenance are jobs, because an irreversible
  operation whose connection drops must still have an identity to query.
- **Open-in-editor** launches the configured template split into argv with no shell, confined to the Root after
  canonicalisation, spawned detached, rate-limited, and able to materialise a historical version into a `0600`
  read-only temp file.

### Web UI

- **Svelte 5, Vite, TypeScript, pnpm**, with the built SPA embedded in the binary behind a default-on feature and
  the built output committed so `cargo build` never needs node.
- **CodeMirror 6 as the diff editor**, fed the daemon's hunks through its override hook so the merge view's own diff
  is never used; line wrapping opt-in; a file with a line over a configurable threshold opens without highlighting.
- **One History screen with three view modes** — a Snapshot rail (default), file-first tree and scrubber, activity
  feed — carrying the selected file and Snapshot across switches, with sticky day headings, a per-file filter, and a
  "hide routine Snapshots" filter. **Restore's presentation is shared by all three modes**: a mode decides
  navigation, not destructive affordances.
- **Search is an entrance, not a fourth mode**: a header field with an overlay of results that resolves to the
  (file, Snapshot) pair the modes already carry; a result in another Root switches the Root visibly.
- **Two themes**, defaulting to the system preference with a pin, owning the diff colours and the editor palette;
  three daemon-side preference keys (theme, syntax line limit, wrap) reached through a public preferences endpoint,
  with browser storage used only as a paint cache.
- **Budgets**: eager ≤ 150 KiB gzip, total ≤ 400 KiB gzip, held by a size check; the browser floor is Baseline
  "widely available"; accessibility commits to keyboard operability and a visible focus ring and claims no WCAG
  level.

### Tray and notifications

- **The tray ships on all three platforms as a separate process** (ADR 0008) and is **the only carrier of desktop
  notifications**: on Windows a per-user background process is the documented remedy for a non-interactive window
  station, and on macOS notifications need a bundle identifier, so the tray is an app bundle.
- **A view, never a source**: everything it shows is derivable from the status payload and the event stream, and no
  notification carries a tray-only action. It is an ordinary `/v1` client over the CLI's socket.
- **Alarms fire only when the insurance stops** — `Unavailable`, `Degraded`, capture halted at the cap, an
  unexpected daemon death, a failed user-initiated job — on transition, coalesced, with recovery replacing rather
  than adding. Muting is reported by status. Icon and notifications are two capability rows that fail
  independently, neither affecting the exit code.

### Lifecycle, packaging, prerequisite

- **One daemon per OS user, never elevated**, autostarted by a unit the tool writes and can verify: a systemd user
  unit with linger on Linux, a LaunchAgent on macOS (whose logout gap is shown rather than hidden), and a **Scheduled
  Task registered at logon on Windows** — a per-user service does not exist in any documented form and would have
  had the same logout gap anyway. **Installation needs administrator rights on none of the three platforms.**
- **Only the service manager starts the daemon**; a flock'd pid file is the single-instance authority, so a running
  daemon is always a supervised one. Shutdown drains the in-flight capture, flushes pending debounce windows and
  survives being killed past the stop timeout.
- **Packaging is tiered over one universal mechanism** — an archive plus an install command, every package format a
  wrapper over it. Tier 1: archives with shell and PowerShell installers plus a Homebrew formula in our own tap.
  Tier 2: deb and rpm, the only formats that can declare the `git` prerequisite. Flatpak, Snap and AppImage are
  ruled out on blocking facts; there is no MSI. **One package carries both binaries**, and **no install or uninstall
  path ever deletes history** — erasing is only ever an explicit command.
- **The external `git` is a maintenance-only prerequisite**, and its absence degrades instead of blocking: capture
  runs on in-process git2, a missing or too-old git marks maintenance unavailable with a reason in status, and the
  one real consequence — Retention freeing nothing — is bounded by pausing capture at the disk cap. Discovery never
  reads an inherited `PATH`; the minimum version is 2.32.0; every invocation is hermetic (no shell, explicit git
  dir, system and global config disabled, whitelisted environment); `--force` and the bundled maintenance command
  are forbidden. Keepster prints the platform's install command and never runs a package manager.
- **Versions are independent by explicit statement**: the product version is the only one a user cites, and a
  product major implies nothing about the API or the store format. Status prints build, API and store-format rows;
  the git hash is recorded in full and no build timestamp is captured.

### Observability

- **One log mechanism on all three platforms**: our own rotated file in a per-user directory, because the native
  sinks lose either the content (the unified log does not persist info) or the readability (the Windows event log
  without an admin-registered message file). On Linux the journal still receives startup and fatal lines.
- **Volume is bounded by size-triggered rotation plus a file count**, with a declared ceiling; **a raised log level
  expires by itself** and shows as a status row, because a cap protects the disk while expiry protects the content.
  The carrier is a hand-written writer over the tracing facade, justified because no maintained crate offers the
  narrow shape without importing a second configuration surface.
- **The tray logs to its own file**, since its most valuable line is "the daemon died" — exactly when the API is
  gone.
- **A diagnostics command writes a local archive with fixed content and prints an inventory first** (ADR 0013):
  status, the three versions, the environment, the effective configuration with provenance, the Registry, per-Root
  store topology, the index watermark, and log tails. **Paths go in as they are** — redaction was rejected as a
  promise that cannot be kept — while **no bytes of any file from history ever do**, and **nothing is ever sent
  anywhere**. The daemon collects it; an unreachable daemon yields a degraded archive whose inventory names the
  missing sections.
- **Log lines are human-readable with fields by default**, switchable to JSON by a global key, one file either way.

### Performance budget and ceilings

- **Two layers**: an outcome budget stating what the product promises, reviewed at release and published per
  platform with macOS and Windows marked unverified; and an operation tripwire that promises nothing and fails a
  bench target. Outcome lines: capture ≤ 5 s p95 at ceiling as the daemon's own contribution (never the configured
  debounce plus rate limit), fetch a version ≤ 100 ms, diff ≤ 250 ms for a file ≤ 1 MiB, a path's chronology
  ≤ 100 ms, startup Reconciliation ≤ 30 s warm (cold is disk-bound and explicitly not claimed), Preview first row
  ≤ 200 ms then ≥ 20,000 paths/s, idle ≤ 0.5 % CPU and ≤ 150 MiB RSS with three Roots at ceiling, capture peak
  ≤ idle + 40 MiB, and store size ≤ 2× the Root's included content.
- **v1 claims 100,000 included files, 10,000 included directories and 10 GiB of included content per Root**, and
  **nothing refuses**: registration above it runs a Capture Preview and asks; watch exhaustion falls back to
  periodic Reconciliation with `Degraded`.
- **A tripwire line exists only where it protects a named invariant or mirrors an outcome line.** Thresholds carry
  2× headroom and are committed; the headroom is sized to the multiples this design's real failures produce, not to
  laptop noise. Every recorded number carries its environment, and the fixture never lives on a memory-backed
  filesystem.
- Content search and Capture Preview are **explicitly outside** the read budget: their input is unbounded by
  design.

## Testing Decisions

**What makes a good test here**: it asserts observable behaviour at the highest seam that can see it, it is
deterministic by construction rather than by retry, and it never asserts an absence that only a timeout can prove.
The suite **gates what we promise and reports what it merely observes** about the environment — the rule that lets
the gated layer carry no retry budget at all. There are **no test doubles**: no clock trait, no filesystem trait;
the only double in the workspace is a real git store in a temporary directory.

**Five seams, highest first.**

1. **The `/v1` contract, through an in-process daemon harness.** The real daemon on an ephemeral loopback port and a
   temporary socket or pipe, over real stores, driven by a real HTTP client and by the real CLI pointed at the same
   socket — which is how "the CLI is a thin client" and "status --json is a verbatim passthrough" become tests. It
   must be a real server rather than an in-process service call, because cookie round-trips and event streaming are
   HTTP-level behaviour. Everything user-visible is tested here: the no-overlap rule, cursor pagination, entries,
   ranged content reads, diffs, Restore's whole-request preflight and its confirmations and resolutions, jobs, pins,
   status, configuration provenance and explain, Preview, export, both searches, event replay and `resync`,
   preferences, and the served OpenAPI document.
2. **`keepster-app` use-cases over a real git store in a temporary directory** — only for facts the harness cannot
   assert cheaply because they are physical: that space is actually reclaimed after a Segment ref is deleted, that
   deleting a ref frees nothing while reflogs are on, that rebuilding the index from git objects reproduces the same
   rows, that a pinned Snapshot survives collection, that a thinned Segment is genuinely re-chained, that the
   baseline reuses the tree cache, and that maintenance-unavailable is raised when the configured git is missing.
   A month of history is produced **through the real capture path at injected times** — never by fabricating refs
   and backdated commits, which would make the rig a second implementation of the store's invariants.
3. **`keepster-core` as values, with no disk**: the trigger machine as a fold over (event, time) pairs, the
   Retention Ladder and its boundary invariant, the layered include/exclude matcher with negation and pruning, the
   change-evidence rule including the racy window and the inequality rule, and search matching.
4. **Vitest on the SPA's non-visual logic**, kept in plain modules: event-stream reconnection, the hunk feed into
   the diff editor, and Restore's preflight. No component tests and no snapshot tests.
5. **One browser smoke test at release**: bootstrap code to cookie, the screen boots from embedded assets, pick a
   Snapshot, open a diff, see the preflight dialog. It runs in the release workflow on the tag, not in the merge
   gate, because a browser update can fail it on unchanged code.

**Reconciliation's guarantee is tested with the watcher switched off**: a real tree mutation — including the three
losses no platform announces, plus renames, deletions and a permission-only change — then a reconcile pass, then an
exact comparison of the resulting Snapshot's contents. Watcher tests assert **presence only**, within a generous
timeout; what each platform actually delivers is a manual conformance report that gates nothing.

**Fixtures** live in a dev-only testkit crate with three layers: shared plumbing (temporary stores, the real-git
probe, the history builder that plays `(now, mutation)` streams through the real use-cases); the seeded bulk
generator, whose parameters sit in a **frozen** profile guarded by a digest test so a fixture's shape cannot move
without the performance thresholds moving in the same commit; and a freely growing catalogue of degenerate cases —
a near-limit path, a non-UTF-8 name, symlinks, empty directories, a file past the cap.

**The platform matrix is declared, not inferred.** Every (case, platform) pair is `ran`, `impossible` (compiled out
with a recorded reason, so the decision is made at compile time) or `unavailable` (a capability probe failed, with
its reason). The table is generated from the catalogue and published per platform; a printed runtime skip is
forbidden, because a month later "never run on Windows" would be indistinguishable from "green on Windows".

**Flake policy: no retries.** Half the racy code here is the product itself — the store lock, capture coalescing,
event resync — so a retry would paint green exactly the class of failure a user experiences as a lost update.
Anything in the gate that depends on time waits on an **observable state transition**, never on a duration, which
makes testability a requirement on the daemon's own reporting. Tests that install watches run in a serial group,
because the per-user watch limit is a machine resource, and a missing external git is `unavailable`, not a failure.

**Coverage** is a report answering one question — which crate has no test at all — never a threshold.

**CI**: GitHub Actions, with the rule that a check gates the merge if and only if its verdict is a property of the
diff rather than of the machine. Merge gate: format, lints, tests on three platforms, the API compatibility check,
the UI size check, the built-SPA freshness rebuild, licence and bans checks, and an MSRV leg. Release: the bench
tripwire on the reference machine, the smoke test, the platform table with no gaps, the outcome-budget review.
Scheduled: the advisory database check.

## Out of Scope

- **Encryption of the History Store at rest.** At-rest protection is the operating system's job, plus restrictive
  permissions on the store.
- **Any remote**: pushing, syncing or backing up history off the machine — and therefore any network egress at all,
  including from the diagnostics command.
- **Being a backup tool.** Durability against losing the machine is a different product with opposite trade-offs.
- **A Zed or editor extension.** The design's point is to sit below the editor.
- **Editing files inside the web UI.** It reads history and restores.
- **Any interaction with a watched directory's own `.git`** — its index, HEAD, branches or stash.
- **Mounting a Snapshot or a Root's history as a filesystem.** Post-release: the mechanism differs on all three
  platforms and none is cheap, and v1's need is served by export.
- **A TUI client.** Post-release: the CLI is v1's answer for a machine without a browser.
- **A full-text index for content search.** Rebuilding one would make the derived-cache promise false rather than
  merely slow; adding it later is additive.
- **Cross-Root content search as the default** (it exists only on explicit request), and **no global cross-Root
  timeline feed**.
- **Writing configuration through the API.** Round-tripping a hand-edited, git-tracked TOML is real work for a file
  edited rarely; it can be added additively.
- **A "make the Working Tree identical to the Snapshot" Restore mode**, whose worst case is irreversible precisely
  in the region the tool cannot see.
- **Ownership, ACLs, extended attributes and Windows security descriptors** in capture and Restore.
- **Automatic migration of history between store formats**, and any store-format downgrade blocker.
- **Component tests, visual snapshot tests and a full browser E2E suite**; also `beta`/`nightly` toolchain legs and
  a coverage threshold.
- **Proactive notification channels beyond the tray**, and any notification the daemon itself delivers.
- **macOS distribution beyond a Homebrew formula** (Developer ID and notarisation are the precondition of any other
  channel), and **Flatpak, Snap, AppImage and MSI** packaging.

## Further Notes

- **No production code exists yet.** The repository holds the map, its 35 resolved tickets, 14 ADRs, `CONTEXT.md`
  and a throwaway SPA prototype on its own branch; there is not one `Cargo.toml`. Everything above is a decision,
  not a description.
- **The ADRs are load-bearing and should be read before touching their areas**: segmented chains; pins as
  parentless commits; no ports in core; the baseline from the previous tree; error codes as an open set; modes in
  the sidecar; pruning over negation; the tray as a separate process; a Withheld path having no tree entry; no store
  migration; volume identity gating capture; no clock below the daemon; the bundle disclosing by inventory; and the
  stat cache being derived from observation.
- **One earlier invariant is amended** and the amendment must not be "fixed" back: the derived index remains
  deletable, but its stat-cache table is derived from observation of the Working Tree rather than from git objects,
  so the rebuild story has two halves and losing the cache costs one full hashing pass.
- **Where the code lives**: the canonical remote is Bitbucket and CI runs on GitHub, so release tags must reach the
  GitHub copy or the API compatibility gate compares against the wrong baseline, and nothing mechanically blocks a
  merge on the canonical side.
- **Four checks decay silently by construction** and are written down for that reason: the bench tripwire, the
  browser smoke test, the outcome-budget review and the platform table are release-time, not merge-time.
- **The performance numbers are predictions, not measurements**, except where a ticket records a measured figure
  with its environment; the first job of the tripwire layer is a baseline for the tree-read path, which has never
  been measured for this project.
- **Sequencing suggestion, not a decision**: the store and its invariants, then capture and the policy kernel, then
  the API and CLI, then Reconciliation's full evidence rule, then Retention and maintenance, then the SPA, then the
  tray and packaging. Every layer above the store is testable at the harness seam as soon as the daemon serves
  anything.
