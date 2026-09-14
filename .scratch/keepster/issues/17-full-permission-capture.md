# 17 — Full permission capture and restore

Type: grilling
Status: resolved
Blocked by: 10

## Question

Ticket 10 settled that a Snapshot keeps only the executable bit, as the git tree entry mode (`100755` / `100644`),
and that a Restore over an existing file carries the replaced file's permission bits across and applies the
Snapshot's executable bit on top. Two consequences were accepted for v1 and flagged for a separate decision:

- a permission-only change other than `x` (`chmod go-r key`, an agent's `chmod -R 777 .`) changes no tree, so it
  produces no Snapshot and is invisible in history;
- a Restore cannot put back the permissions a file had when the Snapshot was taken, only its executable bit, so an
  accidental `chmod 644` on a private key cannot be undone through Keepster.

Should Keepster capture and restore permissions in full, and if so, how?

Settle:

- whether the full permission bits are captured out of band — a commit trailer, a side blob per Snapshot, or
  another carrier — and what Forget, Segment thinning and Pins (ADR 0002) must do to carry that data along;
- whether ownership, ACLs (POSIX ACLs, macOS ACLs, Windows DACLs) and extended attributes are in or out, per
  platform;
- whether a permission-only change is a change that warrants a Snapshot, given the watcher reports it only as an
  attribute event and Reconciliation would then have to compare permissions as well as content;
- where a Restore takes permissions from when both the Snapshot and the replaced file carry them, and what happens
  on Windows, which has no POSIX bits;
- how the Safety Snapshot preserves the permissions a Restore is about to overwrite;
- the interaction with [12 — Snapshot write path](12-snapshot-write-path.md), whose index entries carry the mode
  that reaches history.

## Answer

### Scope: mode bits, and nothing else

A Snapshot records the **full POSIX mode** — all twelve bits, `0777` plus setuid, setgid and sticky — for every
path the tree holds. Ownership, POSIX and NFSv4 ACLs, extended attributes and Windows security descriptors are
**out of scope**, and not because they are hard:

- **Ownership is unrestorable by construction.** Changing a file's owner needs `CAP_CHOWN` on Unix and
  `SE_RESTORE_NAME` / `SE_TAKE_OWNERSHIP_NAME` on Windows; ticket 05 fixes one daemon per OS user, running as that
  user and **never elevated**. An unprivileged process may only `chgrp` into a group it already belongs to.
  Recording a fact the tool can never act on is noise in the UI, not insurance.
- **ACLs cost a system dependency.** Both maintained crates link `libacl` at build and run time (`exacl` 0.13.0,
  Linux + macOS NFSv4; `posix-acl` 1.2.0, Linux only), which contradicts the single-artifact, no-runtime-dependency
  stance the map takes for the web UI and the store alike, and breaks a static build. On Windows there is no
  maintained wrapper at all — `windows-acl` was last released in 2021 — so it would mean our own `unsafe` layer over
  `GetNamedSecurityInfoW` / SDDL.
- **The tool's own framing.** Recording every ACE and xattr is what a backup tool does; this one insures content,
  and ADR 0001's read-optimised store is the same choice made once already.

Symlinks get no mode: Linux ignores a link's permission bits and has no `lchmod`, so the bits would be
unrestorable on the platform that matters most. Empty directories stay invisible — git cannot represent them, and
making the sidecar their carrier is a different feature (see the map's *Not yet specified*).

Git's own reason for rejecting full modes — trees becoming umask-dependent across contributors — does not apply
here: a History Store is local, is never cloned, never merged and never shared, so there is no second umask to
disagree with.

### The carrier: a metadata sidecar inside the Snapshot tree

The modes live in **one blob, `.keepster-meta`, at the root of the Snapshot's tree**. Three carriers were
excluded before the choice:

- **The SQLite index** — ticket 03 declares it a wholly derived cache that may be deleted and rebuilt from git
  objects at any time, so modes stored there evaporate on a rebuild. This is ADR 0002's argument against a pin flag,
  applied again.
- **A blob referenced only from a commit trailer** — physically impossible. A commit references its tree and its
  parents and nothing else, so such a blob is unreachable and `gc --prune=now` (required by ticket 06) collects it.
- **The trailer text itself** — ticket 23 already drew the line: fine for a handful of paths, wrong for thousands.
  `chmod -R 777 .` is exactly the case this ticket exists for, and it would put a hundred thousand lines into a
  commit message, where the trailer block's 25% rule is fragile besides.

A per-directory sidecar (bup's `.bupm`) was rejected too: it buys locality that only enormous Roots need, and
charges a service entry in every directory of every tree, to be filtered out of `entries`, diffs, Restore and
Forget at every level.

**It is a metadata sidecar, not a mode file.** The first line carries a format version and the Root's prevailing
defaults; each following line is a typed record. This ticket defines exactly one record type — the mode — and
[23 — How a Withheld path is represented in the git tree](23-withheld-tree-representation.md), which needs a
carrier of the same shape, is now blocked on this one and may add its own type rather than a second reserved name:

```
keepster-meta 1 file=0644 dir=0755
m 0600 .ssh/id_rsa
m 0700 bin
```

- **Records hold only deviations from the header defaults**, so a uniform Root's body is empty and the file grows
  with anomalies — a `0600` key, a `0700` directory, a sticky directory — not with the Root.
- **The defaults are the modal file and directory modes**, recomputed only during Reconciliation, which walks
  everything anyway; every other capture carries the header forward. Fixed `0644`/`0755` defaults were rejected
  because a Root under umask 002 (`0664` files) would make every file an exception, i.e. a full mode table
  rewritten on every capture.
- **Lines are sorted by path bytes**, and a path that is not UTF-8 is C-quoted as git quotes such names (ticket 10:
  names are bytes). Determinism is not cosmetic: unchanged modes must produce byte-identical content, hence the same
  OID, so the entry is carried forward from the baseline untouched and ticket 12's `write_tree` short-circuit
  survives.
- **The sidecar is part of the baseline** and obeys ticket 12's rules like any other entry — lexicographic
  insertion order, blob written before the tree.
- **A user's own file named `.keepster-meta` in the Root's top level is Withheld `reserved_name`**, the mechanism
  ticket 10 already defines for `.git`. Nothing appears in the Working Tree: the sidecar exists only as a git object
  inside the History Store, and `entries`, diffs and Restore never expose it.
- **Pins are free**: ADR 0002 re-creates a pinned Snapshot as a parentless commit *sharing the original's tree*, so
  the sidecar travels with it at no cost. Thinning needs no changes at all. **`forget` rewrites the sidecar** in the
  same pass that already rebuilds every affected tree and pin, dropping the forgotten paths' records.

### Capture: a permission-only change is a change

A change of mode alone now changes the sidecar, hence the tree, hence produces a Snapshot whose diff is
permissions only. Both capture paths agree on this, because it is not a Trigger in ticket 04's sense — which
decides *when* a Snapshot is taken — but the definition of what counts as a change, and that must be one definition:

- **Reconciliation compares the mode**, at no extra cost: it already `stat`s every file (ADR 0004 puts hardlink
  grouping by `(dev, ino)` there), and `st_mode` arrives in the same call. No platform offers a way to learn a mode
  without a `stat`.
- **The watcher `stat`s the path on an attribute event.** It cannot do better: `notify` collapses inotify's
  `IN_ATTRIB` — one bit for chmod, chown, xattr, timestamps and link count alike — into
  `Modify(Metadata(Any))`, and flattens `FILE_NOTIFY_CHANGE_ATTRIBUTES` and `_SECURITY` into a plain `Modify(Any)`
  on Windows; `MetadataKind::Permissions` is constructed by no backend. Only macOS distinguishes `Ownership` and
  `Extended`, and a plain `chmod` arrives there as `Any` too.
- **False positives cost nothing.** Research 02 warns that `std::fs::copy` on APFS can raise a metadata event on the
  *source* file; if neither content nor mode differs, no entry is mutated, `write_tree` short-circuits and no
  Snapshot exists to explain.
- **Windows reads and applies no modes.** The disk is never probed, the sidecar is carried forward untouched, and
  ticket 10's carry-forward rule for the executable bit is unchanged.

### Restore: asked by default, answered by one scalar

Ticket 10's rule — the temp file inherits the *replaced* file's bits and only `x` comes from the Snapshot — existed
because the mode was nowhere in history, so the only way not to lose it was not to touch it. That reason is gone,
and the mode is now part of what a Restore restores:

- **Divergent modes are a per-request resolution, not a confirmation.** Both outcomes are legitimate, so this is a
  choice in ticket 10's vocabulary, and `force` never resolves choices. Preflight returns `confirmation_required`
  naming the diverging paths; the request answers with the scalar `modes: "snapshot" | "keep"`, optionally
  overridden per path in `resolutions`. CLI: `--modes=snapshot|keep` and `--modes <path>=snapshot|keep`. On a TTY it
  asks; with `--no-input` and no answer it exits 5, as ticket 10 defines.
- **`--force` deliberately does not apply modes.** It means "yes, break structure" and is typed reflexively in
  scripts; letting it apply modes silently would reinstate "the Snapshot always wins" through the back door.
- **The mode is applied to the temp file before the rename.** A `chmod` that fails — setgid into a group the user
  does not belong to, for instance — discards the temp file and reports that path `failed`, having touched nothing.
- **A newly created file takes the Snapshot's mode**, not the umask.
- **`unchanged` now requires both** content and mode to match.
- **The Safety Snapshot needs no new mechanism**: capture records modes, so the Snapshot taken immediately before a
  Restore holds the bits about to be overwritten, and one `SnapshotId` still undoes one operation. The exception is
  ticket 10's `overwrite_unrecoverable` — where the current version cannot enter the Safety Snapshot, its mode is
  lost with it, and preflight already demands confirmation.

### Configuration and contract

- **`restore.modes = "ask" | "snapshot" | "keep"`**, default `ask`, an ordinary key at both of ticket 04's levels
  with provenance (`default / global / global-pattern / global-root / root`, extended by ticket 04's Comments),
  answered by `explain?path=` like any other. The UI does not write it —
  the config is read-only (ticket 07), and the Restore dialog shows the value and its source. The accepted cost: a
  `.keepster.toml` committed to a project travels to other clones, so a colleague's Restore can apply modes without
  asking — reversible and local, but silent.
- **`entries.mode` keeps its meaning**, the tree entry mode (`100644`, `100755`, `120000`, `040000`), because it
  describes the git object. The full mode is a **separate field, present only where modes were actually observed**,
  as four octal digits; a Root captured on Windows has no such field. Folding both into one field would have forced
  a fabricated `0644` on Windows or an optional field anyway. The addition is additive to `/v1`, which ticket 14
  measures against the last released tag.

### What this refines

- **Ticket 10**: "a Restore over an existing file keeps its permissions" and "accepted loss" are superseded by the
  `modes` scalar and by full capture; the size cap, Withheld and the rest stand.
- **Ticket 12 / ADR 0004**: the sidecar is one more baseline entry, subject to lexicographic order, and the
  short-circuit is preserved precisely because it is untouched when nothing changed.
- **Ticket 06**: `forget` gains the sidecar to rewrite, in the pass it already runs.
- **Ticket 23**: now blocked on this ticket, and free to add a record type instead of a second carrier.
- **Ticket 19 (export)**: the mode is now available to export; whether an exported file carries it is still that
  ticket's question.
