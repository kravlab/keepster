# 23 — How a Withheld path is represented in the git tree

Type: grilling
Status: open
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
