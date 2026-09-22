# Part of the derived index is derived from observation, not from git

Ticket 03 declares the SQLite index a derived cache that may be deleted and rebuilt from git objects at any time,
and ADR 0004 leaves a tree entry carrying only a mode and an OID. Reconciliation needs a cheaper test than hashing,
so it keeps the last observed size and mtime per path — and **that table cannot be reconstructed from git**, because
git records nothing about the Working Tree's metadata. The invariant survives in the form that matters (the cache
may still be deleted at any time and nothing in it is authoritative), but its rebuild story now has two halves:
paths and OIDs come from Segment walks, while the stat cache is refilled by one full pass that hashes everything.

## Considered Options

- **Putting size and mtime in `.keepster-meta`.** The sidecar already carries per-path facts git cannot express, so
  it looks like the natural home, and it would make the cache genuinely rebuildable from git. Rejected by ADR 0009's
  own argument, only stronger: that ADR refused to record a *size* in a `w` record because a growing file would mint
  one substantively empty Snapshot per capture; an mtime changes for **every** touched file, so an ordinary `touch`
  would rewrite the sidecar blob and produce a Snapshot in which nothing changed but our own metadata — capture
  churn indistinguishable from the user's work.
- **Hashing every path on every pass**, keeping the index wholly git-derived. Rejected on ticket 30's arithmetic:
  10 GiB per pass is read- and hash-bound, tens of seconds against a 30 s warm budget, hourly, evicting the user's
  page cache to recompute an answer that is unchanged in almost every pass.
- **Holding the cache only in memory.** Equivalent to the previous option at every daemon start, which is the
  moment ticket 02 makes Reconciliation mandatory, so it would put a full-store hash on every boot.

## Consequences

- **A cache loss costs one full hashing pass**, not a broken guarantee. That pass is the cold case ticket 30
  explicitly declines to claim, and it is self-healing: the pass that pays it refills the table.
- **The stat data is never authoritative.** It gates whether a path is read; the OID decides whether anything
  changed, so a stale or wrong row costs a hash, never a wrong Snapshot. Any doubt — a racily-clean entry, a missing
  row, an unreadable `stat` — hashes.
- **Nobody may "fix" the asymmetry by moving mtimes into the Snapshot.** That is the option rejected above, and it
  would convert metadata churn into history.
- **`keepster status` must be able to say the filter is weak** where the filesystem's mtime granularity is coarse,
  because the user is the one who decides whether to pay for `reconcile.verify = "hash"`.
