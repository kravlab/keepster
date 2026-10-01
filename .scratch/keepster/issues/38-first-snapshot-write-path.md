# 38: The first Snapshot — History Store invariants and the write path

**Blocked by:** 37 (The Registry — register, deregister and resolve a Watched Root)

**Status:** done

**What to build:** a user can ask for a Snapshot of a Watched Root and get one, stored in a History Store that obeys
the storage invariants every later ticket depends on. Triggers, rules and reading come later; this is the write path
and the store's shape.

The store is segmented: one ref per Segment, each starting from a parentless commit, Snapshots chained within it,
leaf-only ref names, reflogs off so a deleted ref can actually free objects, git's own file-mode probing disabled,
and the `SnapshotId` and Root id carried as commit trailers (ADR 0001). The baseline for each capture is re-seeded
from the previous Snapshot's tree, so the daemon holds no index between Snapshots (ADR 0004). Capture takes the
exclusive per-store lock. Only the built-in reserved-name rule applies for now: a `.git` component is recorded as
Withheld and never descended into.

- [x] A manual capture produces a Snapshot with a ULID `SnapshotId`, and the id appears in the commit trailer
- [x] A Segment is created lazily for the period its first Snapshot falls in, and its start commit is parentless
- [x] Reflogs are off and file-mode probing is disabled at store creation, both asserted against a real store
- [x] The baseline is read back from the previous Snapshot's tree at every capture; entries are applied in
      lexicographic order, and the tree write reuses unchanged subtrees
- [x] A second capture with no changes produces a Snapshot whose tree is identical to its parent's, cheaply
- [ ] A `.git` component (and its platform aliases) is recorded as Withheld with the reserved-name reason and is
      never entered — **half done**: not captured and not entered, while the record that says so waits for ticket 48
- [ ] Capture holds the exclusive per-store lock; a concurrent capture coalesces rather than blocking or dropping —
      **half done**: the lock is held, and coalescing belongs to the policy kernel of ticket 43
- [x] Every timestamp the store records derives from the injected `now`, proven by a test that writes a Snapshot
      dated a month ago

## Comments

- **Two criteria above are deliberately unfinished here and were transferred, not dropped.** The record that names a
  Withheld path belongs to ticket 48, which already carries the reserved-name reason among its criteria; coalescing a
  capture that meets a held lock is now a criterion of ticket 43. The boxes stay unticked so that nothing here claims
  to be finished, while the status is `done` because this ticket's own scope is.

- **The store lives at `history.git` inside the Root's own directory**, beside the Registry record, so
  the record, the store and ticket 39's derived index each have a place of their own.
- **The Segment is named for the hour its first Snapshot fell in, in UTC**, by `jiff`: a local bucket
  would renumber Segments at every offset change and sort wrongly across one, and the name is also the
  ref's last component, which ADR 0001 requires to stay a leaf and to sort chronologically.
- **The Snapshot's moment is a parameter all the way down** (ADR 0012) and the store's tests assert the
  recorded commit time against that value — which is what makes a month of Retention history
  producible by this very path in ticket 51 rather than waited for. `git2::Signature::now` joined the
  lint's forbidden list with this ticket, and the lint was verified on a live violation.
- **Every regular file is captured; nothing else yet.** Include Rules arrive with ticket 41, the records
  that name what a Snapshot deliberately does not hold with ticket 48, and symlinks and special files
  with ticket 49. Until then a path that is not a regular file, and a name that is not UTF-8, are passed
  over — which this ticket records rather than hides, because a Snapshot that omits silently is the one
  thing the product forbids.
- **`.git` is not captured and not entered**, which is this ticket's half of the criterion; the record
  that says so needs the sidecar, and that is ticket 48's by the decision taken when this ticket was
  planned. An empty directory also leaves no trace yet — git cannot represent one — and ticket 50 owns
  that.
- **No git concept appears in the store's public signatures.** `CapturedSnapshot` carries a `SnapshotId`
  and a `SegmentName`, so the tests resolve commits through the refs themselves; ticket 08 makes that
  the property keeping the storage engine replaceable in one crate.
- **One lock per History Store**, taken by a capture and — from ticket 52 — by a maintenance pass:
  git concedes a corruption risk under concurrent access, and two captures racing would leave one of
  them unreferenced in any case. A second capture **waits**; it does not coalesce. Coalescing is a
  decision about Triggers, so it belongs to ticket 43's policy kernel, where the accumulated paths and
  the rate limit live — implementing it here would put trigger policy in the transport layer.
- **The declared MSRV rises from 1.85 to 1.87, and the gate is why.** `git2 0.21.0` declares no
  `rust-version` at all while using `str::from_utf8` as an inherent constructor, so the floor is set by
  measurement rather than by a declaration: 1.85 and 1.86 both fail on
  `inherent_str_constructors`, 1.87 compiles the whole workspace. Downgrading `git2` is forbidden and
  dropping it would contradict ticket 01, so the floor moved — stated here because a floor is a promise
  to the packagers of ticket 27.
- **Review found four defects worth the name, and they are fixed rather than recorded.**
  - *Deletions were never recorded.* The index was seeded from the previous tree and only added to, so a
    file deleted from the Working Tree was carried forward for ever and the Snapshot claimed content
    that was gone — the one disagreement with reality this product exists to catch. Baseline entries
    whose paths the walk did not find are now removed, with a regression test.
  - *One unreadable file cost the whole Snapshot.* A capture now takes the Snapshot anyway, carries the
    previous content forward for what it could not read, and returns the list — which `incomplete` and
    `omitted` put on the wire, as ticket 07 requires of every shape that serves a Snapshot. Ticket 48 is
    where the omission becomes part of the Snapshot itself.
  - *The baseline did not cross an hour boundary.* Only the current Segment's ref was consulted, so the
    first Snapshot of a new hour had no baseline at all — exactly the case ADR 0004 says must be tested
    rather than assumed. The baseline is now the tip of the newest Segment at or before the current one,
    while the parent stays the current Segment's tip so a new Segment is still parentless. The test
    discriminates by making a file unreadable across the boundary, since tree equality cannot: trees are
    addressed by content, so a rebuild from scratch yields the same id.
  - *The TREE cache was being thrown away.* Every file was inserted on every capture, and an insertion
    invalidates that path's ancestry, so `write_tree` rebuilt everything. An entry whose content and
    mode already match the baseline is now left untouched. Hashing remains until ticket 44 brings the
    stat cache; the saving here is the tree rewrite. The *cost* claim itself is not asserted by a unit
    test — it belongs to the bench tripwire of ticket 69, because a timing assertion in the suite would
    be the flaky test ticket 31 forbids.
- **Also from review:** index order is now the byte order of the `/`-joined path rather than `Path`'s
  component order, which disagree exactly where a file and a directory share a stem (`sub.txt` against
  `sub/b.txt`) and which is where ticket 12's measured 0.86 s against 22.74 s applies; the Registry lock
  is released before the walk, so an unrelated Root no longer waits on a capture that may take seconds;
  `open` re-applies the two store invariants instead of assuming whoever created the store kept them
  (ADR 0001 calls them invariants to assert); `file_size` is zero rather than `u32::MAX`; a tautological
  UTC test is gone; and the capture's weak assertions now check the Segment against the moment the same
  answer reports and the CLI's printed identity against the Snapshot it took.
- **One review item was rejected with reason.** A Root whose record cannot be read keeps answering
  `root_unavailable`, not `root_degraded`: `CONTEXT.md` reserves `Degraded` for a Root that is watched
  but limited, while an unreadable record leaves nothing to capture at all, and `/v1/status` already
  reports that condition as `Unavailable`.
- Verified: 69 tests, `cargo fmt --check`, `clippy -D warnings`, `cargo deny check licenses bans`, and
  `cargo check` on 1.87. **The Windows-target lint can no longer run on this host**: `git2`'s vendored
  zlib needs an MSVC-compatible compiler, and neither it nor a mingw toolchain is present, so the
  `cfg(not(unix))` mode branch is unproven until the gate's own Windows runner sees it. No test has run
  on Windows or macOS.
