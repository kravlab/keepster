# 34 — Search across Snapshots

Type: grilling
Status: resolved

## Question

The read side is what this tool exists for, and the one read gesture never designed is search. Ticket 15 settled
the history browser as one screen with three view modes — a Snapshot rail, a file-first tree-and-scrubber, an
activity feed — all of which assume the user already knows which path they want.

Settle:

- **Whether v1 has search at all**, or whether "find the file, then scrub" is the v1 answer and search is
  post-release like ticket 18's mount and ticket 22's TUI.
- **By path, by content, or both.** These are different products: a path search is a query over ticket 03's derived
  index; a content search over ~1,600 Snapshots is a full-text problem, and git's own `grep` across commits is a
  linear walk.
- **The collision with ticket 03's invariant.** The SQLite index is declared a *derived cache that may be deleted
  and rebuilt from git objects at any time*. A content index is large and slow to rebuild; either it obeys that
  invariant and pays a long rebuild, or it is a second store with its own lifecycle, which the invariant exists to
  prevent.
- **The collision with ticket 30's read budget**, which claims ≤ 100 ms to fetch a version and ≤ 100 ms for a
  path's chronology. A search has no such bound unless it is indexed, and Capture Preview is the only precedent for
  a deliberately expensive read — it is streamed and bounded by throughput.
- **Where it lives in the UI** (ticket 15's one screen) and **whether the CLI and `/v1` get it too**, since ticket
  07 made the API public and the SPA unprivileged: a search endpoint is part of the contract or it does not exist.
- **Scope**: within one Root or across all of them.

## Answer

**Both halves ship in v1, as two separate operations, and neither builds a full-text index.** The two are
different products, as the ticket says, and the decision keeps them different all the way down: a path query is a
budgeted lookup in an index that already exists, a content query is a streamed walk that promises no total time.

### Path search is not a convenience — it is the missing gesture

Ticket 15's three view modes all navigate a tree, and that tree comes either from a Snapshot or from the Root. **A
file that no longer exists in the Working Tree therefore has no gesture at all** — while a deleted file is the
product's central scenario. Path search closes that, and it is nearly free: ticket 03's path-change index exists
precisely to answer "in which Snapshots did this path change", so a query over it is a `GLOB` on rows already
written, inside ticket 30's ≤ 100 ms chronology budget. This is why the path result row must state **whether the
path still exists in the newest Snapshot**: without it, "found" is indistinguishable from "found, and already
gone", and the second is the whole value.

### Content search is a walk over distinct blobs, never an index

Counted the obvious way — grep each of ~1,600 Snapshots — the operation is absurd: `git grep` per commit re-reads
trees and rescans content that did not change. Counted correctly, the unit of work is the **distinct blob**, which
git already deduplicates, so the cost is proportional to *stored content*, bounded by ticket 30's 10 GiB per-Root
ceiling: tens of seconds, not milliseconds. Attribution comes from a join: the path-change rows gain a **blob OID
column**, which is derived from the same tree-diff walk that already builds them and therefore stays wholly inside
ticket 03's rebuildable-cache invariant.

**A full-text index is rejected for v1, and the reason is that invariant rather than effort.** Ticket 03 promises
that the SQLite cache may be deleted and rebuilt from git objects at any time; rebuilding an FTS index means
re-reading and tokenising every blob in the store, which would not make the promise expensive — it would make it
false. Adding one later is additive and needs no new decision, which is why this is recorded here rather than as an
ADR.

Its cost model is ticket 20's, and so is its shape: **streamed NDJSON, not a job** — ticket 20 made `forget` a job
because a dropped connection strands an irreversible operation, and a search is the inverse — taking no lock,
counting as no Trigger, and aborting when the client disconnects. It is **explicitly outside ticket 30's read
budget**, which is stated rather than implied, exactly as Preview's unbounded input is.

### Scope: paths everywhere, content in one Root unless asked

The store and its index are per-Root, so "across all Roots" means two different things. **Path search spans every
Root by default** — fanning a bounded index query across the handful of Roots ticket 30 sizes for, and answering
the common loss, "I deleted it and cannot remember which project". **Content search takes one Root by default and
all of them only when explicitly asked**, because its cost is proportional to stored content, so a fan-out
multiplies an already unbudgeted walk; when asked, the cost is shown before it starts and the stream reports
per-Root progress. `/v1` being additive-only (ticket 07) means the default could widen later without a `/v2`.

### The contract: two operations, because every observable property differs

- `GET /v1/search/paths?q=&roots=` — synchronous JSON, inside the budget. Row: RootId, path, the number of
  Snapshots touching it, the first and the last, and whether it exists in the newest Snapshot.
- `GET /v1/search/content?q=&roots=&path=` — NDJSON stream, no budget, abortable.
- CLI mirrors them as `keepster find <glob>` and `keepster grep <pattern>`.

A single `/v1/search?mode=path|content` was rejected on ticket 14's mechanics: the document is generated from the
DTO types, so a response type that depends on a query parameter can only be expressed as a union — the client is
left guessing, and the `oasdiff` additivity gate then argues with every change to either half. CLI-only was
rejected by ticket 07: the SPA is unprivileged, so a gesture without an endpoint cannot exist in the UI.

### Query syntax

Substring and literal by default, patterns behind a flag, smart-case throughout. `--glob` switches the path query
to **the Include Rules matcher** of ticket 04 and ADR 0007 — one pattern language in the product, not two —
and `--regex` switches the content query to the `regex` crate. The crate choice carries a security property worth
recording: the endpoint is public by ticket 07 and reachable from a browser, and `regex` guarantees linear-time
matching, so a search cannot become a way to hang the daemon with one string. Pattern-only was rejected because the
commonest query (`config.toml`) would need escaping; substring-only because `**/*.env` is already how the product
talks about paths.

### The result unit is a content version, not a Snapshot

This **corrects the row shape stated earlier in this ticket's own reasoning**: with blob-level deduplication, a
file that matched and then went unchanged for 1,400 Snapshots would emit 1,400 identical rows — and 1,440 Snapshots
a day is exactly what the default rate limit permits. A content row is therefore **(RootId, path, blob OID, the
Snapshot range over which that content was current, line number, snippet)**. The range is also what makes a result
actionable in ticket 15's UI: it resolves to a Snapshot to open, where a list of identical rows leads nowhere.

### Nothing is skipped silently

The product already refuses silent omission — `incomplete` names what it could not read, `Withheld` carries a
reason, ADR 0009 rejected the zero-byte blob as "the lie that propagates furthest". A search that returns nothing
must therefore be distinguishable from a search that did not look:

- **binary blobs** are skipped by NUL detection (ripgrep's rule) and **counted**;
- a **`Withheld`** path has no blob at all (ADR 0009 gives it no tree entry), so it is reported as its own category
  with its reason;
- **`.keepster-meta`** is always excluded — it is our metadata, not the user's content;
- the stream's **final line is a summary**: versions scanned, binaries skipped, `Withheld` paths encountered. Both
  counts are produced by the walk anyway.

### In the UI it is an entrance, not a fourth mode

A field in the History screen's header, with results as an overlay. The test ticket 15 set decides it: a view mode
must answer what it shows, and search's answer after a result is picked is always one of the three existing modes —
so it is not a mode but a way of **arriving at the (file, Snapshot) pair** those modes already carry between them.
Picking a result closes the overlay and hands that pair to the current mode; a result in another Root switches the
Root in the header visibly. The path/content toggle lives in the field, and a content search shows progress and a
cancel control, because it is streamed and outside the budget. A separate screen was rejected for the reason
ticket 15 already gave when it rejected three screens.

Refines tickets 03, 07, 15, 20 and 30.
