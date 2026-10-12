# 61: Autostart on three platforms, and one supervised daemon

**Blocked by:** 36 (Workspace skeleton, the daemon and `keepster status`)

**Status:** done

**What to build:** a user installs autostart with one command that also verifies it, the daemon comes back after a
reboot, exactly one runs per user, and a user on macOS is told plainly about the logout gap instead of discovering it.

One daemon per OS user, running as that user, **never elevated**, and **installation needs administrator rights on
none of the three platforms**. Only the service manager ever starts the daemon, so a running daemon is always a
supervised one. On Windows autostart is a Scheduled Task registered at logon — a per-user service does not exist in
any documented form and would have carried the same logout gap anyway.

- [x] `keepster service install | uninstall | status` writes, verifies and removes the platform unit: a systemd user
      unit on Linux, a LaunchAgent on macOS, a logon Scheduled Task on Windows
- [x] Linger is enabled for the invoking user on Linux without a prompt where the platform allows it, and the exact
      command is printed where it is refused
- [x] Nothing in the install path requires administrator rights on any platform
- [x] A flock'd pid file is the single-instance authority; a second daemon exits non-zero naming the live pid, and a
      stale socket left by a crash is detected and replaced
- [x] When the socket is absent the CLI asks the platform to start the daemon and waits; with no unit installed it
      stops and points at the install command, and it never forks a bare daemon
- [ ] Supervision restarts the daemon after an unexpected death on all three platforms — *declared in each unit
      and asserted as text only; whether the managers honour it is ticket 82's, and on Windows it is in doubt*
- [x] `status` carries the autostart and linger rows, reading as unavailable with the logout-gap wording on macOS,
      and "service never installed" is a state distinct from "stopped"
- [x] The stop path drains and flushes as specified, and a kill past the timeout leaves a recoverable store —
      *on SIGTERM and an interrupt; Task Scheduler ends a task outright, which is the kill case*

## Comments

- **From ticket 39's review: the startup index recovery is only safe once the single-instance lock exists.**
  `recover_indexes` runs before the control channel is bound, with `quick_check` and, on damage or a schema
  mismatch, a discard of `index.sqlite` and its `-wal`/`-shm`. A second daemon started beside a running one would
  therefore unlink indexes the first holds open — the corruption vector the ordering was meant to avoid — and then
  rebind the socket. The flock'd pid file this ticket adds must be taken *before* `recover_indexes` runs, and the
  test for a refused second instance should assert that it touched no index.
- **From ticket 60: `keepster service install --no-web-ui`.** The flag calls
  `keepster_service::declare_no_web_ui(config_path)`, which writes `[ui] listener = false` only when it
  creates the global configuration. On `NoWebUi::Existing { add }` the command changes nothing and prints
  `add` plus the restart reminder (ticket 24). The listener-off refusal of `keepster ui` currently says
  "restart the daemon"; once `keepster service restart` exists, name it there.
- Done. `keepster-service::autostart` writes a systemd user unit (`Restart=on-failure`,
  `RestartPreventExitStatus=5`, `TimeoutStopSec=10`, `WantedBy=default.target`), a LaunchAgent
  `org.keepster.daemon` (`RunAtLoad`, `KeepAlive.SuccessfulExit=false`, `ExitTimeOut=10`) and a Scheduled Task
  `Keepster` (logon trigger for the invoking user, `InteractiveToken`, `LeastPrivilege`, `RestartOnFailure`
  every minute, no time limit). `service install` writes, registers, starts, switches linger on (printing
  `sudo loginctl enable-linger <user>` where polkit refuses), then verifies the unit is reported installed
  and the daemon answers; a refused registration removes what it wrote. `service uninstall` stops and
  removes the unit and nothing else. `service status` exits 0 running and enabled, 3 installed but not
  running, 4 never installed, 5 installed but disabled.
- `daemon.pid` in the runtime directory is locked (`flock` on Unix, a write-unshared open on Windows) before
  the Registry is opened, so a refused second daemon touches no index — tested by taking the live daemon's
  index away and seeing it not come back. A refused daemon exits 5, which the units do not restart on.
- With no `--control-path`, any command but `status`, `daemon` and `service` asks the service manager to
  start a daemon that does not answer and waits up to 20 s; with no unit it points at `service install`.
  `status` never starts one: it says "never installed" or "installed but not running", exit 3 either way.
- The daemon now stops on SIGTERM as on an interrupt, through the drain-and-flush sequence; tested with the
  binary, as is a SIGKILL followed by a restart serving the store.
- `status` fills `autostart` and `linger` from the platform, with `autostart_reason` (`not_installed`,
  `disabled`) and `linger_reason` (`logout_gap` on macOS and Windows, `disabled` on Linux).
- **What no test here exercises:** the registration itself — `systemctl --user enable`, `launchctl
  bootstrap`, `schtasks /Create` — and the restart after an unexpected death. Registering a unit changes the
  machine a test runs on, so the tests cover the unit texts as values and every decision Keepster makes in a
  sandbox whose service manager cannot be reached. The three platforms want a manual conformance pass, as the
  watcher's events do. On Windows the task runs the console binary, so a console window appears at logon
  until the tray or packaging ticket gives the daemon a windowless entry point.
- Closed with the supervision box open: the units declare it and the tests read the declarations, but no test
  watches a manager restart anything, and on Windows `RestartOnFailure` may not cover a process that exits.
  That evidence is ticket 82's, split out rather than claimed here.
- From review: `service install` refuses (exit 5, naming the pid) while a daemon no service manager started is
  running, since the unit's own daemon would be refused by its lock and the install would then report a
  supervised daemon that does not exist. A held pid lock is asked again for up to a second before it is
  believed, because a process the daemon spawns holds a copy of the lock's descriptor between fork and exec;
  the harness hit that window on restarts. SIGTERM is taken over before the daemon starts, and the installed
  path is the executable as the platform names it, not canonicalised. ADR 0008 is amended: the Windows daemon
  is a Scheduled Task now, not a per-user service.
