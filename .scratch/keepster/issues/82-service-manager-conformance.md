# 82: Service-manager conformance on three platforms

**Blocked by:** 61 (Autostart on three platforms, and one supervised daemon)

**Status:** migrated

**Migrated to:** #20

**What to build:** evidence, per platform, that the units ticket 61 writes do what they declare — so that "the
daemon comes back after a reboot and after an unexpected death" is observed rather than assumed.

Ticket 61's tests assert the unit texts as values and every decision Keepster makes in a sandbox whose service
manager cannot be reached, because registering a unit changes the machine a test runs on. What no test there sees
is the manager honouring the unit. Like the watcher's per-platform behaviour, this is a manual conformance report
that gates nothing — unless a disposable runner (a fresh VM per run) makes it automatable, which is the first thing
to decide.

- [ ] Linux (`systemd --user`): install, verify enabled and running, `kill -9` restarts it, `kill -TERM` does not,
      a refused second daemon (exit 5) is not restarted, reboot brings it back, linger keeps it across logout
- [ ] macOS (LaunchAgent): the same, plus whether `KeepAlive.SuccessfulExit=false` relaunches a refused daemon
      (exit 5) every throttle interval while a bare daemon holds the lock — and if it does, what to change
- [ ] Windows (Scheduled Task): whether `RestartOnFailure` restarts a process that **exits** non-zero or is killed,
      or only a task that fails to launch — widely reported as the latter, which would leave the daemon
      unsupervised and need another mechanism (a watchdog task, or a repetition trigger with `IgnoreNew`)
- [ ] Windows: whether the console window the task's process opens at logon is acceptable until a windowless
      entry point exists
- [ ] `keepster uninstall` on each leaves no unit, no running daemon, and every History Store untouched
