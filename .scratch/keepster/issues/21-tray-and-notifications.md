# 21 — Tray icon, and whether the daemon may push notifications

Type: grilling
Status: resolved

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

## Answer

**The tray ships in v1 on all three platforms, as a separate process that is simultaneously a presence indicator,
a command menu and the sole carrier of desktop notifications. Ticket 05's Out-of-scope line on proactive
notifications is reversed — but the tray is a view and never a source, so nothing it shows or raises may be
reachable only through it.**

### It earns its keep on all three counts, not one

The tray does three jobs and the user asked for all of them: an at-a-glance indicator that capture is alive, a
one-click menu for the handful of common operations, and — the one no `.desktop` entry can replace — a channel
that speaks up when the insurance has stopped. That third job is what makes the whole thing worth a second
process: this tool's real failure mode is not "the UI is awkward to launch", it is "it silently stopped capturing,
and I found out when I went looking for history that isn't there".

### Structurally: one extra process, the same on every platform

Recorded as `docs/adr/0008-tray-is-a-separate-process.md`, because the three platform facts behind it are
invisible from the code and the question a future reader asks is architectural, not tray-shaped.

**On Windows there is no choice, and the reason is worth recording.** Ticket 05 registers the daemon as a Windows
per-user service. A per-user service instance does run under the signed-in user's token in that user's logon
session — but the SCM still assigns a service process running under a user account the window station
`Service-0xZ1-Z2$`, and Microsoft states plainly that "all other window stations are noninteractive, which means
they cannot display a user interface or receive user input"; only `WinSta0` can. Microsoft never writes the single
sentence "a per-user service cannot show a tray icon", so this is an inference from two documented rules — but it
is the same inference behind Microsoft's own documented remedy for this exact case: *create a separate process in
the interactive user's session and talk to the service over IPC.* Which is precisely this design.

**On macOS the menu bar item itself is free** — `NSStatusBar::systemStatusBar()` needs no bundle and no
`NSApplication`, only a run loop on the main thread, and a LaunchAgent already defaults to `LimitLoadToSessionType
= Aqua`, i.e. the GUI session. Notifications are what force a bundle (below).

**On Linux the daemon could technically do it itself**, and uniformity wins anyway: a design where the icon lives
inside the daemon on one platform and outside it on two others buys one saved process and pays with a permanent
branch in every lifecycle discussion. Worse, a process holding a windowing event loop that survives logout is a
contradiction of exactly the kind ticket 05's linger decision exists to avoid.

Consequences: ticket 08's workspace gains a ninth crate; the tray contains no SPA, so "`cargo build` never needs
node" survives; packaging inherits a second artifact; and the tray is autostarted by the **desktop session** (XDG
autostart, an `Aqua` LaunchAgent, the `Run` key), never by the service manager, which still starts only the daemon.
`keepster tray install | uninstall | status` mirrors `service`, over the same platform module rather than a second
copy of the calls. **A machine with no tray installed is a normal machine**, not a degraded one.

### Transport: the CLI's socket, because a live indicator cannot poll

The tray is neither a CLI nor a browser, and it gets no third listener. It uses the **permission-protected
socket / named pipe**, whose authority is "the same OS user" — which the tray satisfies by construction, being
started by that user's desktop session. It is then an ordinary client of the `/v1` contract ticket 07 declared fit
for third-party UIs. What settles it is the indicator: keeping an icon current by polling is what ticket 07
rejected as trading latency against constant wakeups of a daemon ticket 06 told to wait for idle. The tray
subscribes to the SSE stream — which ticket 07 already guarantees works identically over the socket — and
redraws from `root.state_changed`, `store.over_cap`, `snapshot.created` and `job.*`.

### The invariant: a view, never a source

Every state the tray displays and every alarm it raises must be derivable from `GET /v1/status` and the event
stream, and **no notification carries an action reachable only from the tray**. Otherwise ticket 05's "`keepster
status` is the single authority on health" quietly stops being true and a headless machine loses information
rather than convenience. This invariant is what forces the CLI addition below, and what keeps the tray's own
health rows out of the exit code.

### When the daemon is not running

The tray shows "stopped" and offers to start it **through the service manager** — `systemctl --user start`,
`launchctl kickstart`, `sc start` — using the same implementation as `keepster service`, so ticket 05's rule that
only the service manager ever starts the daemon stays literally true and no unsupervised daemon can appear behind
the pid file's back. Exiting when the daemon is unreachable was rejected outright: the icon would vanish exactly
when it should be shouting, and its absence is indistinguishable from a tray that failed to autostart.

"Service never installed" is a third state, distinct from "stopped", and it offers installation rather than a
start. Detection is a failed socket connection followed by reconnection with growing backoff, never a poll loop; a
dropped event stream reuses ticket 07's `resync` path.

### Which events notify, and what breaks the mute cycle

Only **the insurance stopping**: a Root entering `Unavailable`; a Root entering `Degraded`; capture halted at the
disk cap because maintenance is unavailable without an external `git` (ticket 11); the daemon dying other than by
command. Plus exactly one addition: **the failure of a user-initiated job.** `forget` is irreversible and ticket 07
made it a job precisely because a dropped connection loses the outcome, so a notification is the only delivery
that survives a closed window. Success notifies nothing. `snapshot.created` never notifies — that is the machine
that manufactures muting.

Ticket 05 named the mute-then-silent-failure cycle without breaking it. It is broken by frequency, not by settings:

- **Transitions, not states.** An alarm fires on entry and never repeats while the condition holds.
- **Coalescing.** One unmounted disk holding five Roots is one notification.
- **Recovery replaces rather than adds.** All three platforms support replacing a notification by identifier, so
  the loop closes without a second interruption.
- **Muting is possible but never invisible.** Because the tray is a view, `keepster status` reports the state of
  the push channel — Keepster's own configuration key, the platform's authorization where it is queryable, and
  whether a tray is attached at all. Muted notifications therefore stop being a silent failure, because the pull
  authority reports on them.

Clicking a notification opens the web UI at the relevant Root: a shortcut, never the only path.

### Delivery is always the tray's, on every platform

The daemon decides *what* to raise — it owns the state machine, the transition rule and the coalescing — and emits
an event; the tray delivers. The platforms disagree about what is possible (Windows: impossible from the service;
macOS: impossible without a bundle; Linux: perfectly possible from the daemon over the session bus), and a
"whoever can" design would pay for that with a platform branch and two code paths per event.

What makes uniformity cheap is a Linux fact: **the tray process delivers notifications even where its icon cannot
appear.** Stock GNOME has no StatusNotifierItem support, so `ksni` finds no watcher and nothing is drawn — while
`org.freedesktop.Notifications` in that same GNOME works from that same process. The GNOME gap therefore costs the
icon, not the alarm.

Three things follow for free: the linger trap disappears, because the tray exists only inside a graphical session
by construction, so "no session, no notification" needs neither a `graphical-session.target` binding nor
`ServiceUnknown` handling; D-Bus never leaks into the daemon, keeping ticket 08's boundaries intact; and `status`
can say "no tray attached — notifications are not being delivered", because a tray is a socket client the daemon
can see. The honest price: on a machine where the tray was never installed there are no alarms at all. A tray
starting *after* an incident takes current state from `GET /v1/status` on connect — `Last-Event-ID` restores the
stream, it does not substitute for a state snapshot.

### The icon and the notifications are two channels that fail independently

This is the shape all three platforms share, and it is why "the tray works" is not a state that can be reported in
one word:

- **Linux.** SNI is not a freedesktop standard: the specification sits on freedesktop.org as a perpetual draft
  ("Version 0.1, Publication Date: TBD") and every real implementation uses the `org.kde.*` names, so we register
  against `org.kde.StatusNotifierWatcher`. Stock GNOME Shell contains no StatusNotifier code at all and needs the
  AppIndicator extension — which **Ubuntu 26.04 ships and enables by default**, while Fedora 44, Debian 13 and
  RHEL 10 do not. Best estimate from Debian popcon and the Steam survey: **roughly 70–85 % of Linux desktop users
  see the icon with zero configuration, 15–30 % see nothing**, with wide, one-sided uncertainty. XEmbed is not a
  fallback — GNOME removed the X11 session in 49. Notifications, by contrast, work everywhere with no extension.
- **Windows.** Toasts work; the icon lands in the overflow area, and **only the user can promote it** — there is no
  `NIS_VISIBLE` and no opt-in API, only `NIS_HIDDEN` for self-hiding. Nor is promotion queryable, so this cannot be
  a status row: it is a one-time hint shown when the tray is installed. The icon needs a real top-level window with
  a message loop (`HWND_MESSAGE` misses the `TaskbarCreated` broadcast) and must be re-added whenever that
  broadcast arrives, including on a primary-display DPI change.
- **macOS.** The menu bar item is free; notifications require a bundle (see below) and an authorization the user
  can refuse.

**Therefore `keepster status` carries two separate capability rows for the tray**, each with a state, a
machine-readable reason and a human fix — the shape ticket 11 already established for `maintenance: unavailable`,
reused rather than reinvented. Reasons include no owner of `org.kde.StatusNotifierWatcher` (fix: name the
AppIndicator extension, and say Ubuntu already has it), no owner of `org.freedesktop.Notifications`, and
notification authorization denied. **Neither row affects the exit code**: ticket 11 gave the non-zero exit to
things that actually impair the insurance, and a missing view cannot impair what it views.

### macOS: a bundle is engineering, notarisation is a channel decision

`UNUserNotificationCenter` raises `NSInternalInconsistencyException` without a `CFBundleIdentifier`, so **the macOS
tray is an `.app` bundle** — not a preference. Every bundle-free alternative was rejected on identity grounds:
`mac-notification-sys` works unbundled only by swizzling `-[NSBundle bundleIdentifier]` to impersonate another
installed application, defaulting to `com.apple.Finder`; `osascript -e 'display notification'` posts as Script
Editor and opens Script Editor when clicked; `CFUserNotificationDisplayNotice` is a modal alert, not a banner. All
three publish under someone else's name, which is not acceptable for a tool whose whole message is "you can trust
what this says".

**v1 signs ad-hoc and distributes through a Homebrew formula.** Formulae and bottles carry no quarantine attribute,
so Gatekeeper's first-open prompt never fires, and an ad-hoc signature satisfies both Apple silicon's execution
requirement and the notification path. Cost: zero money, no notarisation step in the release. **The condition for
change is recorded rather than deferred into fog: any channel other than a Homebrew formula — a dmg, a cask —
requires a Developer ID certificate, hardened runtime and notarisation, hence Apple Developer Program membership
at 99 USD/year.** Since macOS 15 there is no Control-click override, and from 2026-09-01 Homebrew stops supporting
casks that fail Gatekeeper, so this is a real gate and not a formality. This ticket makes it a stated precondition
of the second channel, in the spirit of ticket 14's manual `make check-api` gate.

Incidentally, the tray makes ticket 05's LaunchAgent coverage gap visible where the user already is: "coverage ends
at logout" becomes a menu bar state instead of a line one must run `keepster status` to find.

### The menu, and the CLI additions it forces

Open the web UI; snapshot now; pause and resume a Root; **restart the daemon**; a one-line health summary in the
menu header; quit. All are `/v1` calls or service-manager operations, nothing exclusive.

Restart exposed a hole in ticket 05's CLI surface, which offered only `service install | uninstall | status`. Under
the view-never-a-source invariant a tray-only restart is not allowed, so **the CLI gains `keepster service restart`,
`start` and `stop`**, and the menu item calls the same code. Restart is the one menu item that stops the insurance,
briefly, so it asks for confirmation; ticket 05 already guarantees shutdown drains the in-flight capture and
flushes pending debounce windows, so it is safe by construction. The tray then loses its socket and reconnects
through the backoff path above.

### Dependencies

`tray-icon` for the icon on all three platforms, with `default-features = false, features = ["ksni"]` on Linux —
the default pulls `libappindicator`, GTK 3 and `libxdo` (X11), while the `ksni` route speaks only D-Bus, so the
Linux build links no GTK, no GLib and no X11. Its `with_guid` (0.25) gives the Windows icon a stable `guidItem`, so
the user's manual promotion out of the overflow area survives the binary being replaced by an update — which
matters precisely because promotion is user-only and not queryable. `notify-rust` for notifications, default
features (zbus, no `libdbus`), with `Hint::DesktopEntry` so GNOME resolves the right name and icon — which means
the tray ships a `.desktop` file on Linux. One licensing note to carry into packaging: **`ksni` is `Unlicense`**,
not MIT/Apache-2.0.
