# 71 — Capturing a repository's own git configuration

Type: grilling
Status: needs-triage

## Question

**Motivating incident (2026-09-22, the map's owner).** An agent ran a script, and the script corrupted the
repository's git configuration. Nothing wrote to the config through `git config`, so no policy around that command
would have seen it — a hook gates a tool call, not what an already-running script does, and the set of ways a file
gets rewritten cannot be enumerated. This is the same argument ticket 02 made about filesystem events when it made
Reconciliation the guarantee instead of watching for particular events: prevention needs a complete list, insurance
does not. **And this is precisely the class of loss the product exists to insure — yet v1 does not cover it.**

Three resolved decisions stand in the way, and this ticket exists to decide whether one of them is redrawn:

- **The reserved-name rule is non-overridable** (ticket 10): a `.git` component is recorded once as Withheld
  `reserved_name` and never descended into, because libgit2's owner-less index rejects such a path on every platform.
- **The map places "any interaction with the watched directory's own `.git`" out of scope** — its index, HEAD,
  branches or stash.
- **An excluded directory is never entered** (ADR 0007), which is also how the watcher stays inside its budget, so
  `.git` produces no events for anything beneath it.

The sketch to accept, reject or narrow: an **explicitly enabled capture of the configuration-like files only** —
`config`, `info/exclude`, `hooks/*`, `description` — and never `index`, `objects`, `refs`, `HEAD`, `logs`, which is
what ticket 10 rejected `.git` for. Restore through a temp file and rename is mechanically safe here, since git holds
no lock on the config and reads it whole.

Settle:

- **Whether v1 takes this at all**, or whether it is post-release like ticket 18's mount and ticket 22's TUI. The
  destination is a buildable v1 spec, and the spec and tickets 47 and 48 are already written, so taking it is an
  amendment with a stated cost.
- **The exact path set**, and whether `hooks/*` belongs in it: a hook is executable code, so restoring one restores
  code, and a captured hook is a supply-chain artefact in a way `config` is not.
- **Credentials.** A repository config legitimately contains remote URLs, and a URL may embed a token
  (`https://user:token@host`). The secret denylist matches *names*, not contents, so capturing `config` can put a
  live credential into history — the exact outcome ticket 10's denylist exists to prevent. Decide whether that is
  refused, redacted (and how, given the tool refuses redaction elsewhere), captured knowingly, or gated by a
  content check.
- **How such a path is named in the tree**, given libgit2 rejects a `.git` component physically, and how Restore maps
  the stored name back to the real one — this is the "escaped name" ticket 10 rejected for `.git` as a whole, now
  proposed for four rarely-changing files.
- **Which configuration level may enable it.** Ticket 24's class test says a key deciding *what may enter history*
  can never come from a file arriving in someone else's repository, so this looks global-only; confirm, and decide
  whether it is per-Root capable through the global config's Root blocks.
- **How the watcher and pruning change.** ADR 0007 currently forbids entering `.git` at all, so either these four
  paths become a narrow exception in the pruning path (and take watches), or they are covered by Reconciliation only,
  which bounds the latency of the very loss this ticket is about.
- **What `explain?path=` and the Snapshot's entry listing say** about a `.git` path under each answer, so that the
  difference between "withheld because reserved" and "captured under a mapped name" is legible.
- **The linked-worktree and submodule case**, where `.git` is a regular file holding a gitdir pointer rather than a
  directory.
- **What the global config answer is**, independently: `~/.config/git/config` is an ordinary path in an ordinary
  directory and is already coverable by registering that directory, while `~/.gitconfig` is not, because a Watched
  Root is a directory and `$HOME` cannot be one. Decide whether the documentation states that plainly or whether
  something narrower is offered.
