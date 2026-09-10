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
Root config file is deliberately unrestricted: it may set any key, including the secret opt-in list and `ui.editor`.
The risk is accepted knowingly, not overlooked — cloning a repository that ships such a file lets its author
influence what your daemon captures and which command it launches. Flexibility was judged worth it for a
single-user local tool; if that judgement is ever revisited, the mitigation is a trust prompt on first sight of a
Root config, not a restricted key set.

**Merging is by key class, because the keys differ in nature.**

- *Pattern lists* (include, exclude, secret opt-in) layer like `.gitignore`: global layer first, Root layer
  second, last matching pattern wins, and a Root may negate a global rule rather than restate the whole list.
- *Sets and scalars* (which Triggers are active, debounce window, rate limit, retention, `ui.editor`, store
  location, HTTP port) are replaced wholesale by the Root level.

`ui.editor` being a scalar is what makes per-Root editors work: an Android codebase may open in Android Studio
while a web codebase opens in Zed. The command runs on the machine of whoever is viewing, so a value shared through
a repository is unlikely to fit anyone else's paths.

**Store location is configurable.** Default is the global store; a Root may instead keep its History Store
inside its own directory. Choosing that carries three obligations the spec must state: the store must be excluded
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
