# 24 — A CLI-only install: can the loopback listener be switched off

Type: grilling
Status: open

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
