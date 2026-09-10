# 13 — Rename Project to Watched Root across the domain

Type: task
Status: open
Blocked by: 07

## Question

Nothing to decide: [07 — Daemon HTTP API and CLI surface](07-api-and-cli-surface.md) settled that the entity
formerly called **Project** is a **`Watched Root`** (short form `Root`). The decision unblocks nothing until the
documents actually say so, and every later ticket that keeps writing `Project` re-introduces the term.

Carry the rename through, in one pass:

- `CONTEXT.md`: rename the `Project` entry to `Watched Root`, update its `_Avoid_` list (add `project`, `folder`,
  `directory`), and update every other entry that says "per-Project", "a Project's …" — `Registry`, `Working Tree`,
  `Include Rules`, `Snapshot`, `History Store`, `Reconciliation`, `Retention`, `Degraded`, `Unavailable`.
- `.scratch/keepster/map.md`: Destination, Notes, Decisions so far, Not yet specified, Out of scope.
- Tickets `01`–`12`, questions and answers alike.
- `docs/adr/0001-segmented-snapshot-chains.md` and `docs/adr/0002-pins-as-parentless-commits.md`.
- The registry file name `project.toml` → `root.toml` (ticket 03) and the commit trailer
  `Keepster-Project` → `Keepster-Root` (ticket 03).
- CLI spellings fixed by ticket 07: `-r/--root`, `keepster move -r <id> <newpath>`.

No code exists yet, so this is a documentation-only change and now is the cheapest moment it will ever be.
