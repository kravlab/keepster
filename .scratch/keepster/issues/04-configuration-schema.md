# 04 — Configuration schema and precedence

Type: grilling
Status: resolved

## Question

Nearly every behavioural decision so far ended in "the user configures it". What is the configuration model?

Settle:

- levels and precedence: a global config versus per-Root overrides, and where each physically lives (a
  per-Root file inside the watched directory is visible to teammates and to git; one held by the daemon is not);
- format and why;
- what is configurable: which triggers are active (watcher debounce, periodic floor, full scan), the debounce
  window, the rate limit, retention, include/exclude rules, the `.gitignore` opt-in flag, the explicit secret
  opt-in list, `ui.editor` and its `{file}:{line}:{column}` template, HTTP port and token handling;
- defaults for a user who configures nothing;
- reload semantics: does the daemon watch its own config, or does a change require a restart or an explicit
  command;
- what happens on an invalid config: refuse to start, or run with defaults and report degraded state.

## Answer

**Two levels, TOML.** A global daemon config, plus `.keepster.toml` at the root of a watched directory. The
Root config file is unrestricted **within capture policy**: it may set any key that decides what this Root
captures — its include and exclude rules above all. The risk is accepted knowingly, not overlooked — cloning a
repository that ships such a file lets its author influence what your daemon captures. Flexibility was judged
worth it for a single-user local tool, and ticket 20's Capture Preview later turned the mitigation this answer
deferred — a trust prompt on first sight of a Root config — into a visible one.

**Three classes of key are excluded from the Root file**, and none of them is capture policy:

- **Keys that describe the daemon rather than a Root** — the HTTP port and the loopback listener. A port is one
  fact per process, so two Roots demanding different ones is incoherent, and a file arriving in a clone must not
  relocate the daemon's only network surface. Settled by [24 — A CLI-only install](24-cli-only-install.md).
- **Keys that make the daemon execute something** — `ui.editor`. Splitting the template into argv bounds the
  damage; it does not make it acceptable for a file travelling in someone else's repository to choose which binary
  runs on your machine.
- **Keys that decide where your data goes or what may enter it** — the History Store location and the secret
  opt-in list. A Root file choosing the store location can put your history inside the working directory, which
  this answer itself warns is destroyed by `rm -rf`, `git clean -xdf` or a bad checkout; a Root file opting a path
  in past the built-in secret denylist can put `.env` or a private key into history. Both are decisions about the
  user's own exposure, and reload is automatic, so an ordinary `git pull` would otherwise change them silently on
  an already-registered Root.

**The capability is relocated, not removed.** Per-Root values for all of these stay available — in the *global*
config, which is the user's own file and travels in nobody's clone. It grows two kinds of per-Root block:

- `[roots.<RootId>]`, addressed by the surrogate id ticket 13 made the identity, so moving or renaming the
  directory never detaches the override;
- `[[roots]]` with a `path` glob, which is the readable form and the one that covers Roots not yet registered —
  "every Android project opens in Android Studio".

**An id block outranks a pattern block**, and a Root matching several pattern blocks takes the last one, by the
same last-match-wins rule the pattern lists already use.

**These blocks accept the whole schema, not just the relocated keys.** Everything a `.keepster.toml` may say about
a Root — its include and exclude rules included — can be said in the global config instead, merged by the same key
classes below. The consequence is the point: **`.keepster.toml` is entirely optional and nothing requires it to
exist.** It stops being the mechanism for per-Root configuration and becomes one way to express it — the way that
travels with a repository, for a team that wants its rules shared. A user who never creates one is not exposed to
the clone vector at all, and Keepster asks them for nothing in the watched directory.

So the levels become, in increasing precedence: `default`, `global`, `global-pattern`, `global-root`, `root`. The
Root file remains the outermost layer **for the keys it may still set**; for the three excluded classes the
effective outermost layer is `global-root`. A Root file that sets an excluded key is **not** invalid and does not
make the Root `Degraded` — the key is ignored, and the daemon says so rather than dropping it silently, in the
same spirit as ADR 0007's warning for an unreachable negation.

**Merging is by key class, because the keys differ in nature.**

- *Pattern lists* (include, exclude, secret opt-in) layer like `.gitignore`: global layer first, Root layer
  second, last matching pattern wins, and a Root may negate a global rule rather than restate the whole list. The
  secret opt-in list layers the same way but has no Root-file layer at all, per the exclusion above.
- *Sets and scalars* (which Triggers are active, debounce window, rate limit, retention, store location) are
  replaced wholesale by the Root level.

The per-Root editor this answer originally argued for — an Android codebase opening in Android Studio while a web
codebase opens in Zed — is a real need, and it is met by the `[[roots]]` pattern blocks above, in the **global**
config, which is the user's own file and travels in nobody's clone.

**Store location is configurable**, though only from the global config after the relocation above. Default is the
global store; a Root may instead keep its History Store inside its own directory, chosen in that Root's
`[roots.<RootId>]` block rather than in its `.keepster.toml` — precisely because the third obligation below is a
way to lose the history, and a stranger's file must not opt you into it.

Choosing that carries three obligations the spec must state: the store must be excluded
from its own watch (or snapshots feed themselves events), it must be added to the watched directory's
`.gitignore` (or it pollutes `git status`), and the user must understand that `rm -rf`, `git clean -xdf` or a
bad checkout then destroys the history together with the directory it was insuring.

**Reload is automatic** on a change to either level; the Root config file already sits inside the watched tree, so the
daemon is watching it anyway. Keys that cannot take effect live (the HTTP port) are marked in the schema as
restart-required, and the daemon says so instead of silently ignoring the edit.

**An invalid config is never applied.** The last known-good configuration stays in force and the Root is marked
Degraded. If no known-good version ever existed, the Root is not watched at all: falling back to built-in
defaults would drop the user's exclude rules and pull into history exactly what those rules existed to keep out.

**Defaults for a user who configures nothing**: watcher with a ~3 s debounce; rate limit of one Snapshot per 60 s;
Reconciliation at daemon start and on every OS rescan signal; periodic floor Snapshot off; `.gitignore` not
honoured (`files.respect_gitignore = false`); secrets refused.

**Reconciliation is configurable, and on by default.** Disabling it turns off only the startup walk — the response
to an OS rescan signal (`IN_Q_OVERFLOW`, `MustScanSubDirs`, a Windows buffer overflow) stays mandatory, because
there the OS has explicitly announced lost events. A Root whose startup walk is disabled must be reported as
"coverage not guaranteed" in daemon status, so the tool never looks healthier than it is. Evidence for why this
matters is in [02 — Reliability limits of cross-platform filesystem watching](02-filesystem-watching.md).

**Settled in passing**: the Root config file is `.keepster.toml` in the watched directory root, and it is an
ordinary file in the tree — it appears in Snapshots and in git like any other. Surfacing the Degraded and
"coverage not guaranteed" states belongs to
[05 — Daemon lifecycle and autostart on three platforms](05-daemon-lifecycle.md), which already asks how a user
sees that snapshotting is actually working.

## Comments

- Superseded in part by [24 — A CLI-only install](24-cli-only-install.md): the **HTTP port** was listed among the
  scalars replaced at the Root level, which would let a `.keepster.toml` arriving in a clone relocate the daemon's
  only network surface. Daemon-level keys are global-only.
- Superseded in part by a later user decision: **`ui.editor`, the History Store location and the secret opt-in
  list leave the Root file.** Ticket 24's audit of this list used the test "does the key describe the daemon or a
  Root", which all three pass — an editor, a store location and a secret exception genuinely are per-project
  choices. The test was too weak. Two further classes cannot come from a file that arrives in someone else's
  repository whatever they describe: keys that make the daemon *execute a command*, and keys that decide *where
  the user's data goes or what may enter it*. Ticket 07's no-shell argv rule stays, as depth rather than as the
  boundary.
- The per-Root **capability** survives in full: the global config gains `[roots.<RootId>]` and `[[roots]]`
  pattern blocks, so everything that could be said per project can still be said per project — in the user's own
  file. Those blocks accept the **whole** schema, not only the relocated keys, which makes `.keepster.toml`
  optional: it becomes the way to share rules through a repository rather than the way to configure a Root.
  Provenance grows from `default / global / root` to
  `default / global / global-pattern / global-root / root`, which ticket 13 owns as a wire-visible identifier.
