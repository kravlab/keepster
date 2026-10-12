# 57: Capture Preview

**Blocked by:** 41 (Include Rules, the secret denylist, the size cap and `explain`), 42 (Configuration — two levels,
provenance, automatic reload, `Degraded`)

**Status:** done

**What to build:** before registering a large directory — or after registering one — a user can see what would be
captured and what would be excluded, grouped by the rule that decided, without capturing anything.

A Capture Preview classifies a directory against the rules that would govern it. It is streamed rather than a job,
because a dropped connection strands nothing; it never opens the History Store, takes no lock, counts as no Trigger,
and aborts when the client disconnects. It reports **content to capture** and refuses to guess store growth, which is
unknowable without hashing every file — that being the capture.

- [x] The preview streams rows as it walks, with the first row arriving promptly on a large tree
- [x] Output groups by the rule that decided, with the largest entries shown on both sides and a full-depth form for
      the detailed view
- [x] It works for a registered Root and for a directory that is not one, honouring an in-tree configuration file
      exactly as registration would and **showing** what that file sets
- [x] It opens no History Store, takes no lock, counts as no Trigger, and stops when the client goes away
- [x] It states no store-growth estimate
- [x] `keepster watch --dry-run` is an alias for it
- [x] Registering a directory above the claimed per-Root ceiling runs the preview and asks for explicit confirmation
      rather than refusing
- [x] Pending paths and the age of the oldest pending change are reported in `status` instead of listing the debounce
      queue

## Comments

- Ticket 43 delivered this ticket's status criterion: every Root carries `pending_paths` and
  `oldest_pending_seconds` (absent when nothing is pending), from the trigger policy's own state. What is left
  here is the CLI rendering, if `status` should print them.
- **The endpoint.** `GET /v1/preview?path=&depth=1|infinity&only=&measure_excluded=&top=` streams
  `application/x-ndjson`: a `header` at once, an `entry` for every path asked for, a `progress` record every 1000
  paths whatever is listed, then a `summary` — or an `error` in its place if the walk fails. `top` is clamped to
  1000. Refusals: `400 path_invalid` / `depth_invalid` / `preview_invalid`, `409 preview_busy` (one walk per daemon),
  `409 config_invalid` for a file that cannot be read, `500 preview_failed` / `registry_failed`.
- **What it touches** (decided): no History Store, no store lock, no Trigger. It holds the Registry mutex only
  while resolving the configuration, as every endpoint does, because resolving may write the known-good copy into
  a Root's store directory. Disconnect is seen on every line sent and on the progress records, so even a
  summary-only walk stops when its reader goes away. A reader that stays connected and stops reading releases the
  slot after 60 s.
- **Which directory, under which rules** (decided):
  - A Root is previewed under its own configuration.
  - A directory inside a Root is previewed under that Root's rules, as a capture of the Root classifies it: the
    walk starts there, the ignore files of its ancestors are applied, and the header says `root` and `within`.
  - Anything else is a candidate, configured exactly as registering would configure it: the global file, its own
    `.keepster.toml`, and — for a directory watched once — the identity registering takes back, with that identity's
    `[root.<id>]` block and what Forget excluded from it.
  - Other Roots are fenced (`withheld other_root`, never entered), and an overlap is named in the header.
  - With no argument, `keepster preview` walks up to the Root holding the working directory (ticket 07's rule,
    through `/v1/roots?path=`) and otherwise previews the working directory itself.
- **Showing the file** (decided): the header's `config` names the file and every key it sets, with the values it
  writes (a list's own entries only) and its warnings.
- **The walk** (`keepster_store::preview`): every verdict a capture uses, from a `stat` and a name only.
  - An excluded directory is pruned and named, unless `measure_excluded` is asked for, or `depth=infinity`, which
    implies it. Measuring never enters a fence or a loop.
  - A special file, including one reached through a followed link, is `withheld special_file`.
  - A directory that cannot be listed is a single `unreadable` entry.
  - The summary groups by the rule that decided, largest first, with the largest files on both sides. The
    "left out" side counts Withheld files as well, since a file past the size cap is usually the largest of all.
- **No growth estimate**: nothing on the wire carries one, and the CLI says so in words.
- **The claim**: `keepster_core::ROOT_CEILING` is 100,000 files, 10,000 directories and 10 GiB captured.
  - `keepster watch` previews quietly first. Past the claim it shows the summary and asks on a terminal.
  - Elsewhere it refuses with exit 5 until `--yes` is given.
  - A busy preview refuses `watch` with exit 5 rather than register unseen.
  - `watch --dry-run` renders the preview and registers nothing.
  - The preview's listing options (`--depth`, `--only`, `--measure-excluded`, `--top`, `--json`) stay on
    `keepster preview`.
- **`status`** prints `N change(s) pending, the oldest Ns ago` from ticket 43's fields; the queue is never listed.
- **Not built here:** ticket 20's trust prompt on first sight of a `.keepster.toml`. The security-relevant keys
  (secret opt-ins, `ui.editor`, store location) are shown like every other key, not singled out.
- **Known shape:** the preview walk is its own walker beside capture's `collect_files`, sharing every verdict
  (`CapturePolicy`, fences, `Place`) but not the traversal, because it emits rather than stages. The wire's `kind`,
  `class` and `reason` are open strings, as `Problem.code` is.
