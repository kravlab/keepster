# 59: Search across Snapshots — by path and by content

**Blocked by:** 39 (Read a version back — the derived index, entries, content and the timeline), 48 (Withheld, the
metadata sidecar, special files and paths as bytes)

**Status:** done

**What to build:** a user can find a file by a fragment of its name across all their Roots — including a file that no
longer exists in the Working Tree, which no other gesture in the product can reach — and can search the content of
every version in a Root when they remember the code but not the file.

Two operations, because every observable property differs. Path search is a query over the path-change rows that
already exist, inside the read budget, spanning every Root by default. Content search is a **streamed walk over
distinct blobs** — git deduplicates, so the work is proportional to stored content — attributed through a derived
blob-id column, one Root by default and all of them only on request. **No full-text index**: rebuilding one would
make the derived-cache promise false rather than merely slow.

- [x] Path search returns, per hit, the Root, the path, how many Snapshots touched it, the first and the last, and
      **whether the path still exists in the newest Snapshot**
- [x] Path search spans every Root by default and answers inside the chronology budget
- [x] Content search streams results, is cancellable, and is explicitly outside the read budget
- [x] A content hit is a **content version** with the Snapshot range it was current for, a line number and a
      snippet — never one row per Snapshot
- [x] Content search takes one Root by default and all Roots only when asked, showing the cost before starting
- [x] Binary blobs are skipped by content detection and counted; Withheld paths are reported as their own category
      with a reason; the metadata sidecar is always excluded; the stream ends in a summary of versions scanned,
      binaries skipped and Withheld paths met
- [x] Substring and literal matching are the defaults, a flag switches the path query to the Include Rules matcher
      and the content query to regular expressions, and matching is case-insensitive unless the query has capitals
- [x] Regular-expression matching is linear-time, so a query cannot hang the daemon
- [x] `keepster find` and `keepster grep` mirror the two operations

## Comments

- **Attribution through the index (schema 6).** `changes` gained a `blob` column: the content a path held after
  each change, derived from git like every other column, so deleting `index.sqlite` rebuilds it. A row with no
  content that is not a deletion is a Withheld candidate, resolved against that Snapshot's sidecar.
- **Path search** spans the watched Roots and the unwatched ones whose history is kept; it reads `changes` by its
  primary key (`DISTINCT path ORDER BY path`, then one keyed aggregate per hit) and answers JSON with `limit`
  (100, at most 1000; `0` or garbage is `limit_invalid`), `truncated`, and `unsearched` for a Root whose history
  could not be read. A Registry that cannot be read fails the request (`registry_failed`) rather than answering
  "nothing found". `--glob` reads the query as an Include Rule written at the Root: smart-case holds, and a
  directory pattern (`build/`) finds what is beneath it.
- **Content search** needs `roots` or `all=true` (`search_invalid` otherwise). The unit is the content version —
  one path holding one content over a run of Snapshots — read once per distinct content however many paths and
  Snapshots held it. The stream is `plan` (versions and bytes per Root) first, then `withheld` (each path once
  per reason, at the first Snapshot met), `match`, `progress` (every 100 versions and at each Root's end), and
  `summary` last; the CLI says a search that ended without its summary broke off. A closed or stalled reader
  ends the walk (`ndjson` module shared with preview, unit-tested in the daemon). The content identity on a
  match is named `content` and is opaque (ADR 0003).
- **Matching.** Lines are matched without their ending (`\n` or `\r\n`), so `$` works on CRLF files, and no
  phantom line follows the last ending. Smart-case ignores what follows a backslash in an expression (`\S`,
  `\pL`, `\p{Greek}`). Binary is a NUL in the first 8000 bytes. Regexes use the linear-time `regex` crate with
  a 1 MiB compiled-size limit.
- **No full-text index**, as the ticket says. Symbolic links are stored as blobs and their targets are searched
  as text; harmless, and left as is.
- **Exit codes.** `find` and `grep` exit 0 when nothing is found (the summary says what was looked at); a bad
  query is a usage error.
