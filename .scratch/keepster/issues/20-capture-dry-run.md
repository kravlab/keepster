# 20 — Capture dry-run: what would enter the next Snapshot

Type: grilling
Status: open
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
