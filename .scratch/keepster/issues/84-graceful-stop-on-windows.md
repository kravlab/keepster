# 84: A graceful stop of the daemon on Windows

**Blocked by:** 67 (The tray process, and notifications when the insurance stops)

**Status:** migrated

**Migrated to:** #21

**What to build:** `keepster service stop` and `keepster service restart` on Windows stop the daemon the way they do
on Linux and macOS: the daemon finishes its graceful shutdown and says `daemon.stopping` on the event stream before
it exits, so a tray tells a stop someone asked for from a death.

Today `autostart::stop` on Windows runs `schtasks /End`, which ends the task's process outright (ticket 67, plan item
13). The store survives that as it survives any kill, but nothing the daemon does on a graceful stop happens, and the
stream ends without `daemon.stopping`: ten seconds later the tray says "Keepster's daemon stopped unexpectedly" after
a stop the person asked for. The daemon on Windows waits only for Ctrl-C (`crates/keepster-daemon/src/main.rs`), and
a process the Task Scheduler started has no console to send one to.

`restart` runs `schtasks /End` and then `/Run` at once. The task's `MultipleInstancesPolicy` is `IgnoreNew`, so a
`/Run` that arrives while the ended process is still exiting may be ignored and leave the daemon stopped. This was
read from the code and the task definition; it has not been observed on a Windows machine.

- [ ] Decide how a stop request reaches the daemon on Windows, and that only the daemon's own user can send one
- [ ] `service stop` uses it, and the stream says `daemon.stopping` before it ends
- [ ] `service restart` starts the task again only once the old process has exited
- [ ] Tests, at the seams the decision allows, show a stop announced as a stop; the tray then says nothing

## Comments

- Found by the code review of ticket 67.
