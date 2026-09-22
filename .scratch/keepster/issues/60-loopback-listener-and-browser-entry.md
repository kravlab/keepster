# 60: The loopback listener and the browser's way in

**Blocked by:** 36 (Workspace skeleton, the daemon and `keepster status`)

**Status:** ready-for-agent

**What to build:** a user can open the web UI from the CLI and land in an authenticated browser session with no token
in the address bar — and a user who only wants the CLI can switch the network surface off entirely.

The socket needs no token, because OS permissions are the boundary. The loopback listener does, and the real threat is
the browser: any page the user has open can post to loopback, and through DNS rebinding can claim its own origin. So
the token never travels in a URL beyond a single-use bootstrap code, and every request is checked twice.

- [ ] A token is minted per daemon start into a `0600` file in the runtime directory
- [ ] `keepster ui` opens the browser with a single-use bootstrap code, which the daemon exchanges for an
      `HttpOnly`, `SameSite=Strict` cookie, burns, and redirects away from
- [ ] Every request checks that the host header is loopback with the expected port and that the origin, when present,
      is in the allowed list, which is configurable and never a wildcard
- [ ] `keepster ui token rotate` revokes explicitly, and a daemon restart rotates implicitly
- [ ] The listener is switchable, defaults to on, and lives only in the global configuration, because a key
      describing the daemon is never settable per Root
- [ ] With the listener off the **whole token path disappears** — no mint, no file — and `keepster ui` fails with the
      same code as a rotate attempt; `status` is the only place that says whether a network surface exists
- [ ] The install command's no-web-UI switch writes the key only when it creates the configuration file, so an
      existing file is never round-tripped
- [ ] Embedding the SPA stays a separate build switch: a build without it keeps the listener on, because the API is
      public for third-party clients
