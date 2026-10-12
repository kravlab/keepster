# 62: Logs — one rotated file per user, and a debug level that expires

**Blocked by:** 42 (Configuration — two levels, provenance, automatic reload, `Degraded`)

**Status:** ready-for-agent

**What to build:** when something goes wrong a user has a readable log in a known place on every platform, and a debug
level left on overnight can neither fill the disk the daemon is protecting nor push the useful lines out of the
window.

The platforms' native sinks lose either the content or the readability, so the log is our own rotated file in a
per-user directory; on Linux the journal still receives startup and fatal lines. Volume is bounded by size-triggered
rotation plus a retained-file count — no library caps total bytes, so a total is only ever "size times files" — and
the level is an operation with an expiry, which is the root-cause answer rather than the compensating one.

- [x] The daemon writes to a rotated file in the documented per-user directory on each platform, at the configured
      level, defaulting to informational
- [x] Rotation triggers on size, retains a bounded number of files, and the declared ceiling is a configuration key
- [x] Raising the level takes a duration and returns to the default by itself; an active temporary level is a row in
      `status`
- [ ] The rotation writer is covered by tests on all three platforms, including the boundary and the semantics of
      renaming an open file on Windows — *written and compiled for Windows; not yet run on Windows or macOS, so not ticked*
- [x] Lines are human-readable with structured fields by default, switchable to one JSON object per line by a global
      key, one file either way
- [x] The format and the ceiling describe the daemon, so they are never settable per Root
- [x] On Linux only startup and fatal lines reach the journal, so the volume is not duplicated

## Comments

- The log format and ceiling describe the daemon, so they are global-only and — if they cannot change live —
  `live: false` in `keepster_app::config::SCHEMA` (ticket 42); see ticket 60's Comments for what a daemon key
  still needs in the per-Root blocks.
- Implemented; open until the Windows rotation test has run on Windows. The writer is
  `keepster_service::rotating::RotatingFile`: rotation before a record that would take the file past its share
  of `log.ceiling` (a fifth: the current file and four rotated ones), never splitting a record, a record larger
  than a file alone in one. Rotation renames the current file first and shifts the older ones only once that
  worked, because on Windows a reader that forbids deletion makes the rename fail and the rotation is retried at
  every record — shifting first would have deleted every rotated file while the reader held on. Meanwhile the
  file grows past its size rather than lose a line.
- Keys `log.level` (default `info`), `log.format` (`text` or `json`) and `log.ceiling` (default 50 MiB, at
  least 64 KiB): daemon keys, global-only and restart-required. The file is `daemon.log` in
  `$XDG_STATE_HOME/keepster`, `~/Library/Logs/Keepster` or `%LOCALAPPDATA%\Keepster\logs`; Keepster's own
  records at the level, every other crate at warnings at most.
- The raise is `PUT /v1/log/level` `{level, for}` (a second to seven days, `for` required) and
  `DELETE /v1/log/level`; `status` carries `daemon.log` with `until` while a raise lasts. **The CLI is
  `keepster log-level debug --for 30m` and `keepster log-level --reset`, not ticket 33's `keepster log level`**,
  which `keepster log <path>` already parses as a path.
- Standard error carries one line at start and fatal ones (a failed start, a panic); the subscriber writes only
  to the file. A daemon in a test process writes no file — the subscriber is the process's — so the file, the
  format, the ceiling and standard error are tested on the binary, Unix only; the level's API on every platform.
- From review: a raise and an end now change the level under one lock, so neither can leave a level nothing
  ends; a raise must be more verbose than `log.level`; its end is checked against the wall clock (a monotonic
  timer stops while a laptop sleeps); a rotation whose last rename fails puts the current file back instead of
  deleting it, the shift stops at the first file a reader holds so a retry loses nothing, and a `.rotating` file a
  crash left between the two renames is put back when the log is opened.
