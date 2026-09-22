# 41: Include Rules, the secret denylist, the size cap and `explain`

**Blocked by:** 38 (The first Snapshot — History Store invariants and the write path)

**Status:** ready-for-agent

**What to build:** a user's own include and exclude rules decide what a Snapshot captures, secrets stay out by
default, an over-large file is recorded rather than silently missing, and the user can ask why any specific path is
or is not captured — with the rule that decided it named.

Rules are layered pattern lists with negation and last-match-wins, evaluated as pure values. **An excluded directory
is never entered**, so nothing beneath it can be re-included (ADR 0007) — git's rule, adopted because watches are
installed after the rules are applied. Git's silence is not adopted with it: the explain answer names the pruning
ancestor, and loading rules warns for every negation an ancestor makes unreachable.

- [ ] Include and exclude rules layer with negation, the last matching pattern wins, and the matcher is tested as
      values with no disk
- [ ] An excluded directory is never entered, and a negation beneath one is reported as unreachable with a warning
      that does not make the Root `Degraded`
- [ ] The built-in secret denylist refuses the documented name classes by default, and an explicit opt-in list can
      re-admit a specific path
- [ ] A file over the default size cap is recorded as Withheld with the too-large reason, and a per-pattern cap
      overrides the scalar
- [ ] `explain?path=` and `keepster explain` answer whether a path is captured, which rule at which level decided
      it, and which ancestor pruned it where one did
- [ ] Ignore files of the watched directory are **not** honoured by default, and the opt-in layer sits beneath the
      user's own rules
- [ ] Capture applies the rules, so a Snapshot of a tree containing excluded paths holds none of them
