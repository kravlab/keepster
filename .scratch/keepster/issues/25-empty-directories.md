# 25 — Empty directories: does the sidecar carry them

Type: grilling
Status: resolved
Blocked by: 23

## Question

Git cannot represent an empty directory, so one never reaches history: a Snapshot of a Root containing
`build/output/` with nothing in it restores a tree where that directory is absent. This was fog on the map until
ticket 23 gave the question a mechanism and a criterion.

The mechanism: `.keepster-meta` is now the single home for every per-path fact git cannot express, carrying typed
records (`m`, `w`, `i`, `p`, `j`) under a versioned header. An empty directory is exactly such a fact, and
etckeeper solves the same problem with `mkdir -p` lines in a metadata file. Adding a record type is cheap and
reserves no new name.

The criterion: ticket 23 rejected putting an observed size in a `w` record because a changing value in the sidecar
changes the tree, which mints a Snapshot holding nothing new — a growing over-cap log would produce one per
capture. Empty directories are the same shape of problem and possibly worse, because **every build tool creates
and removes them constantly**, and under ticket 17's definition each such change is a real change.

Settle:

- **Whether they are captured at all in v1**, given that the tool's promise is recovering overwritten *work*, and
  an empty directory holds none.
- **The churn.** `target/`, `node_modules/.cache/`, `__pycache__/` and every test harness create and drop empty
  directories between captures. If each transition is a change, a Root under an active build produces Snapshots
  continuously while the user's files sit untouched. Is the answer an Include Rules question (they are usually
  excluded anyway), a debounce question, or a reason not to capture them?
- **Which directories qualify.** Only leaf directories with no entries, or also a directory whose entire subtree
  contains no file — the latter being what a user actually notices missing after a Restore.
- **Restore and export.** Ticket 10's Restore preflight has a `parent_missing` confirmation that creates parent
  directories from the umask; an empty directory would instead be created with its recorded mode (ticket 17).
  Ticket 19's export reuses that preflight, so both must agree.
- **The interaction with Reconciliation's cost.** Detecting an empty directory means knowing a directory has no
  entries, which the walk already knows — but the watcher does not, since it sees an event on the *child* that was
  removed, not on the parent that became empty.
- **What `entries` and diff show**, given that a directory with no children has no representation in the tree today
  and would gain one only in the sidecar.

## Answer

**Empty directories are captured, by default, as a `d` record in `.keepster-meta`, and "empty" means a directory
the Snapshot holds no content for — not a directory with nothing on disk.** Two keys govern it, splitting capture
from restore the way ticket 17 already split `restore.modes`.

This supersedes ticket 17's line "Empty directories stay invisible… making the sidecar their carrier is a
different feature". That feature is this one.

### They are captured, because the alternative is a silent omission

A Restore that drops a directory hands back a broken working tree to everything that needs one to exist —
`logs/`, `uploads/`, `tmp/`, and every framework that assumes a writable directory is there. `.gitkeep` exists as
a widespread convention precisely because this git limitation bites real people, and Keepster is not git: it
promises to put the state back.

The deciding argument is internal consistency. This whole design refuses silent omissions: ticket 10 invented
`Withheld` so that "we did not take this, and here is why" is recorded rather than hidden, and `incomplete` exists
so a Snapshot never claims a completeness it does not have. A directory disappearing from history with no trace
at all is exactly the failure both of those were built to prevent.

**The churn objection is weaker than the ticket assumed**, on two counts checked while resolving it:

- The directories that actually churn — `target/`, `node_modules/`, `__pycache__/` — are excluded in practice, and
  after ADR 0007 an excluded directory is never entered at all, so they are not merely ignored, they are unseen.
- A directory being written into is empty only in the window between `mkdir` and the first write. Ticket 04's
  debounce exists to collapse exactly such windows, so a capture has to land inside one to observe it.

**The detection cost objection dissolves entirely.** The ticket asked how the watcher learns a parent became empty
when it only sees an event on the deleted *child*. After ADR 0004 the deletion removes that child's entry from the
index, and the index is sorted — so "does any entry remain under the prefix `a/b/`?" is a prefix comparison
costing **no system call at all**. Reconciliation, which walks everything anyway, remains the backstop for the
changes the OS never announces.

### "Empty" means no captured content

Take `logs/` holding nothing but `*.log` files excluded by Include Rules. On disk it is not empty; in the tree it
has no entry; on Restore it vanishes exactly as a truly empty directory would. The criterion that matches the
problem is therefore **git-unrepresentability**: a directory under which the Snapshot holds no content.

The literal alternative — nothing on disk — is narrower, needs a `readdir` to distinguish the two cases (losing
the free detection above), and leaves `logs/` disappearing silently, which is the very thing this ticket exists to
stop. `CONTEXT.md` gains the term, because the word does not mean here what it means elsewhere.

Consequences of the definition:

- **Every level of a chain qualifies.** In `a/b/c` with no captured content anywhere, all three are empty by the
  definition and each gets its own `d` record, so each keeps its own mode through ticket 17's existing `m`
  mechanism. Recording only the deepest and restoring with `mkdir -p` would lose the intermediate modes, which is
  what ticket 17 was built to preserve.
- **A directory holding only Withheld paths is empty too.** `run/` containing one FIFO has no captured content, so
  it gets a `d` record; Restore recreates `run/` and not the FIFO, which ticket 10 refuses anyway. Consistent
  without a special case.
- **A directory gaining its first captured file loses its `d` record** in the same Snapshot that gains the file's
  tree entry — one Snapshot, correctly, because content appeared.

### Two keys, because the two decisions are read at different times

Recording is a capture-time decision; recreating is a restore-time one. A Snapshot captured while recording was on
and restored after the user turned recreation off must follow the current policy, with its records left intact.
Ticket 17 solved the identical problem by writing modes always and letting `restore.modes` (`snapshot | keep`)
decide whether to apply them.

- **`empty_dirs`** — capture-time, per-Root (it describes a Root, not the daemon, so ticket 24's class test admits
  it at both levels), default on.
- **`restore.empty_dirs`** — restore-time, default on, living beside `restore.modes` so that everything Restore
  decides is in one place.

The three behaviours asked for are all present — capture and recreate is (on, on); record but never recreate is
(on, off); ignore entirely is (off, ·) — without a single key that has to be read at two different moments.

### Restore and export

- **An empty directory is a first-class Restore target** carrying its recorded mode, so a nested chain needs no
  `mkdir -p` and no umask guesswork.
- **This refines ticket 10.** Its `parent_missing` confirmation creates missing parents from the umask; the rule
  becomes *a directory created during Restore takes its recorded mode where one exists, and the umask only where
  none does*, which applies to parents created for a file just as much as to an empty directory restored for its
  own sake.
- **Export always creates them**, regardless of `restore.empty_dirs`. Ticket 19's export builds a fresh copy in a
  destination of the user's choosing, and a copy missing its directories is quietly wrong; Restore writes into the
  user's live tree, where declining to create anything structural is a defensible choice.
- **`entries` lists them** from their `d` records, merged with the tree listing exactly as ticket 23 merges `w`
  records. A diff shows a directory appearing or disappearing, because the sidecar is part of the tree.
