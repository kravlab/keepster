# 30 — Performance budget, and how it is measured

Type: grilling
Status: resolved

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

## Answer

**The budget is two layers with different audiences and different enforcement.** An *outcome budget* states what
the product promises and is reviewed at release; an *operation tripwire* catches regressions mechanically and
promises nothing. Keeping them apart is what lets the tripwire be gated today — from a first measurement — while
the outcome numbers are still predictions, no daemon-side code existing yet. Collapsing them into one layer
produces either the decoration this ticket names (numbers that fail nothing) or a gate set around whatever the
code already does.

The outcome layer is **widened to include read latency**, which no resolved ticket had budgeted. `map.md` justifies
the entire storage choice on it — *"judged on how cheaply it answers 'give me (Snapshot, path)' and 'diff these two
versions'"* — and ticket 07 asserts a third-party client "needs per-interaction latency" with no figure attached.
Ticket 28 handed this ticket three dimensions (snapshot latency, idle cost, store growth) and all three are the
write side; the omission was structural, not accidental.

### The outcome budget

| Line | Value | Basis |
| --- | --- | --- |
| Capture, the daemon's own contribution | ≤ 5 s p95 at ceiling | 0.162 s `write_tree` + unmeasured `read_tree` |
| Fetch one version of a file | ≤ 100 ms p95 | the read path the store exists for |
| Diff two versions, file ≤ 1 MiB | ≤ 250 ms p95 | daemon computes hunks (ticket 07) |
| A path's chronology | ≤ 100 ms p95 | served by the derived index (ticket 03) |
| Startup Reconciliation | ≤ 30 s at ceiling, warm page cache | 1–4 s to arm watches + a `stat` per path |
| Capture Preview | first row ≤ 200 ms, then ≥ 20,000 paths/s | input is unbounded by design |
| Idle CPU | ≤ 0.5 % averaged over 10 min, no changes | — |
| Idle RSS | ≤ 150 MiB with three Roots at ceiling | — |
| Capture peak RSS | ≤ idle + 40 MiB | 17 MB transient index at 100k entries |
| Steady-state store size | ≤ 2× the Root's included content | — |

**Coverage latency is stated as the daemon's contribution, not as total time.** Total time is configuration
arithmetic — a ~3 s debounce plus up to the 60 s rate limit (ticket 04) — and promising it would be promising the
user's own choice back to them.

**Store growth is a ratio, not bytes per day.** An absolute figure depends on how one person happens to work and is
unverifiable; a ratio is measurable on the fixture and answers the question actually asked, "what will this cost
me". It also makes ticket 06's *assertion* that the ladder bounds growth into something checkable.

**Preview gets throughput, not a total.** Its input has no ceiling by construction: it runs on unregistered
directories of any size, which is its purpose, and after the ceiling decision below it also runs at registration.
A total for an unbounded input is not a number but a function. Time-to-first-row matters because that call now
stands between the user and registering a Root.

**Reconciliation's cold-cache cost is explicitly not claimed.** It is disk-bound, and promising another
subsystem's I/O is the same dishonesty ticket 28 refused when it declined to name a WCAG level.

### The Root ceiling, and a daemon-wide one

**v1 claims 100,000 included files, 10,000 included directories and 10 GiB of included content per Root** — one
number per resource, because any one can be reached alone (20k photo files at 40 GiB leave the watch budget
untouched and still break the first capture). All three match the scales this system was actually measured at.

**Nothing refuses.** Above the claim, registration runs ticket 20's Capture Preview and asks for explicit
confirmation; watch exhaustion falls back to periodic Reconciliation with `Degraded`, which is research 02's F-L2,
already designed. Refusal would contradict every ceiling in this design: ticket 06 overshoots and reports, ticket
05 forbids looking healthier than the truth.

**A per-Root ceiling cannot bound a per-user resource, so a daemon-wide one is added**: across all Roots, at most
**half** of `max_user_watches` as read at startup. The remainder is the editor's and the language server's share of
the same counter (research 02, lines 58-59). Exceeding it moves the next Root onto F-L2 with the reason in
`status`. Six Roots at ceiling already consume half this machine's limit, and the figure varies per machine — the
133,264 on record is this box's, which research 02 says plainly and nothing downstream honoured.

### Reference environment

**The tripwire has one reference machine, on Linux.** It compares run against run, so mixing platforms in it is
meaningless: what it produces is a delta, and a delta needs one base.

**The outcome claim is stated per platform, with macOS and Windows marked unverified** until a CI pipeline exists —
a platform matrix being fog (`map.md`, Not yet specified). Stating a worst-of-three would be the same unverifiable
claim dressed as rigour: the worst of three cannot be named without measuring three, and there is nowhere to
measure the other two. The precedent is ticket 28's refusal to assert a WCAG level, *"asserting an unverified one
is worse than stating what was done"*, and ticket 11's admission that macOS git compliance is measured rather than
documented.

**Every recorded number carries its environment** — CPU, RAM, filesystem, kernel. This repairs an existing hole:
neither `12-snapshot-write-path.md` ("Measured on 100k entries") nor `02-filesystem-watching.md` ("~10k watches and
1-4 s") records a host, so neither is reproducible today. The reference record names btrfs-on-LUKS explicitly, so
that a later run on ext4 is not mistaken for a regression.

**The fixture never lives on tmpfs.** `/tmp` is tmpfs on the reference machine; a fixture placed there measures RAM
— no `fsync` cost, no inode-cache behaviour — and returns flattering numbers silently.

### Fixtures

**A seeded generator, committed as code**, not a committed tree and not a clone of a public repository. A committed
100k-file tree puts the directory under observation inside this git repository, which confuses both the storage and
ticket 04's `.gitignore` layer; an external clone buys plausibility with a dependency that moves, after which
"regression" means "upstream reshuffled its files".

**Two fixtures, split on the layer boundary:**

- **Small** — ~5,000 files / 500 directories / 50 MiB, generated and measured in seconds. The tripwire's fixture: a
  gate that takes ten minutes is never run by hand, and by hand is the only way it runs until CI exists.
- **Ceiling** — 100,000 / 10,000 / 10 GiB, run at release. The outcome claim's fixture.

**The tree's shape is declared, not incidentally plausible**, each parameter tied to a resource already measured:
the file-size distribution (hashing cost), directory fan-out (the watch budget), and **content compressibility,
targeted at ~2.7×**. Measured on the reference machine: 1 MiB of `/dev/urandom` compresses to 1,048,907 bytes with
zlib -6 (+0.03 %) while 465 KB of this project's Markdown compresses to 171,529 (36.9 %, 2.71×). A generator
writing random bytes would overstate store growth by roughly 2.7× for a text tree, git deflating every blob.

**The generator emits a change stream as well as a tree.** Two outcome lines — coverage and growth — exist only
over time, and without declared churn (files per hour, at what size) there is nothing to measure.

**Degenerate cases are separate and small**, since they drown in the ceiling fixture: a path near the length limit,
a non-UTF-8 name (ticket 10), symlinks, empty directories (ticket 25), and a file past the 100 MiB cap producing
`Withheld`.

### Enforcement

**Thresholds carry a 2× headroom, are committed to the repository, and a breach fails `make bench` with a non-zero
exit.** The threshold is committed, never the measurement: measurements are noisy, thresholds are not. Changing one
is a separate commit somebody has to justify — the shape ticket 14 already chose for the API contract, where
`openapi.json` sits at the repo root behind a byte-for-byte `#[test]` so that drift becomes a reviewable diff.

The headroom is not a concession to noise. It is sized to the failures this design demonstrably has, every one of
them a multiple: losing the TREE cache, 0.162 s → 0.83 s (5.1×); strict object validation on an uncached index,
0.83 s → 10.19 s (12.3×); unordered index insertion, 0.86 s → 22.74 s (26.4×). None of these looks like "15 %
slower", while 15-20 % is ordinary run-to-run spread on a laptop that boosts and throttles. A tight threshold would
trade real breakage for false alarms, and a flapping gate is a disabled gate.

The outcome layer has no mechanism and cannot have one: it is a promise, so its check is a release-checklist item
and a published table of numbers carrying the per-platform "unverified" mark. `make bench` is run by hand until a
pipeline exists — the same honest position ticket 14 took with `make check-api` and ticket 28 with
`make check-ui-size`, and it decays silently for the same reason, which is why it is written down here.

### The tripwire set, and the rule that bounds it

**A tripwire line exists only where it protects a named invariant or mirrors an outcome line. No other reason
qualifies.** The rule is the decision: a set that grows "just in case" becomes the same decoration, only slower —
thirty microbenchmarks nobody reads and nobody repairs.

Lines protecting a performance-carrying invariant, each naming what it defends:

| Invariant | Established by | Cost of breaking it |
| --- | --- | --- |
| The baseline is seeded from the previous Snapshot's tree | ADR 0004 | 5.1× |
| Entries are applied in lexicographic order | ticket 12 | 26.4× |
| Strict object validation is off on an uncached index | ticket 12 | 12.3× |
| An excluded directory is never entered | ADR 0007 | walks `node_modules` |
| Include Rules are applied before watches are installed | ticket 02 | watch exhaustion, F-L2 |

Lines mirroring the outcome layer at small-fixture scale catch what no microbenchmark sees: a regression assembled
from parts each within the noise — an extra `fsync` per file, a second `stat` in the walk — whose total is twice as
slow.

**This layer's first job is a baseline, not a gate: `read_tree` must finally be measured.** Ticket 12 records that
no `git2` figure of any kind has ever been measured for this project (ticket 01's numbers are the `git` CLI) and
estimates `read_tree` at "low hundreds of milliseconds". If that estimate falls, this is exactly the case the
two-layer split exists for: the threshold is set from the fact, and the promise is revisited separately.

### Which existing decisions this would have changed

Three of five, which is the evidence that the level is set usefully rather than around existing code — particularly
as no daemon-side code exists.

- **Ticket 12 / ADR 0004**: a 5 s capture budget would have made measuring `read_tree` a precondition of the
  decision rather than a note after it. The decision would likely have survived, resting on a number.
- **Ticket 02**: recording the environment beside the number would have caught 133,264 being one machine's value,
  and the daemon-wide watch ceiling would have appeared there rather than here.
- **Ticket 06**: the 2× ratio turns "the ladder already bounds growth" from an assertion into something measurable,
  which is what decides whether an opt-in, default-off disk cap is safe.
- **Ticket 09**: the prototype measured weight (96.7 KiB gzip) and never time. No interaction-latency figure exists
  in any resolved ticket, though ticket 07 claims one is needed. A read-latency budget would have made opening a
  diff part of the prototype's exit criteria, where it was cheapest to measure.
- **Ticket 10**: the 100 MiB per-file cap is retroactively confirmed — at any plausible hashing rate it fits inside
  the 5 s capture — and would not have changed.

### Correction to this ticket's own framing

The question text said "ticket 06's disk cap pauses capture rather than filling the disk". Ticket 06 says the
opposite: *"Refusing to capture is explicitly rejected"* — the cap floors at the newest tier, overshoots, and
reports. Pausing capture at the cap is ticket 11's behaviour, and only when maintenance is unavailable. The
Root-ceiling analogy was resting on the wrong precedent; corrected above, the ceiling reports and degrades, which
is what ticket 06 actually does.

### No ADR

Two of the three criteria hold — a future reader would ask why 2× and why Linux-only, and both came from real
trade-offs — but irreversibility does not: a threshold changes with a one-file commit, which is the mechanism this
ticket deliberately chose. The decision lives in this answer and in the committed thresholds file.

Refines tickets 02, 06, 09, 12, 20 and 28.
