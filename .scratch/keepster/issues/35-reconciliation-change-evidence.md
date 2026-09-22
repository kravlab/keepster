# 35 — What Reconciliation treats as evidence of change

Type: grilling
Status: resolved

## Question

Ticket 02 made Reconciliation the correctness guarantee and ticket 31 made it the thing correctness tests assert
against — with no watcher installed, a real mutation, and an exact comparison of the resulting Snapshot. Both rest
on an unstated decision: **what makes Reconciliation call a path changed.**

The constraints that make it a real question rather than an implementation detail:

- **The baseline carries neither size nor mtime.** ADR 0004 seeds the baseline from the previous Snapshot's tree,
  and a git tree entry holds a mode and an OID and nothing else. So any comparison cheaper than hashing needs a
  carrier that does not exist yet, and the two candidates are both constrained: ticket 03's SQLite index is
  declared wholly derived and deletable, and ADR 0009's `.keepster-meta` sidecar is content, i.e. it changes the
  Snapshot it is stored in.
- **Hashing everything does not fit.** Ticket 30 claims 100,000 files and 10 GiB per Root with startup
  Reconciliation at ≤ 30 s warm.
- **`(size, mtime)` misses a real case.** An edit of identical length inside one mtime tick is invisible, and the
  tick is a full second on some filesystems. Git hit exactly this and solved it — "racily clean" entries, whose
  mtime is not strictly older than the index's own timestamp, are re-hashed — so a precedent exists and should be
  weighed rather than reinvented.
- **A permission-only change is already a change** (ticket 17), and Reconciliation is specified to take the mode
  from the `stat` it performs anyway.
- **Placeholders and incomplete reads interact.** Ticket 23's `p` record exists precisely because a cloud file's
  mtime has nowhere else to live, and ticket 12's retry queue exists because a carried-forward entry for an
  unreadable file is indistinguishable from an unchanged one.

Settle:

- **The comparison itself**: which observed facts are compared, against what carrier, and whether the answer
  differs between startup Reconciliation, a rescan-signal Reconciliation and the periodic pass.
- **Where the comparison inputs are stored**, given that the index may be deleted at any time and the sidecar is
  part of the Snapshot's content.
- **The racy window**: whether git's re-hash rule is adopted, and what the behaviour is on a filesystem with
  second-granularity mtime.
- **Whether a hash-everything mode exists** as an opt-in, and if so what budget it is allowed to break.
- **What this costs the ≤ 30 s warm budget** of ticket 30, stated as an arithmetic claim over one `stat` per path.

## Answer

**The `stat` comparison is a filter in front of hashing, never the verdict.** The verdict is always the OID: a path
that survives the filter is read and hashed, and only a differing OID produces a new entry. That framing decides
almost everything below, because it makes the two error directions wildly asymmetric — a false positive costs one
hash, a false negative costs the user's work — so the rule is deliberately biased: **any doubt hashes**.

### The comparison inputs live in the derived index, and they are derived from observation

A git tree entry carries a mode and an OID and nothing else (ADR 0004), so any comparison cheaper than a hash needs
a carrier that does not exist yet. It is a **stat-cache table in the SQLite index**, keyed by Root and path.

The sidecar was rejected by ADR 0009's own argument. That ADR refused to put a *size* in a `w` record because a
growing file would then "mint one substantively empty Snapshot per capture"; an mtime is strictly worse, because it
changes for **every** touched file — an ordinary `touch` would rewrite the sidecar blob and so change the
Snapshot's content, making metadata churn indistinguishable from work. Hashing everything on every pass was
rejected on ticket 30's arithmetic: 10 GiB per pass is read- and hash-bound, tens of seconds against a 30 s warm
budget, repeated hourly, evicting the user's page cache to reproduce the previous answer in almost every pass.

This **amends ticket 03's invariant in one specific way**, which is why it is recorded as
[ADR 0014](../../../docs/adr/0014-stat-cache-is-derived-from-observation.md): the index remains a derived cache
that may be deleted at any time, but this table is derived **from observation of the Working Tree**, not from git
objects. Ticket 03's rebuild procedure reconstructs paths and OIDs from Segment walks and cannot reconstruct this;
losing the cache therefore costs exactly one full pass that hashes everything — the cold case ticket 30 explicitly
declines to claim.

### What is compared

**Size and mtime, by inequality, plus the mode from the same `stat`.**

**Inequality, not "newer"**, because a file restored with its original timestamp — `cp -p`, `rsync -a`, an archive
unpacked with timestamps — is older than the cached observation and would be invisible forever under a
"newer than" test. The mode is compared against the tree entry and ticket 17's `m` record, so a permission-only
change is detected with no hash at all, which is what ticket 17 assumed when it said Reconciliation takes the mode
from the `stat` it already performs.

**Inode and device are excluded**: an atomic save changes the inode, but it changes the mtime too, so the field
adds almost nothing while producing false positives after any operation that renumbers inodes. **ctime is excluded**
because it does not mean the same thing on the three platforms — on Windows it is the *creation* time, not the
metadata-change time, which is precisely why git carries `core.trustctime`. A field enters the filter only if it
catches a *miss*, not merely because it is "also about change".

The residual hole is stated rather than hidden: content replaced by different content of identical size carrying an
identical preserved mtime is not detected. Git has the same hole, and reaching it requires the replacement to
originate from an observation we already recorded.

### The racy window: git's rule

A file written *during* the pass takes an mtime equal to the moment of observation, so a further edit inside the
same tick has the same size and the same mtime and would be invisible — and the tick is two seconds on FAT and
exFAT, and a second on some network mounts.

**An entry whose mtime is not strictly older than the pass's start is untrusted and re-hashed on the next pass** —
git's "racily clean" rule. Its cost is bounded by construction, being only the files changed while the pass ran,
and it self-tunes to the filesystem's granularity because it compares against an actually observed time. Relying on
the watcher to deliver the missed edit was rejected outright: ticket 02 made the watcher a latency optimisation and
Reconciliation the guarantee, so leaning on it here would cancel that guarantee in the one place it must be
self-sufficient. A fixed "re-hash anything modified in the last N seconds" window was rejected as a constant that is
simultaneously too large (extra hashing every pass) and too small (N = 1 does not cover a two-second granularity).

### Where mtime cannot be trusted

A key, not an automatic escalation: **`reconcile.verify = "stat" | "hash"`**, defaulting to `stat`, plus a one-off
`keepster reconcile --verify`. Where the filesystem under a Root has coarse mtime granularity — detectable from the
filesystem type the daemon already reads for ADR 0011's volume identity and ticket 33's bundle — **`status` says
so**, because ticket 05 forbids looking healthier than the truth.

Escalating automatically was rejected on cost, not on principle: on a network Root it means **reading the entire
Root over the wire on every pass**, hourly, which would make the tool unusable exactly where ticket 29 decided to
accept network Roots. The difference between automatic and opt-in is not the strength of the guarantee but who
decides to pay for it, and with the weakness reported in `status`, the user can.

### The occasion changes the area walked, never the rule

There are five occasions — daemon start (ticket 02's "black hole" of unknown downtime), an OS rescan signal, the
periodic pass, a config reload or Registry change (ADR 0009 queues the Root), and a remount (ADR 0011). All use the
same filter, the same racy rule and the same doubt-hashes bias. **Only the area differs**: the whole Root in every
case except a rescan signal, which walks the subtree the OS named where it named one (macOS supplies a path) and
the whole Root where it did not (`IN_Q_OVERFLOW` carries none on Linux; Windows discards the whole buffer).

Hashing everything at startup was rejected because its premise is false: downtime does not weaken the evidence. The
cached size and mtime describe the state *before* the daemon stopped, so anything edited while it was down differs
from them and is caught. What downtime changes is the number of changed files, not the quality of the filter — and
the cost would be a full 10 GiB hash on every boot, the cold pass ticket 30 refused to promise.

### The budget holds, with the arithmetic stated

One `stat` per path at the 100k-file ceiling is seconds with a warm cache, after which only the differing set is
read and hashed; at a working developer's churn that set is small. Ticket 30's ≤ 30 s warm Reconciliation therefore
stands, while the cold pass remains disk-bound and explicitly unclaimed.

Two consequences worth recording. **Directory mtimes are not used to prune the walk**: a directory's mtime does not
change when a file's contents change, so pruning on it would skip exactly the edits this pass exists to find; it is
`stat` per path or nothing. And the cache is maintained by the pass itself — rows for paths that vanished are
removed when the pass that observed their absence completes, so it cannot outlive what it describes. Paths excluded
by Include Rules never enter it, since ADR 0007 prunes before the walk descends.

Refines tickets 02, 03, 12, 17, 29 and 30.
