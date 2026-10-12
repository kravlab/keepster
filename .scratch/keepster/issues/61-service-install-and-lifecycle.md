# 61: Autostart on three platforms, and one supervised daemon

**Blocked by:** 36 (Workspace skeleton, the daemon and `keepster status`)

**Status:** ready-for-agent

**What to build:** a user installs autostart with one command that also verifies it, the daemon comes back after a
reboot, exactly one runs per user, and a user on macOS is told plainly about the logout gap instead of discovering it.

One daemon per OS user, running as that user, **never elevated**, and **installation needs administrator rights on
none of the three platforms**. Only the service manager ever starts the daemon, so a running daemon is always a
supervised one. On Windows autostart is a Scheduled Task registered at logon — a per-user service does not exist in
any documented form and would have carried the same logout gap anyway.

- [ ] `keepster service install | uninstall | status` writes, verifies and removes the platform unit: a systemd user
      unit on Linux, a LaunchAgent on macOS, a logon Scheduled Task on Windows
- [ ] Linger is enabled for the invoking user on Linux without a prompt where the platform allows it, and the exact
      command is printed where it is refused
- [ ] Nothing in the install path requires administrator rights on any platform
- [ ] A flock'd pid file is the single-instance authority; a second daemon exits non-zero naming the live pid, and a
      stale socket left by a crash is detected and replaced
- [ ] When the socket is absent the CLI asks the platform to start the daemon and waits; with no unit installed it
      stops and points at the install command, and it never forks a bare daemon
- [ ] Supervision restarts the daemon after an unexpected death on all three platforms
- [ ] `status` carries the autostart and linger rows, reading as unavailable with the logout-gap wording on macOS,
      and "service never installed" is a state distinct from "stopped"
- [ ] The stop path drains and flushes as specified, and a kill past the timeout leaves a recoverable store

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
