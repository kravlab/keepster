# 31 — Testing strategy for filesystem-heavy and time-dependent code

Type: grilling
Status: resolved

## Question

Three resolved tickets each settled a piece of this and each deferred the rest here, so the remaining question is
now statable.

What is already fixed:

- **No test doubles exist** (ticket 08): no `Clock` trait, no `FileSystem` trait, the only double in the workspace
  being a real git store in a temp dir. Policy is `decide(state, event, now) -> (Vec<Action>, PolicyState)` over
  values, so Retention, the include/exclude matcher and the Trigger logic are tested as pure values with no disk.
- **The web UI's share** (ticket 28): Vitest on non-visual logic kept in `.ts` modules — SSE reconnection, the
  `diffConfig.override` hunk feed, Restore's preflight — with no component tests and **no E2E in v1, deferred
  explicitly to this patch, with SSE reconnection named as its first candidate**.
- **Fixtures** (ticket 30): a seeded generator committed as code, a small fixture and a ceiling fixture, a declared
  change stream, and separate degenerate-case fixtures — a near-limit path, a non-UTF-8 name, symlinks, empty
  directories, a file past the 100 MiB cap.

Settle:

- **What tests the parts that cannot be values.** Ticket 02 established that every platform loses events, several
  losses being unannounced, and that Reconciliation is the guarantee. How is that guarantee tested — is a lost
  event injectable, or is the only honest test a real write the watcher provably never reported?
- **Whether ticket 30's generator is shared** between benchmarks and correctness tests, or whether they are
  separate rigs with separate lifetimes.
- **Time-dependent behaviour.** The Retention ladder spans 30 days and ticket 06's boundary invariant only acts on
  a Segment lying wholly past a tier boundary. `now` is a parameter in `core`, but `app` holds a real store: what
  proves a thinning pass that only occurs after a week of wall-clock?
- **The platform matrix for tests, distinct from CI's.** Several degenerate fixtures cannot exist on Windows at
  all; a test that silently skips there is a claim of coverage that does not hold.
- **E2E**: whether v1 has any, and if so what drives a browser against a real daemon.
- **The flake policy** for a suite that writes to real filesystems and spawns a real `git`.

## Answer

One rule generates most of what follows, and it is the rule this map has already applied twice — to performance in
ticket 30 and to platform behaviour in ticket 02: **a suite gates only what we promise, and reports everything it
merely observes about the environment.** Every timing-dependent, platform-dependent and machine-dependent
observation below is pushed into a report layer that fails nothing, which is what lets the gated layer be
deterministic enough to need no retry budget at all.

### Time is injected through the real capture path, never simulated

Ticket 06's ladder spans 30 days and only acts on a Segment lying wholly past a tier boundary, so the facts worth
testing in `keepster-app` are physical: a thinned Segment is genuinely re-chained, `git gc --prune=now` genuinely
reclaims, a parentless pin genuinely survives it, the disk cap genuinely measures *after* maintenance. None is
provable over values and none can wait a week.

**The aged store is produced by the code under test.** Ticket 08's "time is a parameter" is widened from a property
of use-case signatures into a workspace invariant: **nothing below `keepster-daemon` reads a clock**, and every
timestamp the store records — the commit's author and committer `git2::Time`, the period in a Segment's ref name,
the index row, the sidecar — derives from the passed `now`. `std::time::SystemTime::now` and
`git2::Signature::now` are therefore banned below the daemon through `disallowed-methods` in `clippy.toml`, because
a single `Signature::now()` deep in `store` silently un-tests the whole ladder and no reviewer would see it. A test
helper then plays a scripted `(now, mutation)` stream, and ticket 06's worst case — ~1,600 Snapshots, a month of
history — costs seconds, because nothing sleeps and the trees are tiny. Recorded as
[ADR 0012](../../../docs/adr/0012-no-clock-below-the-daemon.md).

Two alternatives were rejected. **Fabricating refs and backdated commits directly in test code** makes the rig a
second implementation of ADR 0001's invariants — leaf-only ref names, parentless Segment starts, the `SnapshotId`
trailer, normalised modes — so Retention would be proved against a store no daemon ever writes; this is the same
objection ADR 0004 raises against state that can drift, moved to the test side. **Compressing wall-clock** (a 1 s
Segment period, tiers in seconds) makes the suite sleep, never crosses the 24 h / 7 d / 30 d arithmetic the
boundary invariant is written in, and makes a test's duration the thing it measures.

### The guarantee is tested with the watcher switched off

This ticket framed the choice as "is a lost event injectable, or is the only honest test a real write the watcher
provably never reported". Both halves are wrong, and the second is the trap: there is no seam between `notify` and
`core::Event` to inject at, and *"the watcher did not report it"* is an assertion about absence, provable only by
an expired timeout. Gating that would freeze a platform's defect as our contract — and ticket 29 has already shown
those defects moving, `smbfs` on macOS and SMB 3.0 on Windows delivering what Linux does not.

So the roles are split the way ticket 02 split them:

- **Correctness tests install no watch at all.** A real tree mutation — including ticket 02's three unannounced
  losses (a populated directory moved in, a write through a second hardlink, an `mmap` write), plus renames,
  deletions and ticket 17's permission-only change — is followed by `Reconcile` with an injected `now`, and the
  resulting Snapshot's contents are compared exactly. Deterministic, untimed, identical on all three platforms.
- **Watcher tests assert presence only**, within a generous timeout: "this write eventually yields a Capture". That
  is the status ticket 02 gave the watcher — a latency optimisation — stated as a test rather than as prose.
- **A watcher conformance report**, run by hand per platform, records what the OS actually delivered, exactly as
  research 02 did locally. It gates nothing.

The decisive argument against one combined end-to-end test is that it stays green under the very bug the guarantee
exists to prevent: a Reconciliation that only scans directories the watcher told it about. With no watcher
installed, that bug fails immediately.

### One dev-only `keepster-testkit`, three layers, two lifetimes

Ticket 30 ordered a seeded generator committed as code; the history helper above is the same machine. The parts
that must *not* be shared are the lifetimes: a bench fixture is judged on statistical shape and ticket 30's
committed thresholds are calibrated to it, while a correctness fixture must gain a case the day a bug is found.
Sharing one artefact means that adding a degenerate case silently moves every measured number, after which a
threshold gets "adjusted" or the gate gets disabled — both failures ticket 30 already named.

`keepster-testkit` (`publish = false`, a `[dev-dependencies]` member of the workspace) therefore has three layers:

1. **Plumbing**: temp-dir store creation, the real-`git` probe, and the history builder that plays
   `(now, mutation)` streams through the real use-cases. Shared by everything.
2. **The bulk generator** of ticket 30, whose parameters live in a named, frozen bench profile. A `#[test]`
   compares a digest of those parameters against a committed one, so the fixture's shape cannot move without the
   thresholds moving in the same commit — the shape ticket 14 chose for `openapi.json`.
3. **The degenerate catalogue**: one explicit case per entry, free to grow, never touched by a benchmark, where it
   would drown anyway.

Ticket 08's dependency-graph rule gains one line: `testkit` may know everything, being dev-only, and the build fact
that the CLI binary links neither `git2` nor `rusqlite` is unaffected.

### The `/v1` contract gets an in-process harness, and only a process gets a process test

Ticket 14 gates the contract's *shape* (generated from `keepster-api`, byte-for-byte against the committed
`openapi.json`); nothing gates the running daemon's conformance to it. Uncovered today: the single-use bootstrap
exchanged for an `HttpOnly` cookie, the `Host`/`Origin` checks, SSE with `Last-Event-ID` and an honest `resync`,
`job.*` progression, the CLI exit-code table, and `status --json` as a verbatim passthrough.

**The main tier is an in-process harness**: the real `keepster-daemon` on an ephemeral loopback port and a
temporary socket or pipe, over real stores in temp dirs, driven by a real HTTP client — and by the real CLI
pointed at that socket, which is how ticket 07's "the CLI is a thin client" becomes a test instead of a claim. It
must be a real server rather than a `oneshot` service call, because cookie round-trips and SSE streaming are
HTTP-level behaviour; one platform detail follows from that and is cheaper to know now than to debug later:
`sun_path` holds 104 bytes on macOS and 108 on Linux, so the harness places sockets in a short directory, not a
deep temp path.

**A handful of tests spawn the real binary**, for what only a process can show: the flock'd pid file refusing a
second daemon, the shutdown drain and debounce flush on SIGTERM, CLI exit codes, and `service install` on the
reference platform. Spawning for *every* contract test was rejected: process start, readiness, ports and cleanup
are flake with no payoff, and ticket 30 established that a slow gate is one nobody runs by hand.

This also splits SSE cleanly in two without a browser: server-side replay belongs to the harness, client-side
reconnection to ticket 28's Vitest.

### One browser test, scoped to what only a browser can refuse

Ticket 28 deferred E2E here and named SSE reconnection first; the harness and Vitest now cover both halves of it,
and the embedded-asset surface of `keepster-ui` (MIME, caching, SPA fallback) is an ordinary `GET` against the
harness. What remains browser-only is that **a real browser accepts the bootstrap exchange and the SPA boots from
the embedded assets**: the cookie attributes on plain-http loopback, `SameSite` across the token navigation, a
first paint with no external request. A failure there breaks the entire web UI, and no Rust test can observe it.

So v1 has exactly one Playwright smoke test — bootstrap → cookie → screen → Snapshot → diff → the preflight dialog
appears — one browser, reference platform, run from the release checklist. One line, not a suite, under ticket 30's
rule that a check exists only where it protects a named invariant. A fuller suite across ticket 15's three view
modes and ticket 16's two themes was rejected as a manual gate nobody would run before ticket 32 exists, and "no
E2E at all" as leaving the one decision that is the browser's to make unobserved anywhere. The accepted cost is a
few hundred megabytes of downloaded browser and a check that decays exactly as `make bench` and `make check-api`
do.

### Three outcomes per (case, platform), and a generated table

A single "skipped" conflates two claims that must stay apart: **"impossible here" is not "never run here"**. FIFOs,
sockets, device files and ticket 17's full POSIX modes are Unix-only; `CON`, a trailing dot and the reserved `.git`
name are Windows-only; a non-UTF-8 name cannot be created through Win32 at all; and symlinks on Windows depend on
the *machine* (Developer Mode or the privilege), not on the platform.

Every (case, platform) pair therefore carries one of three declared outcomes: `ran`; `impossible`, compiled out by
`#[cfg]` with a recorded reason, so the decision is made at compile time rather than hidden in a runtime branch;
and `unavailable`, when a capability probe fails at suite start (Developer Mode, a missing external `git` per
ticket 08's accepted cost, a filesystem lacking a property), with the reason recorded. The table is **generated**
from the catalogue's metadata rather than maintained by hand, and published per platform with macOS and Windows
marked `untested` — the same mark ticket 30's outcome budget carries as `unverified`, for the same reason.

An ordinary run stays green on `unavailable`, since a developer is not obliged to enable Developer Mode; the
release checklist requires the table with no `unavailable` and no `untested` row among the cases possible on the
reference platform. A printed runtime skip was rejected: nobody reads `cargo test`'s stdout, so a month later
"never run on Windows" is indistinguishable from "green on Windows". Narrowing the catalogue to universally
reproducible cases was rejected outright — ADR 0009 defines `Withheld` through FIFOs, devices and unrepresentable
names, so that suite would omit the very invariant the product exists to uphold, repeating in the tests the silent
omission it forbids in the store.

### No retries; nondeterminism is moved, not retried

After the decisions above the gated suite has no timing dependency left: `now` is a parameter, correctness tests
install no watch, each test owns its store. The three places that remain timing-dependent are the watcher presence
tests (which are the report layer), the real-binary tests, and the browser smoke.

**`nextest` retries stay at zero.** Half the racy code here *is* the product — ticket 06's exclusive store lock,
ticket 12's capture coalescing against a held maintenance lock, ticket 07's `resync` — and a retry paints green
precisely the class of failure a user experiences as a lost update or a missing Snapshot. What must stay in the
gate and depends on time waits on an **observable state transition** (a `status` field, a `job.*` event, the pid
file appearing), never on a duration; a per-test timeout catches a hang, and an unbounded wait is a failure rather
than grounds for a second attempt. Testability here becomes a requirement on the daemon's own reporting rather
than a test trick, which is where ticket 05 already put it.

Two structural sources of flake are removed by construction rather than by tolerance. **Tests that install watches
run in a serial `nextest` test-group**, because `max_user_watches` is a per-user resource — the same counter ticket
30's daemon-wide watch ceiling reads — and a parallel suite turns one machine's limit into random failures. **A
missing external `git` is `unavailable`**, not a failure.

### Coverage is a report answering one question

`cargo-llvm-cov`, run by hand and at release, with no threshold. The only question it is used for is **which crate
has no test at all**, which is genuinely invisible in review across a ten-member workspace. A percentage gate was
rejected on ticket 30's own rule — it protects no invariant of this design — and because it would actively fight
two decisions above: it rewards tests on accessors and punishes the honest `#[cfg]` exclusions, where coverage
drops exactly where the matrix records `impossible`. A workspace mean over a pure `core`, a deliberately
thin `keepster-watch` and a `keepster-service` testable on one platform means nothing in any case.

### Surfaced here, deliberately not decided here

**What Reconciliation treats as evidence of change** is settled by no resolved ticket, and this ticket needs it as
a precondition: if the comparison is `(size, mtime)`, an edit of identical length inside one mtime tick is never
noticed — on a filesystem with coarse mtime granularity that window is a full second — while hashing every path
collides with ticket 30's 10 GiB ceiling and its 30 s warm Reconciliation budget. It is a product invariant, not a
test concern, so it becomes its own ticket:
[35 — What Reconciliation treats as evidence of change](35-reconciliation-change-evidence.md).

### Not a glossary matter

None of the vocabulary introduced here — the history builder, `keepster-testkit`, the conformance report, the
capability probe, `ran`/`impossible`/`unavailable` — belongs in `CONTEXT.md`, which is a domain glossary and holds
no implementation detail. The terms live in this answer.

Refines tickets 02, 06, 08, 12, 28 and 30.
