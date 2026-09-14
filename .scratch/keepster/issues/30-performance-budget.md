# 30 — Performance budget, and how it is measured

Type: grilling
Status: needs-triage

## Question

Several decisions were already made on measured numbers, which is precisely why the absence of a stated budget is
a gap: the evidence exists but nothing says what it must stay under.

Numbers already on the record: ticket 12 measured `write_tree` at 0.83 s with no cache, **0.162 s** after one
changed path and 0.0036 s with none, on 100k entries; ticket 02 measured ~10k watches and 1–4 s to arm a
100k-file tree against a per-user budget of 133,264; ticket 12 put the transient index at ≈ (130 + path length)
bytes per entry, 17 MB for 100k files; ticket 09 measured CodeMirror 6 at 96.7 KiB gzip before first paint.

Settle:

- **The budget itself**: snapshot latency for a typical change, daemon idle cost (CPU and resident memory with N
  Roots watched and nothing happening), store growth per day for a working developer, and the time a full
  Reconciliation may take on a 100k-file Root.
- **What the numbers are for.** A budget that fails nothing is decoration: does breaching one fail a test, fail a
  release, or merely get reported? This is the same decay problem ticket 14 flagged for `make check-api`.
- **How they are measured, and on what.** A fixture Root of a stated size, generated or committed; which platform
  is authoritative given the three differ sharply; and whether measurements run anywhere before a CI pipeline
  exists — the CI question is still fog, so this must not quietly depend on it.
- **Which existing decisions the budget would have changed had it existed**, as a sanity check that it is set at a
  useful level rather than around whatever the code already does.
- **The Root-size ceiling**, if any: the largest Root v1 claims to support, and what the daemon does with one
  beyond it — ticket 06's disk cap pauses capture rather than filling the disk, and an analogous answer may be
  needed here.
- **The relationship to ticket 20's Capture Preview**, which walks a whole tree on demand and is the one operation
  a user can trigger that is deliberately expensive.
