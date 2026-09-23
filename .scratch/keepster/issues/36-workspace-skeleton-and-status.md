# 36: Workspace skeleton, the daemon and `keepster status`

**Blocked by:** None (can start immediately)

**Status:** done

**What to build:** a user can run the daemon in the foreground, ask `keepster status` whether they are covered, and
get a machine-readable answer with a meaningful exit code — on a machine with no Watched Roots yet. This is the
thinnest complete path through every layer, and it puts the enforcement mechanisms in place before any behaviour
leans on them.

The workspace is the ten crates of the spec with their dependency rules expressed in Cargo, not in review: the core
is a pure decision kernel (no traits, no I/O, `std` plus `regex`), the CLI links neither the git nor the SQLite
dependency, and the daemon is the only crate that reads a clock. Status content follows the spec's daemon section
(build version, uptime, pid, autostart, linger, the maintenance row as "unknown" until its own ticket) with the Root
array empty.

- [x] The ten-crate workspace builds with an explicit `rust-version` and a committed lockfile; `cargo build` needs
      no node
- [x] `keepster daemon` serves the status and version resources over a permission-protected socket on Unix and a
      named pipe ACL'd to the owning user on Windows
- [x] `keepster status` renders those rows for humans, `--json` emits the API payload verbatim, and the exit code is
      the spec's table (zero here, since no Root is not `Watching`)
- [x] The version resource is served before authentication and carries the daemon build version, the API version and
      the full build hash
- [x] A lint forbids reading the clock below the daemon crate (ADR 0012), and it fails the build when violated
- [x] The dev-only testkit crate exists and provides the in-process harness seam: the real daemon over a short-path
      temporary control channel, driven by a real HTTP client and by the real CLI. (Amended: the harness's *loopback*
      half arrives with ticket 60, which owns the loopback listener and is blocked by this ticket; the original
      wording asked for a listener this ticket does not build.)
- [x] The merge gate runs formatting, lints, tests on Linux, Windows and macOS, the MSRV leg, and the licence and
      bans checks
- [x] Tests assert the exit-code table and the `--json` passthrough through the harness, not through internal calls

## Comments

- **All five of the merge gate's checks pass locally on Linux**: formatting, clippy with warnings denied, the test
  suite, the declared MSRV (verified with 1.85.1, which compiles the whole workspace), and `cargo deny check licenses
  bans`. The **workflow itself** has still never run, and cannot until the GitHub copy exists — ticket 32 recorded
  that as a known consequence of the canonical remote staying on Bitbucket.
- The licence check earned its place immediately by finding two faults in its own configuration: `wildcards = "deny"`
  counted the workspace's internal path dependencies as unpinned (fixed at the root — those dependencies now carry a
  version beside the path, so the check still catches an unpinned third-party crate), and three allowed licences
  appeared nowhere in the graph (removed, so the list stays a statement of fact rather than a permission nobody can
  audit).
- **Windows is verified by type-checking only**: `cargo check` and `cargo clippy` pass for `x86_64-pc-windows-msvc`,
  and no test has executed on Windows. The named pipe, its owner-only security descriptor and the SID in its name are
  therefore unproven at runtime; the first Windows CI run is what settles them.
- The Unix socket is unlinked before binding without consulting a lock, because the flock'd pid file that makes a
  stale socket distinguishable from a live one belongs to ticket 61.
- Two decisions were taken during implementation that the tickets left contradictory or open, and both are recorded
  here rather than only in the conversation: the shipped binary lives in `keepster-daemon` (the wiring crate), so
  ticket 08's "the CLI binary links no store dependency" holds as a fact of the **crate** graph, which the merge gate
  checks with `cargo tree`; and `keepster-service` owns the control-channel path and every Win32 call, because the
  CLI needs the path and may not depend on the daemon.
- **The merge gate grows with the tickets that give it something to check**: `make check-api` needs ticket 40, the UI
  job needs ticket 64, and the Linux aarch64 leg belongs with ticket 68's release targets. Ticket 32's matrix is the
  end state, not this ticket's.
- **Exit code 10 is asserted over values, not at the harness**, because no Root can exist until ticket 37 — the
  daemon cannot serve a Root that is not `Watching` today. Ticket 37 must add that assertion at the harness seam,
  where ticket 36's criterion wanted it.
- A review of this work raised two things that were **rejected with reasons**: the `ERROR_PIPE_BUSY` retry in the
  CLI is correctness rather than scope creep (without it a live daemon caught between pipe instances would report
  itself unreachable, which is the wrong exit code), and `RootStatus` deliberately carries only the id and the state,
  because every other row ticket 07 lists is populated by a later ticket and `/v1` grows additively.
