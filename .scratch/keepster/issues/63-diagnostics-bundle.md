# 63: `keepster diagnose` — a local bundle that discloses by inventory

**Blocked by:** 62 (Logs — one rotated file per user, and a debug level that expires), 52 (The external `git`
prerequisite, the Maintenance Pass and the disk cap), 51 (Retention — the ladder, the Segment Period and thinning)

**Status:** done

**What to build:** a user filing a bug runs one command, gets one local file, and is shown exactly what went into it
— so they can decide what to share. Nothing is ever uploaded.

Privacy is handled by **inventory, not redaction** (ADR 0013): a path arrives in fragments, so a redaction pass would
promise what it cannot keep, and a guarantee that is 95 % effective is worse than none. The content is fixed, because
a trimmed bundle looks exactly like a complete one.

- [x] The command writes a local archive and prints an inventory — every file, its source, and one line on what it
      discloses; a dry-run form prints the inventory without writing
- [x] Contents are fixed with no selection flags: status, the build, API and store-format versions, the environment
      (including the filesystem type under each Root and the watch counters against the ceiling), the external `git`
      discovery result and the last maintenance pass's stderr, the effective configuration with per-key provenance,
      the Registry, per-Root store topology, the index watermark, and the log tails
- [x] **No bytes of any file from history are ever included**, under any flag
- [x] **Nothing is sent anywhere**, and there is no code path that could
- [x] The daemon collects it; with the daemon unreachable the CLI collects what is on disk and the inventory names the
      missing sections rather than producing a smaller bundle that looks whole
- [x] The inventory states which format the log file is in
