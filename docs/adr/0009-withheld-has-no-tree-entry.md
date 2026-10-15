# A Withheld path has no tree entry, and the sidecar is the one home for what git cannot express

A Snapshot that deliberately does not hold a path's content stores **nothing at all** for it in the git tree. The
path's existence and the reason it was withheld live as a `w` record in `.keepster-meta`, the metadata sidecar
ADR 0006 introduced. A reader running `git show` on a Snapshot will find no entry where a file plainly was on
disk, which looks like data loss and is not, so the reasoning is recorded here.

The same decision makes the sidecar the single home for **every per-path fact git cannot express**, because two
such facts turned out to have no carrier anywhere: which paths a Snapshot marked `incomplete` omitted, and the
mtime a dehydrated cloud placeholder must be compared against.

## Considered Options

Ticket 18 supplied the test while arguing about mounting: *"a zero-byte file, an absent entry and an `EIO` all lie
differently."* The question is which lie reaches `entries`, diffs, export and `git show`.

- **A zero-byte blob.** Rejected as the lie that propagates furthest. It asserts the file was empty at that moment,
  so export would create an empty file at exactly the path where ticket 19 promised `skipped` with a reason, and a
  user restoring a 3 GiB video would receive 0 bytes and no error.
- **A marker blob** holding the reason as text. Rejected for putting a small text file where the video was, and for
  charging recognition-and-hiding at five levels — `entries`, diff, Restore, export, `forget` — where any level
  that forgets leaks the marker as user content.
- **A symlink marker** (mode 120000 whose target is the reason). All of the above, plus a collision with ticket 10,
  which stores symlinks as symlinks and restores them as symlinks.
- **An absent entry.** Says only "no content here", which is true. The objection — that it is indistinguishable
  from a path excluded by Include Rules — is answered by the sidecar, which is where that distinction lives, and
  the join costs nothing because `entries` already reads the sidecar for modes.

**A size in the `w` record** was considered separately and rejected on a different axis: a changing value in the
sidecar changes the tree, so a 3 GiB log appended to every minute would mint a Snapshot every minute, each holding
nothing new, and Retention would thin a ladder made of nothing. The live form of the question that size would have
answered — which files are not being captured, and how much — is already served by ticket 20's Capture Preview.
The same argument disqualifies mtime on a `w` record.

## Consequences

- **The rule is statable in one sentence**: any per-path fact git does not express lives in `.keepster-meta` and
  nowhere else. Four record types join ADR 0006's `m`: `w` (reason and kind), `i` (`unstable` or the errno behind
  an `incomplete`), `p` (a placeholder's mtime), `j` (ticket 10's junction marker, which until now had only the
  unnamed home "the Snapshot's metadata").
- **`i` exists because the alternatives were closed.** Ticket 10 requires an incomplete Snapshot to name what it
  omits. The derived SQLite index cannot hold that: ticket 03 lets it be deleted and rebuilt from git objects at
  any time, and after a rebuild the Snapshot would claim a completeness it does not have — the exact failure the
  flag exists to prevent. The commit trailer is closed by ADR 0006's own argument, since an unreadable subtree
  produces thousands of paths.
- **`p` exists because a git tree has no mtime.** Ticket 10 carries a dehydrated placeholder's previous version
  forward when size and mtime match its last capture. Size is recoverable from the blob; mtime is recoverable from
  nothing, and without it the daemon must be conservative and withhold — meaning a file that *was* captured
  successfully becomes `cloud_placeholder` in every later Snapshot merely because the OS freed its space. It does
  not reintroduce the churn that killed the size field: a placeholder's mtime changes only when the file really
  changed, which is Snapshot-worthy anyway.
  As built (ticket 75): the first time a captured file is found dehydrated, its mtime comes from the stat cache of
  ADR 0014, which records the size and mtime of the read that produced the held blob; from then on the path keeps a
  `p` record for as long as it is held, hydrated or not, updated whenever the file is read again. A `p` record
  counts as no change of its path, but one seen for the first time is something to record, as modes observed for
  the first time are: the pass that first finds a file freed mints one Snapshot with no changed path, so the record
  reaches history even where nothing else changes. Freeing or fetching it again mints none. Should the index be
  lost after a file is freed and before that pass, the file is `cloud_placeholder` until it is fetched again. Linux
  has no placeholders to recognise: a FUSE sync client fetches on read and marks nothing beforehand, so there every
  file is read as any other.
- **A Withheld path carries no `m` record.** Its mode is unusable — there is no tree entry to restore onto, and
  ticket 10 already refuses Restore of a Withheld path — and recording it would re-open the churn problem from the
  other side, since `chmod` on an over-cap file would mint an empty Snapshot. An `i` path keeps its `m`, because it
  does have a carried-forward entry.
- **Determinism extends to the new types.** Records sort by path bytes and then by record type, so an unchanged set
  of facts yields byte-identical content, the same OID, and an entry carried forward untouched — leaving ticket
  12's `write_tree` short-circuit intact.
- **Withheld gets no retry queue.** ADR 0004 gave one to `incomplete` because a failed read produces no second
  event. A Withheld path is the opposite: it was observed successfully, and its classification changes only when
  the path changes, which is an event, with Reconciliation as the backstop for the changes no OS announces. The two
  reasons that change without any filesystem event are closed explicitly — a configuration reload touching
  `files.max_size`, `files.max_size_by_pattern` or Include Rules, and a Registry change affecting `other_root` and
  `history_store`, both of which queue the Root for Reconciliation rather than adding a third pass.
- **The downstream consumers need no second mechanism.** Export reports `skipped` with the reason straight from the
  `w` record and creates nothing, which is only possible because no blob exists to write; diff shows a path
  becoming Withheld, or ceasing to be, because the sidecar is part of the tree; `forget` already rewrites the
  sidecar in the pass that rebuilds every affected tree and pin; and a pin shares the original's tree, so the
  sidecar travels with it free.
