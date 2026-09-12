# 21 — Tray icon, and whether the daemon may push notifications

Type: grilling
Status: open

## Question

A tray icon for launching the web UI and running the common daemon commands — and, because it is the same platform
integration living in the same process, whether Keepster may push desktop notifications at all.

Ticket 05 recorded "proactive desktop notifications" under Out of scope, arguing they "deliver nowhere on the
lingering headless daemon that is the main reason linger exists". A tray is precisely the place they would deliver
to, so that argument no longer holds — and the rejection was never confirmed as a user decision, only written into
ticket 05's answer. The map's Out of scope line has therefore been removed and the question is live here.

Settle:

- **What the tray is, structurally.** A separate process with a windowing event loop, started by the desktop
  session rather than by the service manager that starts the daemon (ticket 05: `systemd --user` with linger, a
  LaunchAgent, a Windows per-user service). v1 then stops being "one binary plus an embedded SPA" and becomes two
  binaries — which the still-unspecified packaging question inherits, and which must not break ticket 08's rule
  that `cargo build` never needs node.
- **Whether it is optional, and provably so.** Ticket 05 made health a pull model on purpose: `keepster status` is
  the single authority, built from live daemon state. If any state becomes visible only in the tray, that
  authority quietly stops being one. Decide what the tray is forbidden from being the sole channel for.
- **What it does when the daemon is not running** — the very state it exists to surface. Start the daemon through
  the service manager (ticket 05 permits only the service manager to start it), show a dead state, or exit?
- **Which commands it exposes, and over which listener.** It is a client of the `/v1` contract like any other
  (ticket 07), but the CLI's transport is a permission-protected socket or named pipe and the browser's is loopback
  plus a per-start token — and a tray is neither a CLI nor a browser. Does it shell out to the CLI, or get a
  listener of its own?
- **Notifications: which events, if any, are worth pushing**, and what breaks the mute-then-silent-failure cycle
  ticket 05 named. The candidates are `Unavailable`/`Degraded` transitions (ticket 05) and capture paused at the
  disk cap while maintenance is unavailable (ticket 11).
- **Linux is the hard platform.** The modern channel is StatusNotifierItem over D-Bus, and stock GNOME appears to
  need an extension for it — a fact to establish from primary sources when this ticket is worked, not to assume. If
  a large share of the Linux desktop cannot show a tray at all, the platform matrix is not "all three" and the
  feature's value changes.
- **macOS specifics**: a menu bar item, its signing and notarisation requirements, and whether the LaunchAgent
  logout gap (ticket 05, shown rather than hidden) becomes easier or harder to communicate with one.
- **Whether it earns its keep.** If the tray only opens the SPA and runs three commands, the honest alternative is
  a desktop entry plus `keepster status` in a shell prompt. The answer should say what the tray does that those
  cannot.
