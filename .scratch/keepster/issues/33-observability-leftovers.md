# 33 — Log rotation and a diagnostics bundle

Type: grilling
Status: resolved

## Question

Ticket 05 settled the health surface and deliberately demoted logs: `keepster status` is the authority — live
daemon state, non-zero exit, `--json` — and logs exist for debugging. Ticket 21 added the tray as a second process
with its own failure modes, and ticket 30 added a `status` row for the daemon-wide watch ceiling. What was never
settled is what happens to the logs themselves, and how a user hands us the state behind a bug report.

Settle:

- **Where logs go on each platform**, given ticket 27's autostart mechanisms differ: a systemd user unit on Linux
  (journal, or our own file), a LaunchAgent on macOS, a Scheduled Task on Windows — where a task's stdout goes
  nowhere by default.
- **Rotation and retention**: size or age, whose job it is, and what stops a debug level left on overnight from
  filling the disk the daemon is supposed to be protecting.
- **The tray's logs**, which are a second process in a different session.
- **Whether a diagnostics/support-bundle command exists**, and if so what it may contain. This is the hard part: a
  bundle is most useful with paths, config and Root state, and every one of those is user data. Ticket 10's secret
  denylist keeps secrets out of *history*; nothing yet keeps them out of a support bundle, and a path is itself
  disclosure.
- **Whether anything is ever sent anywhere.** The map rules out any remote for history; a bundle that uploads would
  be the first network egress in the product.

## Answer

**One log mechanism on all three platforms, a support bundle that discloses by inventory rather than by redaction,
and no network egress ever.** Ticket 05's sketch — "the platform's native sink" on each OS — does not survive
contact with the platforms, and the bundle's privacy problem turns out to be solvable only by honesty, not by
filtering.

### Native sinks lose either the content or the readability

- **macOS.** The unified log **does not persist `info`** — which is ticket 05's default level — and never persists
  `debug`: Apple's table marks Debug "not persisted" and Info "Only when collected with the `log` tool". Its store
  size is documented only as "a predefined size", it is a binary format readable solely through
  `log show --predicate`, and where an unset-`StandardOutPath` LaunchAgent's output goes is stated by **no current
  Apple document** (TN2083 is archived and describes ASL, which Apple's own logging page declares superseded).
- **Windows.** A non-elevated process *can* write to the Application log — the `Eventlog` key's default SDDL grants
  BATCH LOGON read and write, which is the logon type a Scheduled Task uses — but without a registered message file
  "the Event Viewer cannot map any event identifiers … and will display an error", and the event-source registration
  lives under `HKEY_LOCAL_MACHINE`, i.e. needs the administrator that ticket 27 removed from installation.
  Microsoft documents **nowhere** what happens to a scheduled task's stdout, and gives no guidance at all on where
  an *unpackaged* Win32 app should write log files.
- **Linux** is the one platform where the native sink is strictly better: a user unit's `DefaultStandardOutput` is
  `journal`, so stderr is already captured, and `journald.conf` bounds the journal at 10 % of the filesystem capped
  at 4 GiB, for free.

**So the log is our own rotated file, in a per-user directory**: `$XDG_STATE_HOME/keepster/` (the XDG spec puts
"actions history (logs, …)" in `$XDG_STATE_HOME`, not in the cache), `~/Library/Logs/Keepster/`,
`%LOCALAPPDATA%\Keepster\logs\`. On Linux stderr keeps reaching the journal, so only startup and fatal lines go
there — the journal stays the place that answers "did it even start", without duplicating the volume.

**What decides it is the bundle.** A fixed-content bundle collected by our own code cannot afford a platform where
the logs live in a binary store behind another executable: Linux-journald plus files elsewhere would mean shelling
out to `journalctl` — a second external binary after ticket 11's `git` — and two reading instructions for one
product. The accepted cost is that we write rotation ourselves on Linux too, where journald would have done it.

### The volume bound, and the `debug` left on overnight

Two mechanisms, because the ticket's question has two halves and a byte cap answers only one.

**The bound**: rotation on size plus retention by file count, giving a declared ceiling (order of 50 MiB per
daemon, a key in the global config). There is no library alternative to expressing it that way — no Rust crate caps
total bytes on disk; every retention mechanism available is file count, file age, or a fixed window, so a total is
only ever "size × files".

**The level is an operation that expires**: `keepster log level debug --for 30m`, after which it returns to `info`
by itself, and an active temporary level is a row in `keepster status`. This is the root-cause answer rather than
the compensating one — "forgot to turn it off" stops being a reachable state — and it protects what a cap cannot:
a 50 MiB budget filled with a night of `debug` retains not one line from the period being asked about. Ticket 21's
rule that a muted channel must be reported applies unchanged to a raised level: a state that changes behaviour may
not be silent.

### The rotation carrier is ours, deliberately

`tracing` + `tracing-subscriber` as the facade (the ecosystem default, and the only one that keeps the structured
fields `status` and the SSE stream already trade in), with a **hand-written `MakeWriter`** for size-triggered
rotation and count-based retention. This is a conscious departure from "prefer an established library", on two
grounds. `tracing-appender` cannot do it at all — its `Rotation` offers only `MINUTELY`/`HOURLY`/`DAILY`/`WEEKLY`/
`NEVER`, `max_log_files` prunes by count and only at a rotation boundary, so `NEVER` never prunes. Of the crates
that can, `flexi_logger` (alive, `Criterion::Size`, `Cleanup::KeepLogFiles`) is a whole logging framework carrying a
second level system and a second configuration surface into a product whose configuration is closed by ticket 04
and whose level switching this ticket just made an expiring operation we must own; `tracing-rolling-file` is the
right shape but 0.1.x, single-maintainer and idle for over a year; `file-rotate` has no `MakeWriter` and no release
in 19 months. The requirement is narrow and fully stated above, so we own it — and the cost is named: rotating an
open file has different semantics on Windows, and the boundary is a race, so this is one of the first things
ticket 31's three-platform matrix must cover.

### The tray logs to its own file

Same mechanism, own file beside the daemon's, own smaller ceiling; the bundle collects both. The level comes from
the same global config key and a temporary raise reaches the tray over the event stream it is already subscribed
to, falling back to the last known level when the daemon is unreachable.

Routing the tray's records through the daemon over `/v1` was rejected on its own primary failure mode: the tray's
most valuable line is "the daemon died", which is exactly when the transport is gone. Logging nothing was rejected
because ticket 21's "a view, never a source" governs *displayed state*, not diagnostics — the tray's own failures
(an icon stock GNOME cannot draw, a dead D-Bus, an unsigned `.app`) are derivable from nothing else.

### `keepster diagnose`

A local zip — the format export already produces (ticket 19) — with **fixed content and no selection flags**, and
`--dry-run` printing the inventory before anything is written, which is the preflight discipline tickets 10 and 19
already established. Contents: `status --json` verbatim; the three versions (build plus full hash, API, store
format); the environment (OS, the filesystem type under each Root, `max_user_watches` against ticket 30's
daemon-wide ceiling, the `git` discovery result and the last maintenance pass's stderr from ticket 11); the
effective configuration with per-key provenance; the Registry (ids, paths, states); per-Root store topology
(Segment, Snapshot, ref and pin counts, size, the oldest and newest Snapshot, the format versions of ticket 26);
the index watermark; the tails of `daemon.log` and `tray.log`; and the **inventory** itself — every file, its
source, and one line on what it discloses.

**The hard line: no content of any file from history, under any flag.** There is no debugging reason to ship blobs.

**Selection flags were rejected** because a trimmed bundle looks exactly like a complete one and a bug report then
begins by establishing what is missing; predictability is worth more than flexibility here. A **minimal** bundle
without configuration and Registry was rejected because it removes the most common defect class in this product —
Include Rules not matching what the user expected, the very question ticket 20's Preview and ticket 07's
`explain?path=` exist to answer.

**Privacy is handled by inventory, not by redaction** — see
[ADR 0013](../../../docs/adr/0013-bundle-discloses-by-inventory.md). Paths go in as they are; the command shows
exactly what it collected; the decision to share stays the user's.

**Nothing is ever sent anywhere.** This is recorded as a product invariant rather than as an implementation
property: the map rules out every remote for history, and the bundle is the one place where egress would have
looked natural.

### Who collects it, and what a dead daemon yields

**The daemon collects it over `/v1`**; when the daemon is unreachable the **CLI collects a degraded bundle** from
files alone — logs, the configuration, the Registry (plain per-Root files by ticket 03), the environment, its own
build version — and the inventory names the sections that are missing (`status`, store topology, the index
watermark). Ticket 08 forbids the CLI to link `git2` or `rusqlite`, so store topology is unreachable without the
daemon, and that is the section least needed when the daemon will not start. "Only the daemon collects it" was
rejected because it fails in the case the command exists for, which is the failure ticket 05 named in its opening
line.

### The line format

**One file. Human-readable lines with `key=value` fields by default, switchable to JSON Lines by a global config
key.** The default is what Q1's and the bundle's whole posture rests on — a user who cannot read the log cannot
decide what they are sharing — while the override serves whoever wants to pipe it into something. Two consequences
are recorded rather than left implicit: the bundle's inventory states which format the log is in, and a JSON log
reaches the same ceiling sooner, since field names repeat on every line. Like every other key here, the format and
the ceiling **describe the daemon**, so by the class test of ticket 24 they live in the global configuration only
and are never settable per Root.

Refines tickets 04, 05, 08, 21 and 30.
