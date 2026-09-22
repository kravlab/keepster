# 36: Workspace skeleton, the daemon and `keepster status`

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

**What to build:** a user can run the daemon in the foreground, ask `keepster status` whether they are covered, and
get a machine-readable answer with a meaningful exit code — on a machine with no Watched Roots yet. This is the
thinnest complete path through every layer, and it puts the enforcement mechanisms in place before any behaviour
leans on them.

The workspace is the ten crates of the spec with their dependency rules expressed in Cargo, not in review: the core
is a pure decision kernel (no traits, no I/O, `std` plus `regex`), the CLI links neither the git nor the SQLite
dependency, and the daemon is the only crate that reads a clock. Status content follows the spec's daemon section
(build version, uptime, pid, autostart, linger, the maintenance row as "unknown" until its own ticket) with the Root
array empty.

- [ ] The ten-crate workspace builds with an explicit `rust-version` and a committed lockfile; `cargo build` needs
      no node
- [ ] `keepster daemon` serves the status and version resources over a permission-protected socket on Unix and a
      named pipe ACL'd to the owning user on Windows
- [ ] `keepster status` renders those rows for humans, `--json` emits the API payload verbatim, and the exit code is
      the spec's table (zero here, since no Root is not `Watching`)
- [ ] The version resource is served before authentication and carries the daemon build version, the API version and
      the full build hash
- [ ] A lint forbids reading the clock below the daemon crate (ADR 0012), and it fails the build when violated
- [ ] The dev-only testkit crate exists and provides the in-process harness seam: the real daemon over an ephemeral
      loopback port and a short-path temporary socket, driven by a real HTTP client and by the real CLI
- [ ] The merge gate runs formatting, lints, tests on Linux, Windows and macOS, the MSRV leg, and the licence and
      bans checks
- [ ] Tests assert the exit-code table and the `--json` passthrough through the harness, not through internal calls
