# 41: Include Rules, the secret denylist, the size cap and `explain`

**Blocked by:** 38 (The first Snapshot — History Store invariants and the write path)

**Status:** done

**What to build:** a user's own include and exclude rules decide what a Snapshot captures, secrets stay out by
default, an over-large file is recorded rather than silently missing, and the user can ask why any specific path is
or is not captured — with the rule that decided it named.

Rules are layered pattern lists with negation and last-match-wins, evaluated as pure values. **An excluded directory
is never entered**, so nothing beneath it can be re-included (ADR 0007) — git's rule, adopted because watches are
installed after the rules are applied. Git's silence is not adopted with it: the explain answer names the pruning
ancestor, and loading rules warns for every negation an ancestor makes unreachable.

- [x] Include and exclude rules layer with negation, the last matching pattern wins, and the matcher is tested as
      values with no disk
- [x] An excluded directory is never entered, and a negation beneath one is reported as unreachable with a warning
      that does not make the Root `Degraded`
- [x] The built-in secret denylist refuses the documented name classes by default, and an explicit opt-in list can
      re-admit a specific path
- [x] A file over the default size cap is recorded as Withheld with the too-large reason, and a per-pattern cap
      overrides the scalar
- [x] `explain?path=` and `keepster explain` answer whether a path is captured, which rule at which level decided
      it, and which ancestor pruned it where one did
- [x] Ignore files of the watched directory are **not** honoured by default, and the opt-in layer sits beneath the
      user's own rules
- [x] Capture applies the rules, so a Snapshot of a tree containing excluded paths holds none of them

## Comments

- **Three scope decisions were taken with the user before implementation.**
  - *`include` is a list of negations.* Within one level, each `files.include` entry acts as `!pattern` evaluated
    after that level's `files.exclude`, so a level's own exceptions win over its own exclude rules, and across
    levels the more specific level wins either way. Everything is captured unless a rule says otherwise. An
    `include` entry starting with `!` is refused.
  - *A minimal loader, not ticket 42's configuration.* Only the `[files]` table is read, from the global
    `config.toml` (`$XDG_CONFIG_HOME/keepster/`, `~/Library/Application Support/Keepster/`,
    `%APPDATA%\Keepster\`) and the Root's `.keepster.toml`, at every capture and every `explain` — so an edit
    takes effect at the next one. Per-Root and pattern blocks, the configuration resource with provenance,
    reload, the last known-good and `Degraded` stay with ticket 42. A configuration that cannot be read refuses
    the capture with `409 config_invalid` rather than falling back to defaults, which ticket 04 forbids because it
    would drop the user's exclude rules. `[files]` refuses unknown keys for the same reason: a misspelt
    `exlude` would otherwise capture what it meant to keep out. A consequence to carry: the documented but
    unimplemented `files.follow_symlinks` (ticket 49) is refused too, until that ticket teaches the reader it.
  - *Withheld `too_large` is named, not yet recorded.* The file has no tree entry (ADR 0009) and appears in the
    capture's answer as `withheld: [{ path, reason }]` and in `explain`; the persistent sidecar record is
    ticket 48's, as the `reserved_name` record already was.
- **The rule engine is git's ignore syntax, compiled to `regex`** (the one dependency the spec allows `core`):
  `*`/`?` stop at `/`, `**/`, `/**/` and a trailing `/**` cross directories while any other `**` is an ordinary
  `*`, `[…]` takes `!`/`^`, escapes and named classes such as `[:digit:]` and never matches `/`, a `/` anywhere
  but the end anchors, a trailing `/` means directories only. Levels are ordered `default < ignore-file <
  global < global-pattern < global-root < root`; `ignore-file` is new on the wire and names a rule from a
  watched directory's `.gitignore`, which is honoured only with `files.respect_gitignore = true` and sits beneath
  everything the user writes. A deeper `.gitignore` outranks a shallower one, as in git.
- **The walk applies the policy as it goes**, so an excluded directory is never listed (asserted on Linux with a
  non-UTF-8 name inside one, which any walk that met it would report as unreadable). Order of questions:
  reserved name, the user's rules, the secret denylist unless opted in, then the size cap.
- **The secret denylist** refuses `.env*`, `*.pem`, `*.key`, `*.p12`, `*.pfx`, `*.jks`, `*.keystore`, `id_rsa`,
  `id_dsa`, `id_ecdsa`, `id_ed25519`, `.netrc`, `.pgpass` and `local.properties`, matched by name at any depth and
  on files only. `files.secret_opt_in` is read from the global file alone; in a `.keepster.toml` it is ignored
  with a `key_ignored` warning, as ticket 04 requires.
- **Size caps**: `files.max_size` (default 100 MiB) and `files.max_size_by_pattern = [{ pattern, max }]`, last
  match wins; a pattern written for a directory (`media/`) covers the files beneath it, and a negated entry, which
  needs no `max`, returns its paths to the scalar.
- **`GET /v1/roots/{id}/explain?path=` and `keepster explain PATH [--json]`** answer with a `decision`
  (`captured`, `excluded`, `pruned`, `secret`, `withheld` with an open `reason`), the deciding `rule` with its
  pattern, effect, level, source file and line, the pruning ancestor, the path's own rule even when an ancestor
  overrides it, what is on disk, the cap and where the scalar cap was set, and every configuration warning. A
  symlink or special file is reported `withheld` (`symlink`, `special_file`), never `captured`, because the walk
  passes them over until tickets 48 and 49. A path that is not on disk is explained as a file.
- **Warnings never make a Root `Degraded`.** `negation_unreachable` is reported for every negation whose
  literally-named ancestor directory is excluded. **Partial against ADR 0007's "every negation":** an unanchored
  negation such as `!*.keep` has no ancestor to check without a walk, and negations inside `.gitignore` files are
  not examined at load, since reading them is a walk too. `explain` answers both cases for a given path.
- **New wire surface, all additive:** `ErrorCode::ConfigInvalid`; `Snapshot.withheld` with `reason` as an open
  string, as ADR 0005 argues for codes, because ticket 48 adds reasons inside `/v1`; the `Explanation`,
  `Decision`, `OnDiskKind`, `RuleReference`, `RuleEffect`, `Origin`, `ConfigLevel` and `ConfigWarning` schemas.
  `openapi.json` was regenerated by `make openapi`.
- **Review**, both axes, before the commit. Fixed: `**` inside a name crossed `/`; bracket classes mishandled `\`
  and named classes; an escaped trailing space in `.gitignore` was dropped; a cap written for a directory never
  applied; `explain` named neither the opt-in that admitted a secret nor where the scalar cap was set, and called
  links and special files captured; the withheld reason and the provenance level were a closed enum and a free
  string respectively, now the other way round; a name the walk could not re-parse made a Snapshot incomplete and
  a bad directory was silently treated as the Root, now paths are built from the walk's own components;
  `CapturePolicy`'s invariants were behind public fields; the ancestor walk was written four times and is now
  `RootPath::ancestors`; "exclusions" (on CONTEXT.md's _Avoid_ list) is gone. Kept, with reason: the
  binary search in `Walked::keeps`, which replaced a quadratic scan the new walk would have made worse.
- Verified: `cargo fmt --check`, `clippy --all-targets --all-features -D warnings`, the CLI dependency gate,
  `cargo deny check licenses bans`, `cargo +1.87 check`, and the whole suite — as root 4 permission tests fail
  as before (root reads a mode-`000` file), and as an unprivileged user every test passes. No test has run on
  Windows or macOS.

