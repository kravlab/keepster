# 24 — A CLI-only install: can the loopback listener be switched off

Type: grilling
Status: resolved

## Question

[05 — Daemon lifecycle](05-daemon-lifecycle.md) fixed one HTTP API on **two listeners** — a permission-protected
socket/pipe for the CLI and 127.0.0.1 plus token for the browser — and [07 — Daemon HTTP API and CLI
surface](07-api-and-cli-surface.md) built the browser's whole security story on top of the second one (per-start
token in a `0600` file, single-use bootstrap exchanged for an `HttpOnly` cookie, `Host`/`Origin` checks). Neither
ticket asked whether a user who never opens a browser may decline that listener.

The question is not the embedded SPA: [08 — Workspace structure](08-workspace-structure.md) already put those
assets behind the default-on `embed-ui` feature, so a binary built without it carries no web UI. The TCP listener
is a separate thing, and it is the product's only network-facing surface — which makes it the part a CLI-only
user actually has reason to care about. The HTTP server itself is not in question either: it is the CLI's own
transport over the socket, so `axum` stays regardless.

Settle:

- **Is the loopback listener switchable at all**, or unconditional? If switchable, the key, its level (daemon
  config only, since it is not a per-Root fact), and its default — on, or off until asked for.
- **What `keepster ui` does when it is off**: refuse and name the key to flip, or start the listener on demand for
  that session.
- **Whether `embed-ui` and the listener are one switch or two.** They are currently independent, and the
  interesting asymmetry is a binary with no embedded SPA but a live listener: ticket 07 made `/v1` public
  specifically so a third-party web or native UI can use it, and such a client needs the TCP listener without
  needing ours. So "no SPA" must not silently imply "no listener".
- **Consequences for the token**: with the listener off, is the per-start token still minted and its `0600` file
  still written, or does that whole path disappear — and does `keepster ui token rotate` then fail loudly?
- **How it surfaces in `status`**: ticket 05 made `keepster status` the single authority on health, so a disabled
  listener must be visible there as a deliberate configuration rather than read as a fault, and it must not affect
  the exit code.
- **Whether a headless install path declares it** (a package or a `service install` flag choosing the CLI-only
  posture up front), or whether it is only ever a config edit after the fact.

## Answer

**The loopback listener is switchable by a single key that exists only in the global daemon configuration, and it
defaults to on. With it off the token path disappears entirely, and `embed-ui` remains a separate switch.**

### First, a scope bug in ticket 04 that this ticket had to fix

Ticket 04 classified the **HTTP port** among the scalars "replaced wholesale by the Root level". That is wrong
twice over. A port is a fact about the daemon, not about a Root, so two Roots demanding different ports is
incoherent for one process. And ticket 04 deliberately left `.keepster.toml` unrestricted, accepting the risk that
a cloned repository's file influences what the daemon captures and which editor it launches — a risk accepted for
*capture policy*, never for the product's only network-facing surface. Under the published rule, a file arriving
in someone else's clone could relocate that surface.

**The port, and the new listener key, are global-only**, and the durable form of the fix is a class test rather
than a list: *a key that describes the daemon rather than a Root is never settable at the Root level.* Auditing
ticket 04's scalars against it, the port is the only offender — Triggers, debounce, rate limit, retention, store
location and `ui.editor` appeared genuinely per-Root, and ticket 04 argued `ui.editor` explicitly.

**That audit was later found too narrow.** Its test asks whether a key describes the daemon or a Root, and
`ui.editor` passes it, because an editor really is a per-project choice. But a second class exists that the test
does not catch: **a key that makes the daemon execute a command.** Such a key cannot come from a file that travels
in someone else's repository regardless of what it describes, and ticket 07's argv splitting bounds the damage
rather than removing the vector. `ui.editor` is therefore global-only too — see ticket 04's Comments.

### The listener is switchable, and on by default

Ticket 05 recorded the fact that makes the question real: **loopback TCP is not user-scoped on any of the three
platforms.** The socket is bounded by OS permissions; the port is bounded by a `0600` token file plus `Host` and
`Origin` checks, and any local process — including one belonging to another user on the machine — can reach it.
That is the part a CLI-only user has a reason to decline, and until now they could not.

Default **on**, because the web UI is a first-class v1 surface (tickets 09, 15 and 16 built a timeline, a diff
editor and a theme system on it) and a default that makes the flagship UI require a configuration edit before
first use is a bad trade for a security gain that is small on a single-user machine.

**Default off with on-demand binding was rejected**, though it is the tempting answer. It would give a minimal
surface for free, but it buys that with a second lifecycle: a listener appearing while the daemon runs, a port
collision surfacing at request time rather than at startup, and a `status` row that changes with no configuration
change behind it. It also does not deliver what it promises — the daemon cannot learn that the browser was closed,
so "on demand" degenerates into "from the first use until the next restart". Ticket 05's whole posture is that
lifecycle facts are established at start and are inspectable; the listener's existence belongs in that category.

The key is **restart-required**, the same class ticket 04 already assigns the port, and ticket 04's rule applies:
the daemon says so rather than silently ignoring the edit.

### With the listener off

- **`keepster ui` refuses**, naming the key, the configuration file holding it, and the restart — `keepster service
  restart`, which ticket 21 added to the CLI. It does not start the listener for that session, because that is the
  second lifecycle rejected above.
- **The token path disappears entirely.** No per-start token is minted and no `0600` file is written. A secret that
  guards nothing is a liability rather than a convenience: it sits on disk looking like a credential, it is copied
  by whatever backs the runtime directory up, and it invites the reader to wonder what it opens. `keepster ui token
  rotate` fails with the same stable error code as `keepster ui`. This also makes `status` honest, since the
  listener row becomes the only place that says whether the daemon has a network surface, with no token file lying
  beside it.
- **The tray's "open web UI" item is disabled** and explains why in the same words the CLI uses — ticket 21's
  invariant that the tray is a view, never a source, applies to unavailability as much as to state.
- **`status` shows it as a deliberate configuration, not a fault**, and it does not affect the exit code, by the
  same reasoning ticket 21 used for the tray's capability rows: the non-zero exit belongs to things that impair the
  insurance, and declining a UI surface does not.

### `embed-ui` and the listener are two switches, never one

Ticket 08 put the SPA assets behind a default-on `embed-ui` feature. **A build without `embed-ui` does not change
the listener's default**, which stays on. Ticket 07 made `/v1` a public contract precisely so a third-party web or
native client could use it, and such a client needs the TCP listener without needing our SPA. Collapsing the two
into one switch would silently remove the surface that decision exists to provide.

The four combinations are all meaningful: SPA and listener (the default); listener without SPA (a third-party
client); SPA without listener (the assets are inert, which is wasteful but harmless, and is what a distributor's
default build looks like on a headless box); neither (the CLI-only posture in its smallest form).

### Declaring the posture at install time

`keepster service install --no-web-ui` **writes the key only when it creates the configuration file**; if a
configuration already exists it changes nothing and prints the exact line to add, plus the restart reminder. This
keeps ticket 07's reason for refusing configuration writes intact — editing an existing TOML means preserving
comments, key order and formatting, or handing the user a whole-file diff — while still giving a provisioning
script the single step it needs on a fresh machine.

Without it the headless path is four steps with a wasted bind in the middle (install, start, edit, restart), which
is how the step comes to be skipped. The file remains the single source of truth: the flag is a way to produce it,
not a second mode, and nothing about the daemon's behaviour depends on how the key got there.
