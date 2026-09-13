# An excluded directory is never entered, so nothing beneath it can be re-included

Include Rules layer with negation and "last matching pattern wins" (ticket 04), which leaves one case unstated:
a rule re-includes a file whose parent directory an earlier rule excluded. We adopt git's answer — the excluded
directory is pruned, is never descended into, and no rule beneath it is ever evaluated. `!node_modules/mylib/**`
under an excluded `node_modules/` therefore captures nothing. A reader will expect "last matching pattern wins"
to mean what it says, so the exception is written down here rather than left to be discovered.

The constraint is not aesthetic. Ticket 02 measured the watcher's cost and settled that Include Rules are applied
**before** installing watches: ~10k watches and 1–4 s to arm a 100k-file tree, against a per-user budget of 133,264.
`inotify` watches are per-directory, so a Root whose excluded subtrees are watched anyway costs watches
proportional to the whole tree rather than to what is captured. A pruned directory receives no watches, which means
no event inside it can ever be delivered — a re-included file there would be invisible to the watcher and visible
only to Reconciliation, and only if Reconciliation descended. Honouring negation below an exclusion therefore is
not a rule-engine change; it is a decision to watch and walk everything.

## Considered Options

- **Full semantics: descend everywhere, negation always wins.** Correct in the obvious sense, and it discards
  exactly the optimisation ticket 02 measured. Watch count and Reconciliation walk cost both become proportional to
  the entire tree, `node_modules` and build output included, on every Root.
- **Prefix analysis: prune unless an anchored negation's literal prefix could reach inside.** Correct *and* usually
  fast, and rejected on two counts. An unanchored negation (`!*.keep`) has no prefix to compare, so it forces a full
  descent — making walk cost a function of a pattern written three levels up, unpredictably. And ticket 07's
  `explain?path=` would then have to explain a static analysis over the rule set, rather than name a rule and a
  line, which is the whole reason `explain` is comprehensible.
- **Git's rule, adopted silently.** Rejected. Git's failure mode is that the negation does nothing and says nothing,
  and the user finds out when a file is missing from history — the one outcome this tool exists to prevent.

## Consequences

- **`explain?path=` names the pruning ancestor**, not just the winning rule: *"re-included by
  `!node_modules/mylib/**` (root, line 8), but `node_modules/` (global, line 12) prunes the parent — pruning wins."*
  Ticket 07 built `explain` to remove the largest class of "why wasn't my file saved" questions; this is the case
  where it earns that.
- **Config load warns for every negation an ancestor pattern makes unreachable**, at the moment the rule is written
  rather than the day the file is missed. A warning, not an invalid config: the Root does not go `Degraded`, because
  the configuration is well-formed and the daemon's behaviour is fully defined.
- **A Capture Preview does not measure pruned subtrees by default** (ticket 20). Capture never enters them, so
  sizing them is a cost capture never pays, and it usually dominates the walk. Each pruned subtree is one named,
  unmeasured line; `--measure-excluded` and `--depth infinity` opt into the descent.
- **The escape hatch is the one users already know.** Excluding a directory's *contents* rather than the directory
  (`node_modules/*` with `!node_modules/mylib/`, re-included level by level) keeps each ancestor unexcluded and so
  keeps negation working — at the watch cost that buys, which is now the user's explicit choice rather than ours.
- **Reconciliation's walk stays proportional to what is captured**, which is what makes ticket 02's guarantee — the
  walk is the correctness mechanism, not the watcher — affordable enough to leave permanently enabled.
