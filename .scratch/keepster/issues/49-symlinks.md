# 49: Symlinks — stored as links, optionally followed

**Blocked by:** 48 (Withheld, the metadata sidecar, special files and paths as bytes), 44 (Reconciliation — the
guarantee, and what counts as evidence of change)

**Status:** done

**What to build:** a symlink is captured as a link and restored as a link, a user can opt specific links into being
followed so a linked-in tree is captured, and Windows' several kinds of link surrogate each behave predictably.

Following is a pattern list, empty by default, layered like the Include Rules, and rules over followed content use
the **logical** path — so a denylist that matches names still refuses a private key reached through a link.
**Coverage of followed targets is periodic Reconciliation, not extra watches**, because watches are a shared
per-user resource and a mutable real-to-logical path mapping loses events silently.

- [x] A symlink is stored with the link mode and its target text, is never followed by default, and is restored as a
      link
- [x] The follow list opts specific links in; following stops at a target inside another Root or a History Store, or
      one that closes a cycle, which is stored as a link instead
- [x] A followed target that *contains* another Root or a History Store is followed, with those subtrees Withheld
- [x] A Root with a non-empty follow list reconciles its followed targets on their own interval, never below the rate
      limit, and reports that as its own fact in `status` rather than as "coverage not guaranteed"
- [x] An unavailable target carries the subtree's previous content forward and marks the Snapshot incomplete, naming
      the link with its errno, so an unmount does not read as thousands of deletions
- [ ] ~~On Windows a junction is stored with its marker and restored as a junction; a real symlink that cannot be
      created is refused with its own reason and never written as a plain file~~ — moved to ticket 76
- [ ] ~~Volume mount points and links whose target cannot be read are Withheld as special files~~ — moved to ticket 76
- [ ] ~~Registering a Root through a link asks whether the link or its real path is the Root, and refuses until
      told; a link-mode Root follows a retargeted link, with the overlap rule still enforced~~ — moved to ticket 77
- [x] Restoring through a followed link writes through it only while the link is still matched and leads nowhere
      forbidden; otherwise the path is refused with its reason

## Comments

- 2026-10-11: ticket 47's Restore passes over a link in a Snapshot (`source()` in
  `keepster-store/src/history/restore.rs` returns `None` for mode `120000`), since none are captured yet; this
  ticket makes it restore one as a link, and raise `symlink_unsupported` where the platform cannot. The
  `link_check_failed` refusal there today refuses every link above a file, because no link is followed yet —
  `files.follow_symlinks` decides which may be.
- 2026-10-11: split with the user before building: Windows link surrogates (junctions, mount points,
  `symlink_unsupported`) became ticket 76, and registering a Root through a link ticket 77.
- 2026-10-11: done. A link is a `120000` entry whose blob is its target text, listed as `symlink`, read
  by `content` as that text, and restored as a link — written beside its place and renamed over it; a file
  where the Snapshot holds a link, or the reverse, is a `type_change`. `files.follow_symlinks` is a pattern
  list layered like the Include Rules, matched against the link's own path with its target's kind, and —
  a decision of its own — settable by the user's files only, never by a Root's `.keepster.toml`, since a
  file arriving in someone else's repository must not pull `~/.ssh` in through a link of its own (ticket
  04's class of keys deciding what may enter). Content beyond a followed link is judged by its logical path.
- 2026-10-11: following stops — and the link is stored as a link — at a target that is the Root, holds
  it, holds a directory on the way or a target already followed, or is a directory already on the way by
  its `(dev, ino)`; a plain directory revisited by identity, such as a bind mount of an ancestor, is named
  `i eloop` with its subtree kept. Other Roots and the History Store are fenced by their real paths (the
  daemon passes them in), so a target inside one is not followed and one containing one has it Withheld.
  An unreachable target — a directory pattern still counts — keeps its subtree and names the link with
  its errno. The stat filter keys the evidence of a file beyond a followed link by its real path, so a
  link retargeted at a lookalike tree is read again.
- 2026-10-11: the followed targets are covered by a whole pass every `files.follow_reconcile_interval`
  (15 min, never below the rate limit) for a Root captured automatically — recorded as `reconcile`, since
  every pass walks the whole Root (ADR 0014) — and `status` reports `follow_targets { count,
  last_reconciled_at, interval }`, the interval in seconds and `last_reconciled_at` the last pass of any
  kind. A Restore writes through a followed link only while the follow list still names it and neither its
  target nor the directory the file lands in is inside another Root or a History Store; otherwise
  `link_check_failed`. A final link the list no longer names, where the Snapshot holds a file, is a
  confirmable `type_change` that replaces the link — nothing is written through it.
