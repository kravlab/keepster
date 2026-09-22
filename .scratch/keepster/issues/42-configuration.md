# 42: Configuration — two levels, provenance, automatic reload, `Degraded`

**Blocked by:** 41 (Include Rules, the secret denylist, the size cap and `explain`)

**Status:** ready-for-agent

**What to build:** a user configures Keepster by editing TOML and sees the effect a second later, can read the
effective configuration with, per key, the level that set it, and is protected from a typo silently changing what is
captured — or from someone else's repository changing it at all.

Two levels: a global daemon configuration, and an optional file in the watched directory. The global configuration
carries per-Root blocks addressed by Root id and pattern blocks addressed by a path glob, both accepting the whole
schema, which is what makes the in-tree file optional. Three classes of key can never come from it: keys describing
the daemon, keys that make the daemon execute something, and keys deciding where the user's data goes or what may
enter it.

- [ ] Precedence is `default`, `global`, `global-pattern`, `global-root`, `root`, and that enum is what the API
      reports as provenance
- [ ] An id block outranks a pattern block, and among pattern blocks the last match wins
- [ ] Pattern lists layer with negation; sets and scalars are replaced wholesale by the more specific level
- [ ] A Root file setting an excluded key is ignored with a warning and does **not** make the Root `Degraded`
- [ ] Editing either level reloads automatically; a key that cannot take effect live is reported as
      restart-required instead of silently ignored
- [ ] An invalid configuration is never applied: the last known-good stays in force and the Root is `Degraded` with
      a machine-readable reason; with no known-good version the Root is not watched at all
- [ ] The configuration resource is read-only and returns the effective values with per-key provenance; `keepster
      config` mirrors it
- [ ] The documented defaults apply to a user who configures nothing
- [ ] A reload that changes classification inputs queues the Root for a full comparison
