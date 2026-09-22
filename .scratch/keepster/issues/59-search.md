# 59: Search across Snapshots — by path and by content

**Blocked by:** 39 (Read a version back — the derived index, entries, content and the timeline), 48 (Withheld, the
metadata sidecar, special files and paths as bytes)

**Status:** ready-for-agent

**What to build:** a user can find a file by a fragment of its name across all their Roots — including a file that no
longer exists in the Working Tree, which no other gesture in the product can reach — and can search the content of
every version in a Root when they remember the code but not the file.

Two operations, because every observable property differs. Path search is a query over the path-change rows that
already exist, inside the read budget, spanning every Root by default. Content search is a **streamed walk over
distinct blobs** — git deduplicates, so the work is proportional to stored content — attributed through a derived
blob-id column, one Root by default and all of them only on request. **No full-text index**: rebuilding one would
make the derived-cache promise false rather than merely slow.

- [ ] Path search returns, per hit, the Root, the path, how many Snapshots touched it, the first and the last, and
      **whether the path still exists in the newest Snapshot**
- [ ] Path search spans every Root by default and answers inside the chronology budget
- [ ] Content search streams results, is cancellable, and is explicitly outside the read budget
- [ ] A content hit is a **content version** with the Snapshot range it was current for, a line number and a
      snippet — never one row per Snapshot
- [ ] Content search takes one Root by default and all Roots only when asked, showing the cost before starting
- [ ] Binary blobs are skipped by content detection and counted; Withheld paths are reported as their own category
      with a reason; the metadata sidecar is always excluded; the stream ends in a summary of versions scanned,
      binaries skipped and Withheld paths met
- [ ] Substring and literal matching are the defaults, a flag switches the path query to the Include Rules matcher
      and the content query to regular expressions, and matching is case-insensitive unless the query has capitals
- [ ] Regular-expression matching is linear-time, so a query cannot hang the daemon
- [ ] `keepster find` and `keepster grep` mirror the two operations
