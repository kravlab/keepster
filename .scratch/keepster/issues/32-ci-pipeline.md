# 32 — The CI pipeline

Type: grilling
Status: resolved
Blocked by: 31

## Question

Three release gates are defined and **all three are manual**, each recorded as decaying silently if nothing runs
it: `make check-api` (ticket 14, `oasdiff breaking --fail-on WARN` against the last released tag),
`make check-ui-size` (ticket 28, eager ≤ 150 KiB / total ≤ 400 KiB gzip) and `make bench` (ticket 30, thresholds
with 2× headroom). Ticket 30 additionally parked the **platform matrix** here: the outcome budget is claimed on
Linux and marked unverified on macOS and Windows precisely because there is nowhere to measure them.

Settle:

- **The matrix**: which platforms build, which run tests, and which run nothing. Ticket 27 ships archives, deb,
  rpm and a Homebrew tap for three platforms, so building all three is not optional.
- **Which of the three gates becomes automatic, and where.** This is not uniform: `check-api` is deterministic and
  belongs on every PR; `check-ui-size` is deterministic but needs node; `check-bench` is **not** — ticket 30 fixed
  one reference machine on Linux, and a shared CI runner with noisy neighbours is not that machine. Either the
  benchmark gate stays manual, or "reference machine" is redefined, and the ticket should say which.
- **Node in the matrix.** Ticket 27 commits `ui/dist` so `cargo build` never needs node; ticket 08 refused to force
  node into the job that only runs core tests. Which jobs install it.
- **What `check-api` needs to work at all**: it measures against the last released tag, so the pipeline must have
  tags and a release process that creates them.
- **Release automation**, if any: what produces ticket 27's artifacts and checksums, and whether the Homebrew tap
  is updated by hand.
- **What blocks a merge versus what blocks a release**, given ticket 30 made the outcome budget a release-checklist
  item rather than a gate.

## Answer

**CI runs on GitHub Actions** (user decision), and one rule sorts every check that exists or will exist:

> **A check gates the merge if and only if its verdict is a property of the diff, not of the machine.**

The rule matters more than the list, because the list grows and without a criterion every addition becomes its own
argument. It also settles three questions this ticket asked separately: `make bench` cannot gate a merge (ticket 30
fixed one reference machine and a shared runner is not it), `cargo deny check advisories` cannot either (its
verdict changes with the advisory database, on unchanged code), and the browser smoke test cannot (its verdict
moves with the Chromium version the runner downloads).

### Why not Bitbucket Pipelines

The repository's remote is Bitbucket, so this was a real fork rather than a formality, and it closes on documented
facts:

- **Atlassian-hosted runners are Linux/Docker only.** Windows and macOS exist solely as self-hosted runners, which
  the `runs-on` step property states outright: *"Only available for self-hosted pipeline runners."* Ticket 27 needs
  Windows and macOS artifacts, and macOS must be built on macOS under the Xcode SDK agreement, so the three-platform
  matrix cannot exist on hosted Bitbucket infrastructure at all.
- **Build minutes**: 50/month on Free, 2,500 on Standard, 3,500 on Premium — a Rust workspace exhausts the free
  allowance in a couple of runs.
- **There is no GitHub-Releases equivalent.** Release hosting is repository Downloads: flat, keyed by filename, an
  upload replacing any same-named file; Pipelines artifacts expire in 14 days and cap at 1 GB.
- **`dist`, the tool ticket 27 already chose, generates GitHub CI only** — `github` *"is currently the only
  supported CI backend"* — and its Homebrew publish job requires the tap to be a GitHub repository with a
  `HOMEBREW_TAP_TOKEN`. Its artifact layer is portable (`dist build -a local|global`, `dist plan`, `dist manifest`,
  and `hosting = ["simple"]`), but the release plumbing ticket 27 leaned on assumes GitHub.

Two consequences of the copy being on GitHub while the canonical remote is Bitbucket, recorded as preconditions
rather than as decisions:

- **Nothing mechanically blocks a merge on Bitbucket.** Bitbucket merge checks read Bitbucket build statuses only,
  and an Actions run reports to GitHub; bridging would mean posting to Bitbucket's build-status API from a token.
  The merge gate below is therefore enforced where the pipeline runs and respected by discipline elsewhere.
- **Tags must reach the copy.** `make check-api` compares against the last released tag and `dist` attaches
  artifacts to a GitHub Release, so the release tag lives in the GitHub copy; a copy without tags leaves the
  compatibility gate comparing against the wrong baseline or vacuous.

### The matrix

| Leg | Runs |
| --- | --- |
| `ubuntu` x86_64 | build, test, `fmt`, `clippy -D warnings`, `check-api`, `cargo deny check licenses bans`, the UI job |
| `windows` x86_64 | build, test |
| `macos` arm64 | build, test |
| Linux aarch64 | build only |
| MSRV (Linux) | `cargo check --workspace` at the declared floor |

Tests run on three platforms **on every PR**, which is what removes ticket 31's `untested` rows; a private copy
would have forced the expensive legs (macOS ×10, Windows ×2 against 2,000 free minutes) onto merges to `main`
instead, and that shape stays available if the copy's visibility changes. `bench` runs on no leg.

**One distinction must not be lost**: this matrix verifies *correctness* on three platforms, and ticket 30's outcome
numbers stay **unverified** on macOS and Windows regardless, because a shared runner is not a reference machine on
any OS. CI removes ticket 31's `untested`, not ticket 30's `unverified`.

### What gates a merge, what gates a release, what runs on a schedule

**Merge gate** — every verdict a property of the diff: `cargo fmt --check`, `cargo clippy -D warnings`,
`cargo test` on the three platforms (which includes ticket 31's harness tier and its digest tests, plus ticket 14's
byte-for-byte `openapi.json` test), `make check-api`, `make check-ui-size`, the `ui/dist` freshness check, the
licence and bans halves of `cargo deny`, and the MSRV leg.

**Release** — every verdict a property of a machine, a browser or a place: `make bench` on the reference machine,
the smoke E2E, the generated platform table with no gaps, the outcome-budget review, the coverage report.

**Scheduled, gating nothing**: `cargo deny check advisories`.

### Node lands in exactly one job

Ticket 08 refused to force node into the job that only runs core tests, and nothing here reverses it: one UI job on
`ubuntu` installs node and pnpm and runs `svelte-check` strict, Prettier, ESLint, Vitest, `make check-ui-size` and
the freshness check. The three Rust legs never install node, which is also what ticket 27's committed `ui/dist`
exists to make possible.

**`ui/dist` freshness is a rebuild plus a byte-for-byte comparison.** It is the only mechanism that proves the
committed artifact matches the sources, which is the whole claim ticket 27 made when it committed a build output;
the price is that ticket 28's pnpm lockfile and its `engines`/`.nvmrc` floor become load-bearing for this gate.
**Declared fallback**: if Vite's output turns out not to be reproducible between a runner and a developer's
machine, the gate moves to a committed hash of the UI sources plus toolchain versions, generated by `make ui` — it
degrades, it is not switched off.

### Release automation

The `dist`-generated workflow, triggered by a `v*` tag: per-target archives, the shell and PowerShell installers,
checksums, a GitHub Release, and the Homebrew formula published to our own tap. Ticket 27's tier-2 deb and rpm are
separate steps (`cargo-deb`, `cargo-generate-rpm`) uploading into the same Release, since `dist` does not produce
either. **Tags are created by hand**, which is deliberate: a release is the one operation in this project that
should not happen because something merged.

### Two amendments to ticket 31

- **The smoke E2E becomes automatic but stays out of the merge gate**, running in the release workflow on the tag.
  Ticket 31 placed it on the release checklist *because no CI existed*, so the placement was reopened here; the rule
  above keeps it out of the gate on its own merits, since a Chromium update can fail it on unchanged code, and
  ticket 31's zero-retry policy makes a single browser-dependent gate the worst possible combination — a flake with
  no second attempt blocks unrelated work. On a tag it can at worst stop a release, which is proportionate. What
  changes is that it no longer decays silently, which is the property this ticket exists to remove.
- **The report layer is produced by CI on all three platforms**, published as run artifacts and gating nothing: the
  generated platform table, and the watcher conformance report. Reports cannot lie here because ticket 30 already
  requires every recorded number to carry its environment, so a row reading "FSEvents on a macOS 15 arm64 runner"
  does not pretend to describe a user's laptop. Whether `unavailable` rows appear on the runners — whether Windows
  symlink creation depends on the runner's privileges, for instance — is not a decision but a fact the first run
  records.

### MSRV is declared, therefore checked

Edition 2024 already requires 1.85+, and without `rust-version` in the manifests a user on an older toolchain gets
an edition parse error rather than a version message. Declaring a floor without checking it would produce exactly
the decorative number ticket 30 forbade, so one cheap Linux leg checks it — the same posture ticket 11 took when it
chose git 2.32.0 as a floor that excludes nothing still supported. `beta`/`nightly` legs were rejected: they fail
from other people's changes and teach the team to ignore red, and the public contract here is HTTP (`/v1`), not a
Rust crate API.

### `cargo deny` is split by the rule

`licenses` and `bans` gate the merge, with `deny.toml`'s allow-list derived from ticket 27's findings — that ticket
made licence expressibility a packaging constraint (`debian/copyright`, an SPDX expression in rpm, nixpkgs) and had
to clear `ksni`'s `Unlicense` by hand, which is precisely an invariant that now has a mechanical owner.
`advisories` runs on a schedule, because a database release would otherwise block an unrelated PR, and that ends as
either an ignore list or a disabled check.

### No ADR

The rule is quotable and load-bearing, but reversible by a one-file commit — the same reason ticket 30 declined one
for its thresholds. It lives here and in the workflow files.

Refines tickets 14, 27, 28, 30 and 31.
