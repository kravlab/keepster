# 25 — Empty directories: does the sidecar carry them

Type: grilling
Status: needs-triage
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
