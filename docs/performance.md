# Performance

Two layers with different audiences (ticket 30). The **outcome budget** states what keepster promises; it is
reviewed at release and published below per platform. The **operation tripwire** promises nothing: it catches
regressions mechanically, and `make bench` fails when a gated line breaches its committed threshold.

## The outcome budget

At the Root ceiling v1 claims: 100,000 included files, 10,000 included directories and 10 GiB of included content.
A number is published only with the environment it was measured in; until one is, a platform's cell says how it
will be measured or that it is unverified. `make bench-ceiling` compares the p95 of its runs with each budget it
measures.

| Line | Budget | Linux | macOS | Windows |
| --- | --- | --- | --- | --- |
| Capture, the daemon's own contribution | ≤ 5 s p95 at ceiling | measured at release by `make bench-ceiling` | unverified | unverified |
| Fetch one version of a file | ≤ 100 ms p95 | measured at release by `make bench-ceiling` | unverified | unverified |
| Diff two versions, file ≤ 1 MiB | ≤ 250 ms p95 | measured at release by `make bench-ceiling` | unverified | unverified |
| A path's chronology | ≤ 100 ms p95 | measured at release by `make bench-ceiling` | unverified | unverified |
| Startup Reconciliation | ≤ 30 s at ceiling, warm page cache | measured at release by `make bench-ceiling` | unverified | unverified |
| Capture Preview | first row ≤ 200 ms, then ≥ 20,000 paths/s | measured at release by `make bench-ceiling` | unverified | unverified |
| Idle CPU | ≤ 0.5 % averaged over 10 min, no changes | measured by hand at release | unverified | unverified |
| Idle RSS | ≤ 150 MiB with three Roots at ceiling | measured by hand at release | unverified | unverified |
| Capture peak RSS | ≤ idle + 40 MiB | measured by hand at release | unverified | unverified |
| Steady-state store size | ≤ 2× the Root's included content | measured at release by `make bench-ceiling` | unverified | unverified |

Linux is the reference platform: the tripwire compares run against run, and a delta needs one base. macOS and
Windows stay unverified: the bench runs on Linux only, and a shared CI runner is not a reference machine on any
platform (ticket 32), so a worst case across three platforms could not be measured, only claimed.

Coverage latency is the daemon's own contribution, not the total: the total is the debounce plus the rate limit
(ticket 04), which is the user's configuration. Reconciliation on a cold page cache is not claimed: it is the
disk's speed, not keepster's.

The ceiling run also checks one estimate rather than a budget: reading the previous Snapshot's tree
(`capture.read_tree`) against ticket 12's "low hundreds of milliseconds", taken as at most 500 ms. Its report says
whether the estimate held and what share of the capture budget it takes.

## The operation tripwire

A line exists only where it protects a named invariant or mirrors an outcome line (ticket 30); the registry is
`crates/keepster-testkit/src/lines.rs`, and its type allows no third reason. The small run holds a gated line's
median to its committed threshold, twice what the reference machine measured, and records a baseline without
judging it. A gate's report prints, beneath each baseline, why it is not gated.

| Line | Exists for | Small run | Ceiling run |
| --- | --- | --- | --- |
| `capture.first_uncached` | strict object validation is off on an uncached index (ticket 12) | baseline: the invariant is not in the code yet (ticket 87) | — |
| `capture.step` | Capture, the daemon's own contribution | gated | ≤ 5 s |
| `capture.read_tree` | Capture, the daemon's own contribution | baseline: measured for the first time | baseline, checked against ticket 12's estimate |
| `store.growth` | Steady-state store size | gated | ≤ 2× |
| `read.version` | Fetch one version of a file | gated | ≤ 100 ms |
| `read.diff` | Diff two versions, file ≤ 1 MiB | gated | ≤ 250 ms |
| `read.chronology` | A path's chronology | gated | ≤ 100 ms |
| `reconcile.unchanged` | Startup Reconciliation | gated | ≤ 30 s |
| `preview.first_row` | Capture Preview, first row | gated | ≤ 200 ms |
| `preview.walk` | Capture Preview, throughput | gated | ≥ 20,000 paths/s |
| `capture.one_changed_path` | the baseline is seeded from the previous Snapshot's tree (ADR 0004) | baseline: breaking the invariant cannot breach it | — |
| `reconcile.excluded_subtree` | an excluded directory is never entered (ADR 0007) | gated | — |
| `watch.excluded_subtree` | Include Rules are applied before watches are installed (ticket 02) | gated | — |
| `capture.first_flat_tree` | entries are applied in lexicographic order (ticket 12) | baseline: breaking the invariant cannot breach it | — |

Each invariant the code holds was broken in an uncommitted change to see whether its line breaches. The two
exclusion lines did, and are gated. The other two did not: an unseeded index and an out-of-order insertion each
cost too small a part of a whole capture, at any size a gate can afford, so those lines are baselines that record
the capture and say why in their reason. A capture is still gated: `capture.step`, each step of the small profile's
change stream, mirrors the Capture outcome line at small scale.

`store.growth` is the whole store after the change stream and a Maintenance Pass, the derived SQLite index beside
the git store included, over the content the Root includes.

### Fixtures

A seeded generator writes every fixture (`crates/keepster-testkit/src/bulk.rs`); none is committed. Each profile
declares its file count, directories, bytes, size distribution, fan-out, excluded subtree, compressibility and
change stream:

- `small`: ~5,000 files, 500 directories, 50 MiB: the main fixture of the gate;
- `flat`: 20,000 empty files in one directory, for the order line;
- `exclusion`: a small tree beside a large excluded subtree, for the two exclusion lines;
- `ceiling`: 100,000 files, 10,000 directories, 10 GiB: the outcome budget's fixture.

`crates/keepster-testkit/bench/thresholds.toml` holds each profile's digest beside the thresholds. A test compares them, so a profile cannot
change without the thresholds file changing in the same commit, and the gate refuses to run against a profile its
thresholds were not calibrated on.

The degenerate cases (a name at the length limit, a name that is not UTF-8, symlinks, empty directories, a file past
the size cap) are a separate small catalogue, `crates/keepster-testkit/src/degenerate.rs`, about correctness. The
tripwire never measures them.

## Running it

| Command | What it does |
| --- | --- |
| `make bench` | the gate: the small fixtures, each gated line against its committed threshold |
| `make bench-calibrate` | rewrites the thresholds file: each gated line's limit as twice its median, rounded up to two significant digits, and every profile's digest; on the reference machine only |
| `make bench-ceiling` | the ceiling fixture against the outcome budget; gates nothing |

The bench runs on Linux only, the reference platform, and refuses to measure anywhere else. It exits 0 when nothing
breached, 1 when a gated line breached, and 2 when it refused to measure or could not finish; `make` turns both 1
and 2 into its own 2, and the tripwire's last line says which. It refuses a fixture directory on a memory-backed
filesystem, which would measure RAM rather than a disk: fixtures are written beneath `KEEPSTER_BENCH_DIR`, or the
build's temporary directory when that is unset. Every report starts with the profiles' digests and the environment
it measured in, and is written to `target/bench/report-<run>.txt`.

A threshold changes only in a commit of its own that says why it moved.

## At release

The platform table, watcher conformance and coverage reports have their own checklist in
[reports.md](reports.md#at-release).

- [ ] `make bench` passes on the reference machine
- [ ] `make bench-ceiling` runs on the reference machine; every outcome line within its budget, or the breach
      reported, and whether `capture.read_tree`'s estimate held
- [ ] Idle CPU, idle RSS and capture peak RSS measured by hand against their budgets
- [ ] The outcome table above updated with the numbers and the environment they were measured in
