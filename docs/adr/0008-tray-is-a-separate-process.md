# The tray is a separate process, and the only carrier of desktop notifications

Keepster ships two executables: the headless daemon, started by the platform's service manager (ADR-adjacent:
ticket 05), and a tray process started by the **desktop session** — XDG autostart on Linux, an `Aqua` LaunchAgent
on macOS, the `Run` key on Windows. The tray is an ordinary `/v1` client over the CLI's permission-protected
socket, and it is the only component that draws an icon or raises a notification, on every platform. A future
reader will reasonably ask why a daemon that already talks HTTP needs a second binary to show an icon; the answer
is three platform facts that are invisible from the code.

## Why a second process is not a preference

- **Windows makes it impossible to avoid.** The daemon is a per-user service. Such an instance does run under the
  signed-in user's token in that user's logon session ([per-user
  services](https://learn.microsoft.com/en-us/windows/application-management/per-user-services-in-windows)) — but
  the SCM still assigns a service process running under a user account the window station `Service-0xZ1-Z2$`, and
  "all other window stations are noninteractive, which means they cannot display a user interface or receive user
  input"; only `WinSta0` can ([Window Stations](https://learn.microsoft.com/en-us/windows/win32/winstation/window-stations),
  [Window Station and Desktop Creation](https://learn.microsoft.com/en-us/windows/win32/winstation/window-station-and-desktop-creation)).
  Microsoft never states "a per-user service cannot show a tray icon" in one sentence, so this is an inference from
  two documented rules — but it is the same inference behind Microsoft's own prescribed remedy: create a separate
  process in the interactive user's session and reach the service over IPC
  ([Interactive Services](https://learn.microsoft.com/en-us/windows/win32/services/interactive-services)).
- **macOS makes notifications impossible without a bundle.** `UNUserNotificationCenter` raises
  `NSInternalInconsistencyException` for a process with no `CFBundleIdentifier`, so the notifying component must be
  an `.app`. The menu bar item itself is free — `NSStatusBar::systemStatusBar()` needs no bundle, only a main-thread
  run loop — which is why the icon and the notification split cleanly here.
- **Linux makes it possible to skip, and it is skipped anyway.** The daemon could call `Notify()` on the session bus
  directly. Uniformity is worth more than the saved process: a component that lives inside the daemon on one
  platform and outside it on two others forks every future lifecycle discussion, and a windowing event loop that
  survives logout contradicts the linger design ticket 05 settled.

## Considered Options

- **The tray inside the daemon wherever the platform allows.** Rejected above: one saved process, bought with a
  permanent platform branch in the lifecycle.
- **"Whoever can, notifies": the daemon on Linux, the tray on macOS and Windows.** Rejected. It would reach Linux
  users who never installed the tray, but the reach is almost entirely illusory — on stock GNOME the tray process
  raises notifications perfectly well even though its icon cannot appear, so the users it would rescue are only
  those with no tray installed at all. The price is a platform branch and two code paths per event, plus D-Bus and
  `.desktop`-entry knowledge leaking into the daemon crate against ticket 08's boundaries, plus the need to bind
  delivery to `graphical-session.target` and handle `ServiceUnknown` for a lingering daemon with no session.
- **No tray, notifications out of scope** (ticket 05's original position). Reversed by the user: ticket 05 argued
  notifications "deliver nowhere on the lingering headless daemon", which is true of the daemon and not of a
  process that exists only inside a graphical session.

## Consequences

- **The governing invariant is that the tray is a view, never a source.** Every state it displays and every alarm
  it raises must be derivable from `GET /v1/status` and the event stream, and no notification may carry an action
  reachable only from the tray. Without this, ticket 05's "`keepster status` is the single authority on health"
  quietly stops being true and a headless machine loses information rather than convenience.
- **The invariant is load-bearing, not decorative.** It is what forces `keepster service restart | start | stop`
  into the CLI, because the tray's restart menu item would otherwise be a tray-only action; and it is why the
  tray's own capability rows in `status` leave the exit code alone — ticket 11 gave the non-zero exit to things
  that impair the insurance, and a missing view cannot impair what it views.
- **A machine with no tray is a normal machine.** The daemon, `keepster install` and `status` must be fully usable
  where the tray was never installed. The honest price is that such a machine gets no alarms, which `status`
  reports as "no tray attached".
- **The daemon decides, the tray delivers.** The transition detection, coalescing and replacement rules live in the
  daemon and reach the tray as events, so two sessions with two trays cannot disagree about what happened. A tray
  connecting after an incident takes state from `GET /v1/status`; `Last-Event-ID` restores the stream but is not a
  state snapshot.
- **Icon and notifications are two independently failing channels**, so health is reported as two rows and never as
  one word. Stock GNOME draws no icon but delivers notifications; Windows delivers toasts but hides new icons in
  the overflow area, where only the user can promote them and no API can even ask whether they did.
- **Packaging inherits a second artifact** and three gates: the macOS `.app` is ad-hoc signed and Homebrew-formula
  only in v1, with Developer ID plus notarisation the precondition of any other channel; the Windows installer must
  create a Start-menu shortcut carrying an AUMID, which is what makes a toast display at all; and `ksni`, which
  keeps GTK and X11 out of the Linux build, is `Unlicense` rather than MIT/Apache-2.0.

## Amendment (ticket 61)

On Windows the daemon is no longer a per-user service: ticket 27 replaced it with a Scheduled Task registered at
logon, which runs in the user's own session with an interactive token. The window-station argument above therefore
no longer forces the split on Windows, though it may still prevent the daemon from drawing until ticket 82 says
what such a task's process can do. The decision stands on the record's other grounds: macOS needs a bundle to
notify, and a component inside the daemon on some platforms and outside it on others would fork every lifecycle
discussion — the argument the Linux bullet already makes.
