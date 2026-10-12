# 60: The loopback listener and the browser's way in

**Blocked by:** 36 (Workspace skeleton, the daemon and `keepster status`)

**Status:** done

**What to build:** a user can open the web UI from the CLI and land in an authenticated browser session with no token
in the address bar — and a user who only wants the CLI can switch the network surface off entirely.

The socket needs no token, because OS permissions are the boundary. The loopback listener does, and the real threat is
the browser: any page the user has open can post to loopback, and through DNS rebinding can claim its own origin. So
the token never travels in a URL beyond a single-use bootstrap code, and every request is checked twice.

- [x] A token is minted per daemon start into a `0600` file in the runtime directory
- [x] `keepster ui` opens the browser with a single-use bootstrap code, which the daemon exchanges for an
      `HttpOnly`, `SameSite=Strict` cookie, burns, and redirects away from
- [x] Every request checks that the host header is loopback with the expected port and that the origin, when present,
      is in the allowed list, which is configurable and never a wildcard
- [x] `keepster ui token rotate` revokes explicitly, and a daemon restart rotates implicitly
- [x] The listener is switchable, defaults to on, and lives only in the global configuration, because a key
      describing the daemon is never settable per Root
- [x] With the listener off the **whole token path disappears** — no mint, no file — and `keepster ui` fails with the
      same code as a rotate attempt; `status` is the only place that says whether a network surface exists
- [x] The install command's no-web-UI switch writes the key only when it creates the configuration file, so an
      existing file is never round-tripped — *the step it calls, `keepster_service::declare_no_web_ui`; the
      flag itself lands with `keepster service install` in ticket 61*
- [x] Embedding the SPA stays a separate build switch: a build without it keeps the listener on, because the API is
      public for third-party clients — *true of today's build, which embeds none; the `embed-ui` feature itself
      is ticket 64's*

## Comments

- Ticket 42 left the restart-required mechanism waiting for its first key, which is this ticket's listener
  switch: declare it in `keepster_app::config::SCHEMA` with `live: false` and `root_settable: false`. Its value
  is taken from the global file's own table when the daemon starts and kept until it restarts; an edit then
  shows `restart_required: true` on `GET /v1/roots/{id}/config`. Two things remain to do here: a daemon key
  must be ignored with a warning in `[root.…]` and `[[roots]]` blocks too, not only in a Root's own file, and
  the end-to-end test for ticket 42's restart-required criterion.
- Done. The keys are `ui.listener` (default on), `ui.port` (default 7419) and `ui.allowed_origins` (default
  empty: the listener's own `http://127.0.0.1:<port>` and `http://localhost:<port>` are always allowed),
  all global-only and restart-required. A daemon key in a Root's file, a `[root.<id>]` block or a
  `[[roots]]` block is ignored with `key_ignored`; a wildcard origin is `config_invalid`. Ticket 42's
  restart-required criterion is tested end to end with `ui.listener`.
- The token is `ui-token` in the per-user runtime directory (`$XDG_RUNTIME_DIR/keepster`, `$TMPDIR/keepster`,
  `%LOCALAPPDATA%\Keepster\run`), created `0600`, removed at a clean stop and at a start with the listener
  off. The browser's cookie, `keepster-<port>`, carries the token, so rotating or restarting ends every
  session; a third-party client sends it as `Authorization: Bearer`. `/v1/version` and `/v1/openapi.json`
  are served before authentication, with `Host` and `Origin` still checked. An allowed foreign origin is
  answered by name with credentials, never `*`.
- The way in is two control-channel-only endpoints, `POST /v1/ui/bootstrap` and `POST /v1/ui/token/rotate`:
  the loopback listener serves every other route and not these, so a browser session cannot mint codes.
  With the listener off (or failed to bind) both answer `listener_unavailable` (409, exit 5), which is
  what makes `keepster ui` and `keepster ui token rotate` fail alike. `status` carries a `listener` row
  (`listening` with its address, `off`, or `failed` with why); it never changes the exit code.
- A listener that cannot bind does not stop the daemon: capture is the insurance and the port is not, so
  the row says `failed` and the browser commands refuse.
- **The install flag itself is ticket 61's**, since `keepster service install` does not exist yet:
  `keepster_service::declare_no_web_ui` is the step it calls — it writes `[ui] listener = false` only
  with `create_new`, and otherwise returns the line to add — tested end to end against the daemon.
- Until ticket 64 embeds the SPA, `/` serves a one-line page saying the build carries no web UI; there is
  no `embed-ui` feature yet, and the listener's default does not depend on one.
- Left open from review, for a decision rather than a patch: a cookie is scoped to a host and never to a
  port, so the browser sends `keepster-<port>` to any other server on `127.0.0.1` the user visits —
  another local user's included. Ticket 07 chose the cookie knowing loopback is not user-scoped (ticket
  05); an ADR should record whether that stands or the session moves off a host-wide cookie. Bootstrap
  codes are single-use and die with a rotate or a restart, but have no expiry of their own.
