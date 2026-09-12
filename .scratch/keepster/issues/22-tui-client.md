# 22 — A TUI client

Type: grilling
Status: wontfix

## Question

An interactive terminal client: the Snapshot timeline, a file's chronology, diffs and Restore, driven by keyboard
in a terminal instead of a browser.

The motivation is not taste. The web UI is served on 127.0.0.1 with a per-start token (tickets 05 and 07), so it
needs a browser on the same machine — which a headless server, a container, or an SSH session does not have
without port forwarding. On those machines the CLI is the only surface, and the CLI is built for single questions
(`status`, one Restore, one diff), not for browsing history.

Settle, if this is ever picked up:

- **Whether it is a separate binary or a subcommand.** The tray (ticket 21) already threatens the "one binary plus
  an embedded SPA" shape; a TUI could instead live inside the CLI binary, which links neither `git2` nor `rusqlite`
  nor `core` (ticket 08) and would keep that boundary intact.
- **What it reuses rather than reinvents.** Diffs are computed by the daemon (ticket 07) and ticket 09 chose
  CodeMirror 6 for the web UI precisely because it renders hunks computed elsewhere — so a TUI consumes the same
  hunks and the diff engine does not fork. Anything it renders differently from the web UI is a decision, not an
  accident.
- **Whether Restore is reachable from it**, and how a destructive action is kept off a single keystroke — the same
  question ticket 15 asks of the web UI, with fewer affordances to answer it.
- **Terminal capability**: colour depth (16 / 256 / truecolour), diff contrast on light and dark terminals — the
  terminal analogue of ticket 16 — and behaviour over a dumb or non-UTF-8 terminal.
- **Whether the web UI still earns its place** if a TUI exists, or the two surfaces split by machine: browser on a
  desktop, TUI over SSH. Two interactive clients for the same read paths is a cost, and the answer must say what
  each one is for.
- **Which listener it uses.** Unlike the tray, a TUI is a terminal process running as the user, so it can use the
  CLI's permission-protected socket or named pipe and needs no new authentication story (ticket 07).

## Comments

Deferred past v1 by user decision (2026-09-11), alongside mounting
([18 — Mounting a Snapshot as a filesystem](18-mount-snapshots.md)). Recorded as a file rather than a single map
line so the reasoning above survives for whoever picks it up. Out-of-scope work does not graduate: if this returns,
it returns as a fresh effort, not as a resumption of this map.
