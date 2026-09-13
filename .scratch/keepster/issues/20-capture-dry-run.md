# 20 — Capture dry-run: what would enter the next Snapshot

Type: grilling
Status: resolved
Blocked by: 12

## Question

Before trusting a Root — or before registering one at all — the user wants to see which files and directories a
Snapshot would contain, and how big it would be, without writing one.

Settle:

- **The output.** The included paths, the excluded ones with their reason (our Include Rules, the secret denylist,
  the `Withheld` reasons from ticket 10), or both — and how that stays readable for a 100k-file Root rather than
  becoming a wall of text nobody reads.
- **Which size is reported, and how it is labelled.** The sum of included file sizes is cheap and honest. The
  actual growth of the History Store is not knowable without hashing every file, which *is* the Snapshot — and
  reporting a number the user will read as "disk cost" when it is not is worse than reporting no number at all.
- **Which question it answers**: "the whole tree as it stands now" (a full walk, at Reconciliation's cost) or "what
  the current pending change set would add" — the latter depends on ticket 12's index lifetime, which is why this
  ticket is blocked on it. Possibly both, under different names.
- **Whether it runs for a directory that is not yet a Root.** This is the more useful case, because it is how a
  user decides whether to register at all, and it means running the rule engine with no Registry entry and no
  store — including `.keepster.toml` in a directory we have never watched (ticket 04 deliberately left that file
  unrestricted, which is an accepted risk for a *registered* Root and needs re-examining for an unregistered one).
- **The relationship to `explain?path=`** (tickets 04 and 07), which already answers "why this path" for one path.
  Dry-run is the same answer for a whole tree, so it must reuse that engine rather than become a second one that
  can disagree with the first.
- **The surfaces**: the `/v1` endpoint, the CLI shape (a flag on `watch`, or its own command — they differ in
  whether a Root must exist), and whether the web UI shows it while adding a Root.
- **Interaction with the live daemon**: a dry-run must not count as a Trigger for the rate limit, must not disturb
  debounce windows, and should state whether it takes any lock — a walk that blocks capture would make the
  diagnostic more dangerous than the thing it diagnoses.

## Answer

**A read-only classification called a Capture Preview: it answers "what would a Snapshot of this directory hold,
right now", for a registered Root and for a directory that is not one. It never opens the History Store, takes no
lock, and is not a Trigger.** The second question the ticket posed — "what would the pending change set add" — is
demoted from a command to a number.

### The pending change set is a count, not a listing

After ADR 0004 an ordinary capture performs no walk: it re-seeds from the previous Snapshot's tree and touches only
the paths accumulated in ticket 08's `PolicyState`, plus the backoff retry queue. So the pending set is nearly free
to read — and worth almost nothing as a listing. It exists only for a registered Root with a live daemon, and it is
ephemeral: a debounce window can flush between the question and the answer, so half the paths printed are already
captured by the time they are read. The need behind it — *is the daemon actually seeing my edits?* — is a health
question, and ticket 05 made `keepster status` the authority on health.

It therefore becomes two fields on `GET /v1/roots/{id}` and in `status`: `pending_paths` (a count) and the age of
the oldest pending path. **No byte total**, because the queue holds paths whose sizes are already stale and a byte
figure there would move when nothing had changed. This keeps one walk-shaped surface in the tool instead of two
with different lifetimes and different meanings for "size".

### The size reported is content to capture, and never store growth

The walk `stat`s every path, so counts and byte totals are free. Three numbers could be read into "snapshot size"
and only two are knowable without performing the capture:

- **Content to capture** — the sum of `st_size` over the files whose content the Snapshot would hold. Reported.
- **Disk growth of the History Store** — what the Snapshot costs after zlib compression and dedup against existing
  objects. **Not knowable without hashing every file, which is the capture.** The spread between text (2–5× smaller)
  and already-compressed binaries (`.jpg`, `.mp4`, `.docx`, ≈ 1×) is too wide to estimate without lying, and a number
  the user reads as "disk cost" when it is not is worse than no number. Not reported, and no percentage is attached
  to the one sentence that says the store will hold less.
- **Current store size on disk** — real, but only exists for a Root that already has history, and ticket 07 already
  puts it in `status`. Not repeated here.

Four count/byte pairs make up the summary: `captured`, `excluded`, `withheld` (split per reason), `unreadable`.
Sizes are apparent (`st_size`), not allocated (`st_blocks`), because git stores logical content — a sparse file
counts as its logical length. Directories are counted separately and contribute no bytes; a symlink counts as the
length of its target path, which is what git stores.

### Output is grouped by the rule that decided, not by directory alone

A flat per-path list is truthful and useless at 100k paths; a bare summary cannot answer "which files?" and forces
a second command. The default is the summary plus a breakdown keyed on **the rule that made the decision** —
`excluded: 4,213 files, 812 MiB — pattern node_modules/ (global, line 12)`, `withheld too_large: 3 files, 6.1 GiB`
— with a **top-N largest** list on both sides, which is what turns a byte total into a decision.

Rule-keyed grouping is what makes this reuse ticket 07's `explain?path=` rather than become a second engine that
can disagree with the first: `explain` already returns which rule at which level decided a path, so a Capture
Preview is that verdict mapped over a walk and grouped by its own answer. The grouping key is free because the
verdict already carries it.

Drill-down: `--path <dir>` scopes everything to a subtree, `--only captured|excluded|withheld|unreadable` prints
the actual paths of one class, and `--depth infinity` lists every path in the Root — the detailed view, spelled
with ticket 07's existing `depth=1|infinity` vocabulary rather than a shell-expanded `--path *`. On the wire this
mirrors ticket 07's rejection of `depth=infinity` as a default on `entries`: aggregates plus a bounded top-N by
default, full enumeration only when asked.

### Pruning wins over negation — refines tickets 04 and 02

Recorded as `docs/adr/0007-pruning-wins-over-negation.md`, because it is a capture-semantics rule rather than a
preview rule and this ticket is not where a future reader will look for it.

The ticket exposed a gap the map never closed. Ticket 04 made pattern lists layered with negation ("last matching
pattern wins") but never said what happens when the last match is on a file whose *parent directory* is excluded.
Ticket 02 then measured its way to a constraint that presumes an answer: Include Rules are applied **before
installing watches** (~10k watches to arm a 100k-file tree, against a per-user budget of 133,264). A pruned
directory receives no watches, so nothing inside it can produce an event.

**Git's rule is adopted: an excluded directory is never entered, and nothing beneath it can be re-included.** The
semantics are already in the user's head from `.gitignore`, the watch budget stays bounded, and Reconciliation's
walk stays proportional to what is captured. Full semantics (descend everywhere, negation always wins) throws away
exactly the optimisation ticket 02 measured. Prefix analysis (prune unless an anchored negation's literal prefix
could reach inside) is correct and usually fast, but an unanchored negation such as `!*.keep` defeats it and forces
a full descent, making walk cost a function of a pattern written three levels up — and `explain` would then have to
explain a static analysis.

Git's rule is adopted with the diagnostic git never gives, because its failure mode is silence — `!node_modules/mylib/**`
does nothing and says nothing:

- `explain?path=` names the pruning ancestor: *"re-included by `!node_modules/mylib/**` (root, line 8), but
  `node_modules/` (global, line 12) prunes the parent — pruning wins."*
- **Config load warns for every negation pattern an ancestor pattern makes unreachable**, at the moment the rule is
  written rather than the day a file turns out to be missing from history. A warning, not an invalid config: the
  Root does not go `Degraded`.

This also settles the descent trade-off. Capture never enters a pruned directory, so measuring excluded bytes is a
cost capture never pays — and the pruned subtree is usually the most expensive part of the tree. The default
summary reports each pruned subtree as **one line, named and unmeasured** (`node_modules/ — pruned by …`); byte
totals for them arrive only under `--measure-excluded`, and `--depth infinity` implies it, since listing the paths
inside a pruned subtree requires entering it.

### An unregistered directory, and its `.keepster.toml`

**Preview runs on a directory that is not a Root, and honours the `.keepster.toml` it finds there exactly as
registration would.** Ignoring the file would make the preview predict something other than what registering does,
which is the one job it has; gating it behind `--trust` makes the default wrong-by-design for the case that matters
most and produces a flag answered reflexively.

Preview neither writes nor launches anything, so ticket 04's `ui.editor` is inert here. The leverage a hostile file
has is over *what the preview shows*, and the failure mode is not a lie but a silent truth: a config adding `.env`
to the secret opt-in list makes the preview correctly report `.env` as captured, and the user reads that as
Keepster's default rather than as the repository author's choice. So the output opens with a **config-found
section** naming every key the Root config sets, with the security-relevant ones — secret opt-in additions,
`ui.editor`, store location — called out rather than buried in a key dump.

That makes the preview **be** ticket 04's unbuilt mitigation. Ticket 04 accepted the unrestricted Root config
knowingly and named the mitigation it did not build — *a trust prompt on first sight of a Root config*. Two
consequences close it at no new cost:

- `keepster watch <dir>` on a directory holding a `.keepster.toml` it has never seen shows that same section and
  asks for confirmation; `--yes` skips.
- A preview whose path lies inside or contains an existing Root reports the overlap in the header record, since
  ticket 07's no-overlap rule means registering it would fail with `409 root_overlaps` — better learned before the
  walk than after.

### Streamed, lock-free, and not a Trigger

Ticket 07 made `forget` and Maintenance jobs because a dropped connection leaves an irreversible operation with no
identity to query. Preview is the inverse of that argument: nothing happens, so a dropped connection costs a re-run.
By ticket 07's own reasoning it is not a job, and a job would additionally make the daemon persist a 100k-entry
result for a read-only diagnostic.

It is **synchronous and streamed**: the connection stays open, per-path records stream as NDJSON while the walk
runs, and a final summary record carries the totals. Nothing is buffered, progress is implicit, the CLI renders
incrementally, and **client disconnect aborts the walk**, so a closed tab does not leave a 100k-path walk grinding.
The default summary is a short terminating response over the same mechanism.

Four properties follow from what a preview does not touch:

- **It never opens the History Store.** The preview classifies the Working Tree; it is not a delta against the last
  Snapshot. So it cannot contend with the Maintenance lock or a running capture, and it takes no lock of its own.
- **It is not a Trigger.** It writes nothing to `PolicyState`, counts nothing toward the rate limit, and neither
  flushes nor extends a debounce window.
- **At most one preview walk in flight per daemon**; a second is refused `409`. A cheap guard on an I/O-heavy
  operation, in the spirit of ticket 07's rate limit on `open`.
- **It runs daemon-side even for an unregistered directory**, because ticket 08's CLI links neither `core` nor the
  store and so cannot evaluate Include Rules itself. `keepster preview` on an unwatched folder therefore needs the
  daemon running, which the CLI says plainly rather than half-answering from defaults.

### Surfaces, and the name

The ticket's own title fights itself: "dry-run" means *run the operation but do not commit it*, which is right as a
flag on registration and wrong as a standalone command — there is no run to dry. Ticket 10 has already spoken for
`dry_run` in its Restore sense ("return the plan, write nothing").

**Term: Capture Preview.** A classification of a directory's current contents against the Include Rules that would
govern it, without capturing anything. Added to `CONTEXT.md`. It is not a Snapshot, and it is not Reconciliation,
which compares a Working Tree against its *latest Snapshot*; a preview compares nothing, it classifies.

**API — one endpoint, with no Root in the path.** `GET /v1/preview?path=<abs>&depth=1|infinity&only=&measure_excluded=&top=`,
returning `application/x-ndjson`. `GET` because it is a pure read. There is no `/v1/roots/{r}/preview` twin: a
preview whose subject may not be a Root cannot live under a Root, and ticket 07 already resolves an absolute path
with `GET /v1/roots?path=`, so preview performs that resolution internally. The stream's **header record** carries
`root: <id> | null`, the config-found section and any overlap; then the per-path records; then the summary record.

**CLI — `keepster preview [<dir>]`**, with `--depth infinity`, `--only`, `--measure-excluded`, `--top N`, and
`--json` as a verbatim passthrough of the stream, matching ticket 07's `status --json`. With no argument it follows
ticket 07's cwd walk-up — the Root containing cwd — and falls back to cwd itself as a *candidate* directory when
there is no Root above, which is the unregistered case. **`keepster watch <dir> --dry-run` is an alias** that runs
the preview and does not register: the word keeps its correct meaning there, over one implementation.

**Web UI — in the Add-Root flow, before the confirm button.** Choosing a directory through ticket 19's
`GET /v1/fs` runs the preview and shows the summary — counts, byte totals, top-N largest in and out, and the
config-found section — with drill-down optional and registration one click past it. Re-runnable from the Root's
Include Rules view, which is where a user goes after asking "why wasn't my file saved".

### What a stat-only walk can and cannot classify

Preview opens no file and hashes nothing, and it turns out not to need to: **every `Withheld` reason from ticket 10
is derivable from `stat` or from the name alone** — `too_large` and `special_file` from the `stat`, `reserved_name`
and `unrepresentable_path` from the name, `other_root` and `history_store` from the Registry, `cloud_placeholder`
from the file attributes ticket 10 already reads. So a preview reports the same `Withheld` set a real capture would.

What it cannot report is a file that `stat`s cleanly and fails on read, because detecting that means reading it —
so ticket 10's `incomplete` has no preview equivalent, and `unreadable` here means a failed `stat` or `readdir`
only. Symlinks are classified under ticket 10's `follow_symlinks` list, exactly as capture would, and unlisted
symlinks are not followed. The walk runs as the daemon's own user, never elevated (ticket 05), so a directory the
user cannot read is `unreadable` rather than an error.
