# 42: Configuration — two levels, provenance, automatic reload, `Degraded`

**Blocked by:** 41 (Include Rules, the secret denylist, the size cap and `explain`)

**Status:** done

**What to build:** a user configures Keepster by editing TOML and sees the effect a second later, can read the
effective configuration with, per key, the level that set it, and is protected from a typo silently changing what is
captured — or from someone else's repository changing it at all.

Two levels: a global daemon configuration, and an optional file in the watched directory. The global configuration
carries per-Root blocks addressed by Root id and pattern blocks addressed by a path glob, both accepting the whole
schema, which is what makes the in-tree file optional. Three classes of key can never come from it: keys describing
the daemon, keys that make the daemon execute something, and keys deciding where the user's data goes or what may
enter it.

- [x] Precedence is `default`, `global`, `global-pattern`, `global-root`, `root`, and that enum is what the API
      reports as provenance
- [x] An id block outranks a pattern block, and among pattern blocks the last match wins
- [x] Pattern lists layer with negation; sets and scalars are replaced wholesale by the more specific level
- [x] A Root file setting an excluded key is ignored with a warning and does **not** make the Root `Degraded`
- [x] Editing either level reloads automatically; a key that cannot take effect live is reported as
      restart-required instead of silently ignored
- [x] An invalid configuration is never applied: the last known-good stays in force and the Root is `Degraded` with
      a machine-readable reason; with no known-good version the Root is not watched at all
- [x] The configuration resource is read-only and returns the effective values with per-key provenance; `keepster
      config` mirrors it
- [x] The documented defaults apply to a user who configures nothing
- [x] A reload that changes classification inputs queues the Root for a full comparison

## Comments

- **Scope decisions taken with the user before implementation.**
  - *Block names.* Ticket 04's `[roots.<RootId>]` and `[[roots]]` cannot share one TOML document — `roots` cannot
    be a table and an array at once — so per-Root blocks are `[root.<RootId>]` and pattern blocks stay
    `[[roots]]` with a `path` glob. The glob is the rules' own syntax, matched against the whole absolute
    path with `/` separators on every platform; a leading `~/` is the user's home, and anything that is not
    absolute after that is refused.
  - *The known-good survives a restart.* Each file's last valid text is kept on disk —
    `<store base>/config.known-good.toml` for the global file, `<store base>/<RootId>/keepster.known-good.toml`
    for a Root's own — so a typo followed by a reboot leaves the Root `Degraded`, not unwatched. An absent file
    is a valid version (the defaults): only a file that was broken from the first time the daemon read it leaves
    a Root with no known-good, `Unavailable` with `state_code: config_invalid`, refused by capture, `explain`
    and `GET /v1/roots/{id}/config` with `409 config_invalid`. `Degraded` uses the same state code.
  - *Restart-required and the full comparison: the mechanism now, the end-to-end proof later.* No key in this
    build is restart-required and Reconciliation does not exist. `KeySpec::live` marks a key that cannot change
    live: its value is taken from the global file's own table when the daemon starts and kept until it
    restarts, with `restart_required: true` on the wire while the file says otherwise — ticket 60's listener is
    the first such key (see its Comments). A change to any key with `classifies` queues the Root in
    `Configuration::take_reconcile_queue()`, comparing values only, so moving a rule to another line queues
    nothing — ticket 44 drains it (see its Comments).
  - *Reload.* Every request that needs a Root's configuration re-reads its two files and compares them with
    the text read last, and a one-second tick does the same for every Root, so nothing waits for a client.
    Ticket 43's watcher can make the tick event-driven; the comparison stays the authority.
- **Pattern blocks**: a Root matching several takes **the last one alone**, as ticket 04 words it ("takes the
  last one"), lists included — so `~/code/android/*` written after `~/code/**` replaces it rather than adding
  to it. Within the levels that do apply, lists layer and scalars are replaced.
- **The schema is closed.** Unknown tables and keys are refused anywhere in either file, not only in `[files]`,
  so a misspelt `[fils]` is an invalid configuration rather than silently nothing. A Root's own file may not
  hold `[root.…]` or `[[roots]]` blocks.
- `GET /v1/roots/{id}/config` (`get_root_config`, tag `configuration`) returns every key in schema order with
  `value` (typed by the key; `max_size_by_pattern` entries are `{ pattern, max }`), `origin` (the level, file
  and line that decided it; a list's is its last entry's), `entries` with per-entry provenance for lists,
  `restart_required`, plus `warnings` and `errors` (the invalid files, each a Problem with `config_invalid`
  and a `path` detail). `keepster config [-r ROOT] [--json]` mirrors it.
- **Locks**: the Registry is always taken before the configuration, and the configuration is resolved while
  the Registry is held — resolving may write a known-good copy into a Root's store directory, which an erasing
  deregistration must not be removing at that moment.
- Not asserted by a test: that the one-second tick, rather than a request, performs a reload. Every observer
  is itself a request that reloads, so nothing outside the daemon can tell the two apart until ticket 45's
  events give a reload an observable effect of its own.
