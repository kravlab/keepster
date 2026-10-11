# 57: Capture Preview

**Blocked by:** 41 (Include Rules, the secret denylist, the size cap and `explain`), 42 (Configuration — two levels,
provenance, automatic reload, `Degraded`)

**Status:** ready-for-agent

**What to build:** before registering a large directory — or after registering one — a user can see what would be
captured and what would be excluded, grouped by the rule that decided, without capturing anything.

A Capture Preview classifies a directory against the rules that would govern it. It is streamed rather than a job,
because a dropped connection strands nothing; it never opens the History Store, takes no lock, counts as no Trigger,
and aborts when the client disconnects. It reports **content to capture** and refuses to guess store growth, which is
unknowable without hashing every file — that being the capture.

- [ ] The preview streams rows as it walks, with the first row arriving promptly on a large tree
- [ ] Output groups by the rule that decided, with the largest entries shown on both sides and a full-depth form for
      the detailed view
- [ ] It works for a registered Root and for a directory that is not one, honouring an in-tree configuration file
      exactly as registration would and **showing** what that file sets
- [ ] It opens no History Store, takes no lock, counts as no Trigger, and stops when the client goes away
- [ ] It states no store-growth estimate
- [ ] `keepster watch --dry-run` is an alias for it
- [ ] Registering a directory above the claimed per-Root ceiling runs the preview and asks for explicit confirmation
      rather than refusing
- [ ] Pending paths and the age of the oldest pending change are reported in `status` instead of listing the debounce
      queue

## Comments

- Ticket 43 delivered this ticket's status criterion: every Root carries `pending_paths` and
  `oldest_pending_seconds` (absent when nothing is pending), from the trigger policy's own state. What is left
  here is the CLI rendering, if `status` should print them.
