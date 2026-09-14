# 13 — Rename Project to Watched Root across the domain

Type: task
Status: resolved
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

## Answer

Done, in one pass. `Watched Root` (short `Root`) is now the only name for the entity across `CONTEXT.md`, the map
and tickets `01`–`12`.

**`CONTEXT.md`**: the `Project` entry is now `**Watched Root** (short form **Root**)` with `_Avoid_: project,
repository, workspace, folder, directory`. Ten further entries were carried through — `Registry` (whose own
`_Avoid_` list lost `project list` for `root list`), `Working Tree`, `Include Rules`, `Snapshot`, `History Store`,
`Retention Ladder`, `Forget`, `Degraded`, `Unavailable` — plus the opening paragraph, where "the project's own
version control" became "the directory's own version control".

**Identifiers renamed** (ticket 03): the registry file `project.toml` → `root.toml`, and the commit trailer
`Keepster-Project` → `Keepster-Root`. Ticket 03's own title is now "Where the Root registry and the snapshot index
live", and the map's link text follows it.

**One identifier was not covered by the rename decision and was settled here**: ticket 07 returns per-key
configuration provenance as "the level that set it", and its values were `default / global / project`. The level is
named after the entity, so it renames with it: the enum is now **`default / global / root`** — later extended to
`default / global / global-pattern / global-root / root` when ticket 04's per-Root overrides moved into the global
config (see ticket 04's Comments). It is a wire-visible
value in the `/v1` contract, but no code exists, so the change is free now and would have cost a `/v2` later.
Ticket 04's merge prose ("global layer first, Root layer second", "replaced wholesale by the Root level") now
agrees with it.

**Deliberately left as `Project`**, because they record the rename rather than use the term: this ticket's own
body, the map's `Project` is renamed `Watched Root` decision entry, and ticket 07's closing rename note.

**Nothing to do** in `docs/adr/0001-segmented-snapshot-chains.md` or `docs/adr/0002-pins-as-parentless-commits.md`:
both contain zero occurrences of the term. Ticket 13 listed them speculatively.

**Out of scope, left untouched**: `.scratch/keepster/research/03-metadata-store.md`. Research files are a captured
record of what the sources said at the time, not living documentation, and rewriting them would misrepresent the
findings that produced the `project.toml` decision. A reader arriving from ticket 03 meets the rename in the
ticket.

CLI spellings needed no work: ticket 07 already writes `-r/--root`, `/v1/roots/{id}` and
`keepster move -r <id> <newpath>` throughout.
