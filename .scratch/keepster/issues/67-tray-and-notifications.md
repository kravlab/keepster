# 67: The tray process, and notifications when the insurance stops

**Blocked by:** 45 (The live event stream), 61 (Autostart on three platforms, and one supervised daemon)

**Status:** ready-for-agent

**What to build:** a user sees at a glance that they are covered, and is told when they stop being covered — without
being told anything about routine work.

The tray is a **separate process** in the user's session on all three platforms (ADR 0008): on Windows a per-user
background process is Microsoft's own documented remedy for a non-interactive window station, and on macOS
notifications need a bundle identifier, so it is an app bundle. It is an ordinary API client over the CLI's socket and
is governed by one invariant — **a view, never a source**: everything it shows is derivable from the status payload
and the event stream, and no notification carries a tray-only action.

- [ ] The tray shows live state from the status payload and the event stream, never from its own bookkeeping, and
      offers no action the API does not already expose
- [ ] Alarms fire only when the insurance stops — a Root `Unavailable` or `Degraded`, capture halted at the disk cap,
      an unexpected daemon death, and a failed user-initiated job — on transition, coalesced, with recovery replacing
      rather than adding
- [ ] Muting is reported by `status`, so it never becomes a silent failure
- [ ] The icon and the notification channel are two capability rows in `status` that fail independently, and neither
      affects the exit code
- [ ] On Linux the tray notifies even where the desktop cannot draw its icon
- [ ] The daemon decides, the tray delivers: no notification originates in the tray
- [ ] The tray writes its own log file with its own ceiling, and takes its level from the event stream, falling back
      to the last known level when the daemon is unreachable
- [ ] The CLI gains the restart, start and stop service verbs
- [ ] The tray's autostart is installed by the install command, which reports honestly on a machine with no graphical
      session

## Comments
- **From ticket 61.** `keepster_service::autostart::{start, autostart}` is the "ask the service manager"
  implementation the tray's start offer and the CLI's `service start` should share; `restart` and `stop`
  have no platform function yet. Once `keepster service restart` exists, name it in the listener-off
  refusal (`loopback.rs`, ticket 60), which says "restart the daemon" today. The Windows task runs the
  console binary, so a window appears at logon until the daemon gets a windowless entry point.
- **From ticket 62.** The tray "takes its level from the event stream", but no event says a raise began or
  ended yet: `LogLevels::raise` and `end` in `keepster-daemon/src/logging.rs` are where one belongs. The
  writer to reuse for `tray.log` is `keepster_service::rotating::RotatingFile`, with its own smaller ceiling.
- **From ticket 63.** `keepster diagnose` collects only `daemon.log` and its rotations today. Once the tray
  writes `tray.log`, the bundle must hold its tail too: add it beside the daemon's in
  `keepster-daemon/src/diagnostics.rs` (`log_tail`, and `on_disk` for the bundle collected without the
  daemon), with its own line in the inventory in `keepster-daemon/src/bundle.rs`.
