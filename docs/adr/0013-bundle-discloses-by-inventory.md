# The support bundle discloses by inventory, and never redacts

A useful bundle carries the Registry, the effective configuration and the logs, and all three are full of the user's
paths — a path being disclosure in itself. The obvious move is a redaction pass: strip paths from log lines, replace
Root paths with surrogates. We do the opposite: **the bundle contains what it contains, and `keepster diagnose`
prints an inventory of every file and what it discloses, before writing anything** (`--dry-run` prints it without
writing). Nothing is ever sent anywhere. A reader who finds no redaction code will otherwise read its absence as an
oversight and add it.

## Considered Options

- **A redaction pass over logs and configuration.** Rejected because it makes a promise that cannot be kept. A path
  arrives in fragments: inside an errno message, inside a URL, in a glob from someone else's `.keepster.toml`, in
  the skipped-path list of an incomplete Snapshot, in a `Withheld` reason. One missed shape turns a stated guarantee
  into a false one, and a user who believed the guarantee shares more than they would have shared knowingly. No
  redaction is safer than redaction that is 95 % effective, because the residual risk is then understood.
- **Surrogate identifiers instead of paths** (RootId only, or salted hashes). Rejected as removing the most common
  defect class this bundle exists for — Include Rules not matching what the user expected, the question ticket 20's
  Capture Preview and ticket 07's `explain?path=` exist to answer — which cannot be diagnosed without the patterns
  and the paths they were matched against.
- **Selection flags** (`--no-logs`, `--no-config`). Rejected because a trimmed bundle is indistinguishable from a
  complete one, so every bug report would start by establishing what is absent; the fixed content plus the inventory
  gives the user the same control at the moment they choose to share.
- **Uploading the bundle.** Rejected outright, and recorded as a product invariant: the map places every remote out
  of scope, and this is the one command where egress would have looked natural.

## Consequences

- **The user is the filter, and is equipped to be one.** The inventory is part of the contract, so it lists every
  file, its source, and one line on what it reveals; the log's default format stays human-readable for the same
  reason (a log nobody can read cannot be reviewed before sharing).
- **One hard exclusion remains**, and it is not a redaction rule but a content rule: **no bytes of any file from
  history, under any flag.** There is no debugging reason to ship blobs, and their absence needs no pattern matching
  to guarantee.
- **The bundle may be pasted into a public issue tracker by a user who did not read the inventory.** That risk is
  accepted knowingly; the mitigation is that the inventory is printed, not buried in documentation.
- **A degraded bundle must say so.** With the daemon unreachable the CLI collects what is on disk, and the inventory
  names the missing sections rather than producing a smaller bundle that looks whole.
