# 23 — How a Withheld path is represented in the git tree

Type: grilling
Status: resolved
Blocked by: 17

## Question

Ticket 10 introduced **Withheld** — a path a Snapshot records as present in the Working Tree but whose content it
deliberately does not hold — together with its reason vocabulary (`too_large`, `special_file`, `reserved_name`,
`unrepresentable_path`, `other_root`, `history_store`, `cloud_placeholder`), the boundary against `incomplete`, its
visibility in `entries` and the `withheld` Restore refusal. It never settled what is physically stored.

The choice is already narrowed by decisions elsewhere. The SQLite index is a wholly derived cache that may be
deleted and rebuilt from git objects at any time (ticket 03), so a reason stored only there evaporates on a rebuild
— the same argument ADR 0002 used to reject a pin flag. Git notes are rejected outright (ticket 03). What remains
is the tree itself or the commit trailer block that already carries `SnapshotId` and `Keepster-Root`.

Settle:

- **What the tree holds for a Withheld path**: nothing at all, a zero-byte blob, or a blob holding a marker. Each
  lies differently to anything reading the tree with plain git, and an absent entry is indistinguishable from a
  path excluded by Include Rules.
- **Where the reason lives**, given that it must survive an index rebuild from git alone, and how it scales: a
  commit trailer is fine for a handful of paths and wrong for a Root with thousands of over-cap files.
- **How ticket 12's write path distinguishes the three causes of "this entry equals the previous one"**: genuinely
  unchanged, carried forward under `incomplete`, and carried forward as a valid capture (a cloud placeholder whose
  size and mtime match). The baseline is now the previous Snapshot's tree, so whatever is stored is what the next
  capture sees.
- **Whether a Withheld path is retried**, and if so on what schedule — an over-cap file that shrinks, a placeholder
  that is hydrated, a special file replaced by a regular one.
- **What ticket 10's junction marker uses**, since "the Snapshot's metadata" is the same unnamed home.
- The downstream consumers that re-opened this from the presentation side:
  [18 — Mounting a Snapshot as a filesystem](18-mount-snapshots.md) ("a zero-byte file, an absent entry and an
  `EIO` all lie differently") and
  [19 — Export: saving a version into a directory](19-export-to-directory.md). Whatever is chosen must answer both
  without a second mechanism. Ticket 19 has since resolved: an exported `Withheld` path is not created and is
  reported per path as `skipped` **with its reason**, so the representation chosen here must carry the reason all
  the way to a client's per-path outcome, not only to `entries`.

## Answer

**The tree holds nothing at all for a Withheld path, and `.keepster-meta` becomes the single home for every
per-path fact git cannot express.** Ticket 17's sidecar gains four record types beside `m`, and a Withheld path is
therefore a sidecar record with no tree entry — "there is no content here" being the one statement about it that
is true.

### The tree holds nothing

Recorded as `docs/adr/0009-withheld-has-no-tree-entry.md`, because a reader running `git show` finds no entry
where a file plainly was, which looks like data loss and is not.

Ticket 18 supplied the test: "a zero-byte file, an absent entry and an `EIO` all lie differently", so the question
is which lie we are willing to hand to `entries`, to diffs, to export and to `git show`.

- **A zero-byte blob** was rejected as an active lie that propagates furthest. It asserts "the file was empty at
  that moment", and export would then create an empty file at exactly the path where ticket 19 promised `skipped`
  with a reason. A user restoring a 3 GiB video would get a 0-byte file and no error.
- **A marker blob** was rejected for putting a small text file where the video was, and for charging filtering at
  five levels — `entries`, diff, Restore, export and `forget` would each have to recognise and hide it, and any
  level that forgot would leak the marker as user content.
- **A symlink marker** (mode 120000 whose target is the reason) adds to that a collision with ticket 10, which
  stores symlinks as symlinks and restores them as symlinks.
- **An absent entry** says only "no content here", which is true, and the sidecar supplies "and here is why",
  which is the ticket 10 definition exactly: recorded as present, content deliberately not held.

The ticket's own objection — that an absent entry is indistinguishable from a path excluded by Include Rules — is
answered by the sidecar, which is where that distinction now lives. The join costs nothing: `entries` already
reads the sidecar for modes, so listing a directory means merging the tree listing with the `w` records under that
prefix.

### The `w` record carries a reason and a kind, and deliberately not a size

```
keepster-meta 1 file=0644 dir=0755
m 0600 .ssh/id_rsa
w too_large var/big.iso
w special_file:fifo run/control
i unstable src/main.rs
i eacces private/notes.txt
p 1757684400.123456789 Documents/report.docx
j Links/data
```

The kind is required because ticket 10's `special_file` covers `fifo`, `socket`, `char_device`, `block_device`,
`mount_point` and `wsl_symlink`, and `entries` cannot say what is there without it. Paths are C-quoted when not
UTF-8, exactly as `m` records are (ticket 10: names are bytes). Records are sorted by path bytes and then by
record type, so the file stays byte-identical when nothing changed and ticket 12's `write_tree` short-circuit
survives.

**Storing the observed size was rejected because it turns a growing over-cap file into a Snapshot generator.** A
size in the record means the sidecar changes, which means the tree changes, which means a Snapshot is created —
one holding nothing new, since the content is precisely what we are not capturing. A 3 GiB log appended to every
minute would produce a Snapshot every minute, each of them empty in substance, and Retention would then thin a
ladder made of nothing. The same argument disqualifies mtime on a `w` record.

The live question the size would have answered — *which files am I not capturing, and how much is that* — is
already answered by ticket 20's Capture Preview, which reports `withheld` split per reason with byte totals and a
top-N largest. What is genuinely lost is the historical form, "how big was it in March", and that is the accepted
price.

### The sidecar is the one home for per-path facts git cannot express

Ticket 23 had to separate three ways an entry can equal the previous Snapshot's, and two of them had no carrier at
all:

1. **Genuinely unchanged** — nothing to record.
2. **Carried forward under `incomplete`** — the read failed. Ticket 10 requires the Snapshot to be marked
   incomplete *and to name what it omits*, and that naming had nowhere to live. The derived SQLite index is
   excluded by ticket 03's invariant: a rebuild would drop the list and the Snapshot would then claim a
   completeness it does not have, which is the exact failure `incomplete` exists to prevent. The commit trailer is
   excluded by ADR 0006's argument — fine for a handful of paths, wrong for the unreadable subtree that produces
   thousands. Hence the **`i` record**, carrying `unstable` or the errno.
3. **Carried forward as a valid capture** — ticket 10's rule that "a placeholder whose size and mtime match its
   last capture carries that version forward" has no carrier for mtime, because a git tree has no mtime at all.
   Size is recoverable from the blob; mtime is not recoverable from anything. Without it the daemon must be
   conservative and Withhold, which means **a file that was captured successfully becomes `cloud_placeholder` in
   every later Snapshot merely because the OS dehydrated it** — a loss, not caution. Hence the **`p` record**,
   holding the mtime alone.

The `p` record does not reintroduce the churn that killed the size field: a placeholder's mtime changes only when
the file really changed, and that is a Snapshot-worthy event on its own.

ADR 0006 made the format versioned and typed for precisely this, and named the alternative it was avoiding: "so
ticket 23 can carry Withheld reasons here rather than reserve a second name". The rule is now statable in one
sentence — **any per-path fact git does not express lives in `.keepster-meta` and nowhere else** — which also
closes ticket 10's last loose end: the **`j` record** gives the junction marker a home, "the Snapshot's metadata"
having been an unnamed place until now.

**A Withheld path carries no `m` record.** Its mode is unusable: there is no tree entry to restore, and ticket 10
already refuses Restore of a Withheld path with `withheld`. Recording it would also re-open the churn problem from
the other side, since a `chmod` on an over-cap file would then mint a Snapshot with nothing in it. An `i` path
keeps its `m` record, because it does have a carried-forward tree entry that Restore can write.

### A Withheld path has no retry queue, because a classification is not a failure

ADR 0004 gave `incomplete` paths a retry queue with growing backoff for a specific reason: the read failed, the
carried-forward entry is indistinguishable from "unchanged", and **no second event is coming**, because the file
itself did not change. None of that holds for Withheld. We observed the path successfully and chose not to take
its content; the classification changes only when the path changes, and a path changing is an event the watcher
sees — with Reconciliation, which ticket 02 made the guarantee rather than the optimisation, as the backstop for
every change the OS fails to announce.

Walking the vocabulary: a file shrinking below the cap is a write; a FIFO replaced by a regular file is an unlink
plus a create; leaving `reserved_name` or `unrepresentable_path` requires a rename. Hydration of a cloud
placeholder may well be announced by nothing at all — and that is exactly the class of unannounced loss for which
Reconciliation cannot be disabled.

**Two reasons change without any filesystem event, and they are closed explicitly:**

- **A configuration reload that changes the inputs to classification** — `files.max_size`,
  `files.max_size_by_pattern`, or Include Rules. Doubling the cap must not leave every `too_large` path withheld
  merely because nothing happened on disk. This refines ticket 04, which made reload automatic without saying that
  a reload must re-evaluate paths already classified.
- **A Registry change**, since `other_root` and `history_store` depend on what is registered rather than on the
  file.

Both put the Root in the queue for Reconciliation rather than inventing a third pass with its own schedule.

### What this gives the downstream consumers

- **`entries`** merges the tree listing with the `w` records under the requested prefix, and reports each Withheld
  path with its reason and kind. Ticket 10's promise that a Withheld path "appears in `entries`" is kept without a
  tree entry.
- **Export** (ticket 19) reports the path as `skipped` with its reason, taken straight from the `w` record, and
  creates nothing — which is only possible because no blob exists to write.
- **Diff** shows a path becoming Withheld, or ceasing to be, as a change, because the sidecar is part of the tree.
- **`forget`** already rewrites the sidecar in the pass that rebuilds every affected tree and pin (ADR 0006), so
  the new record types need nothing new there.
- **Pins** are free for the same reason: ADR 0002 re-creates a pinned Snapshot as a parentless commit sharing the
  original's tree, and the sidecar travels with it.
