# 90: The web UI does everything the CLI does

**Status:** migrated

**Migrated to:** #27

**What to build:** every operation a person can perform with `keepster` against a running daemon can also be
performed in the web UI, through the same `/v1` endpoints the CLI uses. The owner's requirement: "the web UI must be
able to do what the CLI does".

Registering a Root, unwatching one Root (with an optional purge) and recording a Move are designed separately (the
grilling session that produced this ticket) and are not part of it. This ticket is everything else.

Already in the web UI, read from `ui/src/lib/api.ts`: the Root list, the timeline (also for one path), a file's
content, the diff between two Snapshots, Restore, path search and content search, and the UI preferences.

Missing, by CLI command, with the endpoint that already serves it:

| CLI | Endpoint |
| --- | --- |
| `status` | `GET /v1/status` |
| `unwatch --all` | `DELETE /v1/roots/{id}` for each Root |
| `purge` (one Root or `--all`), erasing a Root no longer watched | `DELETE /v1/roots/{id}/history`, `GET /v1/unwatched-roots` |
| browsing the history of a Root no longer watched | `GET /v1/unwatched-roots`, then the reading endpoints by id |
| `snapshot` | `POST /v1/roots/{id}/snapshots` |
| `reconcile` | `POST /v1/roots/{id}/reconcile` |
| `diff` against the Working Tree (`--to working`, the CLI's default) | `GET /v1/roots/{id}/diff?to=working` |
| `cat` (one version, as a download) and `export` | `GET …/snapshots/{snapshot}/content`, `GET …/archive`, `POST /v1/roots/{id}/export`, `GET /v1/fs` |
| `explain` | `GET /v1/roots/{id}/explain` |
| `preview`, `watch --dry-run` | `GET /v1/preview` |
| `config` (effective configuration with provenance) | `GET /v1/roots/{id}/config` |
| `pin`, `unpin`, `pins` | `PUT`/`DELETE`/`GET /v1/roots/{id}/pins…` |
| `maintain`, `job` | `POST /v1/roots/{id}/maintenance`, `GET /v1/jobs/{id}` |
| `forget` | `POST /v1/roots/{id}/forget` |
| `ui token rotate` | `POST /v1/ui/token/rotate` |
| `log-level` | `PUT`/`DELETE /v1/log/level` |
| `mute`, `unmute` | `PUT`/`DELETE /v1/alarms/mute` |
| `diagnose` | `GET /v1/diagnostics`; the CLI assembles the bundle itself (`crates/keepster-daemon/src/diagnose.rs`), so a browser download needs a decision on where the bundle is built |

Not applicable to a client served by the daemon: `daemon`, `install`, `uninstall`, `service`, and `ui` itself.

Also asked for in the same session: **editing a Root's configuration** (Include Rules, Triggers, Retention) from the
web UI. The CLI cannot do this either, and the spec puts "Writing configuration through the API" out of scope
(`.scratch/keepster/spec.md`, Out of Scope), so it needs a spec decision before any work: a write endpoint for a
hand-edited, possibly git-tracked TOML.

- [ ] Every row of the table above is reachable in the web UI, with the CLI's consents kept separate from choosing
      the operation, as Restore's dialog already does
- [ ] The diff against the Working Tree is disabled, with the reason shown, for a Root no longer watched, whose
      Working Tree the daemon does not diff (`crates/keepster-daemon/src/diff.rs`), as decided for Restore
- [ ] Irreversible operations (purge, Forget) ask for consent as the Root management dialog does
- [ ] Jobs (Maintenance, Forget) show their progress from the event stream and their outcome from `GET /v1/jobs/{id}`
- [ ] Configuration editing is decided in the spec, and either specified or recorded as out of scope
- [ ] Tests of the plain modules cover each new client call and decision
- [ ] `ui/dist` is rebuilt and committed

## Comments

- Filed from the `/grill-with-docs` session on managing Roots in the web UI (2026-10-14).
- Related: ticket 86 (the open history follows `history.erased`, Retention and Forget).
- UI rule from the owner: the web UI never opens or switches a screen by itself; it shows a banner with a button to go
  there.
