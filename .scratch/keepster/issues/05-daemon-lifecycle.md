# 05 — Daemon lifecycle and autostart on three platforms

Type: grilling
Status: resolved

## Question

A history daemon that is not running is worse than none, because the user believes they are covered. How does it
start, stay started, and report that it is alive?

Settle:

- autostart mechanism per platform: systemd user unit, launchd agent, Windows Task Scheduler or service — and
  whether Keepster installs it (`keepster service install`) or documents it;
- single-instance guarantee, and what a second invocation does;
- where the control socket or port lives per platform, and how CLI and UI discover it;
- behaviour when a watched path is missing, unmounted, or permission-denied: skip and keep going, or refuse;
- logging destination and level, and how the user answers "is it actually snapshotting my work right now?";
- shutdown: what happens to a snapshot in flight.

## Answer

**Scope: one daemon per OS user, running as that user, never elevated.** Linux uses a `systemd --user` unit with
`loginctl enable-linger` so coverage survives SSH disconnect; macOS uses a LaunchAgent; Windows uses a per-user
service where available, falling back to a Task Scheduler logon task. A machine-level daemon was rejected outright:
ticket 04 deliberately left `.keepster.toml` unrestricted (any cloned repository may set `ui.editor` and the
secret opt-in list), which is an accepted risk for a user process and a privilege-escalation vector for a root one.

**Autostart is installed by us, not documented at us.** `keepster service install | uninstall | status`
generates and enables the platform unit, and runs `enable-linger` on Linux (printing the exact command when it is
refused). The decisive argument is not convenience: it is that only code which installs the unit can *verify* the
unit, and "the daemon is running but will not come back after reboot" is the silent failure this ticket exists to
eliminate. Packaging may later enable the unit too, but the guarantee must not depend on packaging, which is still
unspecified on this map.

**Control channel: one HTTP API, two listeners.** The CLI talks over a Unix domain socket (Windows: a named pipe
ACL'd to the owning SID), the browser over 127.0.0.1 with the token already decided for the web UI. The socket is
protected by OS permissions rather than a secret, needs no discovery file, and cannot collide on a port; loopback
TCP is not user-scoped on any of the three platforms, so making it the CLI's transport would reduce the boundary
around a daemon that can read all history and overwrite working files to a single 0600 file. Paths:
`$XDG_RUNTIME_DIR/keepster/daemon.sock` (Linux), `$TMPDIR/keepster/daemon.sock` (macOS — per-user and
private; note the ~104-byte `sun_path` limit), `\\.\pipe\keepster-<SID>` (Windows). Same router, same handlers,
two listeners.

**Single instance: an OS advisory lock, not a socket file.** The daemon holds `flock`/`LockFileEx` on
`daemon.pid`; the pid *content* is informational only. A Unix socket left by a crash is stale precisely when the
lock is free and a connect is refused, and is unlinked before rebinding. A second `keepster daemon` exits
non-zero naming the live pid.

**Only the service manager starts the daemon.** When the socket is absent, the CLI asks the platform
(`systemctl --user start`, `launchctl kickstart`, `schtasks /run`) and waits for the socket; with no unit installed
it stops and points at `service install`. It never forks a bare daemon. This keeps exactly one *kind* of running
daemon — the supervised kind — so `service status` can never report health that will evaporate at logout.

**Path failures get their own Root state.** `Unavailable` (root missing, unmounted, or unreadable — nothing can
be captured) is distinct from `Degraded` (still watching, but not as promised), because the two demand different
actions from the user and a single state with a free-text reason cannot be sorted or alerted on. An Unavailable
Root is never auto-unregistered: identity is a surrogate id and the path a mutable attribute (map decision), so
a vanished path is transient and the History Store is kept; the daemon retries with backoff and re-attaches on
mount. Where only part of the tree is unreadable, the Snapshot is still taken and **marked incomplete**, listing
the skipped paths with their errno — an honest partial Snapshot beats no Snapshot, provided it never claims to be
whole. Refusing to capture an incomplete tree was rejected: one root-owned artefact or dead fuse mount would freeze
history for the whole Root, failing closed exactly when nobody is watching.

**Health: `keepster status` is the single authority, built from live daemon state, never from logs.** It
reports what is true now, exits non-zero whenever any Root is not `Watching`, and `--json` emits the same
payload the web UI renders as a banner — which makes it cheap to put in a shell prompt or a cron check. Rows:
daemon liveness and uptime, autostart enabled, linger, the git prerequisite (populated by ticket 11), then per
Root its state, last Snapshot age, Snapshot count and — where the startup Reconciliation walk is disabled per
ticket 04 — "coverage not guaranteed". Logs go to the platform's native sink (stderr → journald under systemd;
unified log or a rotated file on macOS; Event Log or a rotated file on Windows) at a configurable level, default
info, and exist to debug a specific incident. Making the log the source of truth was rejected: a dead daemon writes
nothing, which is indistinguishable from a quiet healthy one.

**The macOS logout gap is accepted and made visible.** A LaunchAgent dies at logout, so a logged-out Mac is
uncovered. Rather than hide it or close it with a root LaunchDaemon (which would need admin at install time, fork
the install path, and run before FileVault unlock), `status` carries a `linger` row on every platform, reading
`unavailable — coverage ends at logout` on macOS, and the install docs say the same sentence.

**Shutdown: bounded drain, then flush pending debounce.** On SIGTERM the daemon stops accepting Triggers, finishes
the in-flight capture through its ref update, then takes a Snapshot for every Root whose debounce window is
still counting down — bypassing the rate limit, because reboot is the likeliest moment for uncaptured work to
exist and those are the user's most recent edits. Reconciliation is skipped as too slow for a stop window. The unit
declares `TimeoutStopSec≈10s`; being SIGKILLed past it is safe, since a half-written capture leaves at worst an
unreferenced git object for maintenance to collect and a lagging index that the ticket-03 watermark repairs in
milliseconds at next start. A Restore in flight is never torn: temp file plus atomic rename means the working file
holds either the old bytes or the new ones.

**Rejected for v1**: proactive desktop notifications when a Root leaves `Watching`. They attack the ticket's
premise directly, but cost three more platform integrations (D-Bus, macOS UserNotifications with its signing
requirements, WinRT toast), deliver nowhere on the lingering headless daemon that is the main reason linger exists,
and invite the mute-then-silent-failure cycle. Recorded under Out of scope; revisit if the pull-only surface proves
insufficient.
