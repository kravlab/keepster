# File modes live in a metadata sidecar inside the Snapshot tree

Git records only five tree modes, so a Snapshot built from an index we own (ADR 0004) cannot hold anything beyond
the executable bit — yet an agent's `chmod -R 777 .` and an accidental `chmod 644` on a private key are exactly the
accidents this tool insures against. We therefore store the full POSIX mode of every path in one extra blob,
`.keepster-meta`, at the root of each Snapshot's tree, holding only the paths whose mode deviates from the Root's
prevailing defaults. A reader will find an entry in the tree that exists on no disk and that `entries`, diffs and
Restore all hide, which is why it is written down.

## Considered Options

- **The derived SQLite index.** Rejected on ticket 03's invariant: the index may be deleted and rebuilt from git
  objects at any time, so modes kept there evaporate on a rebuild. ADR 0002 rejected a pin flag for the same reason.
- **A blob outside the tree, addressed by a commit trailer.** Rejected as physically impossible: a commit
  references its tree and its parents only, so the blob is unreachable and the `gc --prune=now` that ticket 06
  requires collects it.
- **The trailer text itself.** Rejected by ticket 23's line — fine for a handful of paths, wrong for thousands —
  which the mass-`chmod` case crosses immediately, in a commit-message block whose 25% rule is fragile besides.
- **One metadata blob per directory, as bup's `.bupm`.** Rejected: the locality it buys matters only to enormous
  Roots, and it charges a service entry in every directory of every tree that `entries`, diffs, Restore and Forget
  must then filter at every level.
- **A full mode table rather than deviations.** Rejected because it is rewritten whenever any file is added or
  removed, which defeats the `write_tree` short-circuit permanently; with fixed `0644`/`0755` defaults instead of
  the Root's modal pair, a Root under umask 002 degenerates into exactly this.

## Consequences

- **Unchanged modes must cost nothing.** The file is sorted by path bytes and otherwise deterministic, so an
  unchanged set of modes yields byte-identical content, the same OID, and an entry carried forward from the baseline
  untouched — leaving ticket 12's short-circuit (0.0036 s against 0.162 s) intact.
- **The header is recomputed only by Reconciliation.** The modal file and directory modes need a full walk, which
  Reconciliation performs anyway; every other capture carries the header forward.
- **A permission-only change becomes a real change**, because it changes the sidecar and therefore the tree. The
  watcher cannot tell a `chmod` from a `touch` — `notify` collapses `IN_ATTRIB` into `Modify(Metadata(Any))` and
  Windows attribute and security notifications into `Modify(Any)` — so an attribute event means "go `stat` that
  path", and a false positive simply mutates nothing.
- **One reserved name in the Root's path space.** A user file named `.keepster-meta` at the Root's top level is
  Withheld `reserved_name`, the mechanism ticket 10 defines for `.git`.
- **Pins are free, `forget` is not quite.** A pin shares the original's tree (ADR 0002), so the sidecar travels with
  it at no cost; `forget` must drop the forgotten paths' records in the same pass that already rebuilds every
  affected tree and pin.
- **It is a metadata sidecar, not a mode file.** Records are typed and the format is versioned, so ticket 23 can
  carry Withheld reasons here rather than reserve a second name — the one decision that would be expensive to
  revisit, since the name reaches every tree.
