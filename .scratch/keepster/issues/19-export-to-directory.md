# 19 — Export: saving a version into a directory of the user's choosing

Type: grilling
Status: resolved

## Question

v1 currently writes history back over the working file only: Restore, as settled by tickets 07 and 10. The user
also wants to put a historical version *somewhere else* — this file as it was, a whole subtree as it was, an
entire Snapshot, or a diff — into a directory they name, leaving the Working Tree untouched.

This is the same read as Restore with a different destination, so the decision is mostly about which of Restore's
hard-won rules survive the change of destination, and which are Restore-only.

Settle:

- **What can be exported**: a single path's version, a subtree, a whole Snapshot, a diff between two Snapshots —
  and in what shape (plain files laid out under the destination, or one archive).
- **Which of Restore's preflight rules apply.** Ticket 10 gave Restore a whole-request preflight with `dry_run`,
  per-reason confirmations plus `force`, per-path resolutions for name collisions and unrepresentable names,
  `skip_unresolvable`, and a flat refusal for hardlinked files. Writing into an empty directory outside any Root
  changes the stakes: nothing is being overwritten, so the hardlink refusal and the collision resolutions may be
  unnecessary — but a destination that is *not* empty brings them straight back.
- **Whether export may write into a watched Root.** Exporting into a Root feeds the export back into history and
  can collide with a live capture; refusing outright is simple but would block the obvious "put the old version
  next to the new one" workflow.
- **What a `Withheld` entry exports as** (ticket 10): omitted, a zero-byte placeholder, or a refusal for the whole
  request — and whether that differs from what Restore does.
- **Diff export format**: a unified text diff, the pair of files, or both; and what a diff means for a binary or
  `Withheld` path.
- **The three surfaces.** The `/v1` contract is additive-only (ticket 07), so this is new endpoints, not changed
  ones. The CLI takes a destination argument. The web UI cannot write to a directory from a browser, so its export
  is a download — a genuinely different mechanism from a daemon-side copy, and the contract has to serve both
  without growing two semantics.
- **Who performs the copy**: the daemon can read the store but may be unable to write to the user's chosen
  directory (a sandboxed or differently-confined install), while having the CLI write it means streaming bytes over
  the API. This also decides whether a huge export is a job with progress (like `forget` and Maintenance in
  ticket 07) or a synchronous request.
- **Permissions of exported files**, and whether the executable bit travels (ticket 10 keeps it in the tree mode);
  ticket 17 may widen this.

## Answer

### The daemon performs the copy, into a path

Export is `POST /v1/roots/{r}/export` with a `dest` the daemon writes to itself, symmetrical with Restore. Having
the daemon stream bytes for the client to unpack was considered and rejected: it would make the CLI carry an
unpacker and its own collision handling, and the browser — the surface that cannot receive a `dest` at all — would
still need an archive, so the contract would grow two semantics instead of one.

The objection that a client-side unpack needs a system archiver does not hold literally (a Rust CLI unpacks
in-process), but it holds for the browser, where unpacking is the user's problem, and it holds for size: a
generated-on-the-fly archive is not byte-reproducible and not seekable, so `Range` cannot resume it. A 100 GB
download that dies in its fifth hour restarts from zero, and costs 2× the disk besides. A daemon-side write has
none of that: 1× disk, resumable by re-running, and by the job rule below it survives the tab that started it.

**The daemon and the browser are always on the same machine** (ticket 05: one daemon per OS user, loopback
listener), which is what makes a `dest` meaningful to a web client at all. The endpoint is therefore not privileged
in ticket 07's sense: a third-party UI calls exactly the same one.

### Preflight synchronous, the write a job

The request answers either `400 confirmation_required` or `202` + `job_id`; `dry_run: true` is always synchronous
and returns the per-path plan plus the selection's total size and file count.

Preflight is bounded and cheap — it reads a tree already in the store and `stat`s the destination — so it is not
what a background job is for, and its result is a dialogue with the client, not the outcome of work. The write is
the opposite: a whole Snapshot is the longest-running request in the tool, and ticket 07 already owns the
machinery — `job.started` / `job.progress` / `job.completed` over SSE, the CLI waiting and rendering progress by
default, `--detach` printing the id. No `export.completed` event is added; the generic `job.*` set covers it,
unlike `restore.completed`, which reports a synchronous operation.

Two response shapes on one endpoint is not an anomaly: Restore already has `confirmation_required` against `200`.

### Which of Restore's rules survive

Ticket 10's preflight applies **minus the three rules that exist because Restore overwrites the Working Tree**:
`hardlinked`, `overwrite_unrecoverable` (except as below) and the Safety Snapshot. What remains:

- the whole request is checked before anything is written, and any path needing a decision stops all of it;
- `dry_run`, the per-path outcome, and `skip_unresolvable`;
- the resolutions `name_collision` and `name_invalid`, which are **not** inherited by inertia: the destination may
  be a different filesystem from the Root — a Linux Root exported onto exFAT or into a macOS directory produces
  exactly ticket 10's `Foo.txt`/`foo.txt` and `con.txt` — and that vocabulary is already designed;
- **overwriting is off by default**, gated by a new yes/no confirmation `overwrite` (CLI `--overwrite`, covered by
  `--force`). Outside a Root the destination has no history and no Safety Snapshot, so an overwrite there is
  irreversible; the tool's other irreversible operation, `forget`, already requires an explicit `confirm: true`
  (ticket 06), and silently overwriting would be the one that does not.

### A destination inside a Watched Root

Permitted, behind the confirmation `dest_in_root`; a destination inside a **History Store** is refused outright
(`dest_in_store`) — it is not in any Working Tree, no Include Rule looks at it, and writing there corrupts a store
whose integrity nothing verifies.

Refusing a destination inside a Root was rejected because it kills the motivating workflow ("put the old version
next to the new one" is `--to ./old/` almost every time). The usual fears do not survive inspection: the daemon
already writes into Roots during Restore, writes are atomic (temp + rename), a capture meeting the maintenance lock
coalesces rather than blocks (ADR 0004), and an exported file genuinely exists in the Working Tree, so capturing it
is honest — `explain?path=` says whether Include Rules take it.

The confirmation exists for a case the user does not mean: exporting a **whole Snapshot** into its own Root
duplicates the Root inside itself, doubling the next Snapshot, and the per-Root disk cap of ticket 06 answers that
by pausing capture. The confirmation is asked whenever the destination is inside a Root, not only for large
exports: a "one file passes silently" threshold would need a number, and nothing justifies one.

**Overwriting an existing path inside a Root takes a Safety Snapshot of that Root** and returns its `SnapshotId`,
exactly as Restore does. Inside a Root the overwritten file may hold edits made since the last Snapshot — up to the
rate limit of ticket 04 — which is precisely what Restore's Safety Snapshot protects, and without this Export would
be a second, less safe way to destroy a working file. Forbidding the overwrite instead was rejected because it cuts
the wrong case: what is usually overwritten inside a Root is the leftovers of a previous export into the same
`./old/`, i.e. the repeat-export workflow the `overwrite` confirmation exists to keep. Where the destination path
lies inside a Root but is excluded by Include Rules, the Safety Snapshot cannot hold it — that is ticket 10's
existing `overwrite_unrecoverable`, not a new rule.

### Scope: one shape, mirroring Restore; a diff is not part of it

The body is `{ from, paths: [...], dest }` — a directory means everything beneath it, omitting `paths` means the
whole Snapshot — because every decision above (whole-request preflight, one Safety Snapshot per operation, per-path
outcome) is stated over a list of paths, and ticket 07 already rejected one-path-per-call for Restore for the same
reason.

**A diff is exportable, but not through this endpoint**: `GET /v1/roots/{r}/diff?from=&to=&path=&format=unified`
already returns the finished unified text. Adding a diff mode here would create a second way to produce the same
bytes and would force answers to what a diff means for a binary or a `Withheld` path — questions that do not arise
otherwise. The "pair of files" shape is two ordinary exports with different `from`.

Saving it does need work on the clients, and one piece is not optional: **`keepster diff` gains `-o <file>`** and
writes the bytes itself, because in Windows PowerShell 5.1 — the shell present by default — a native command's
output is decoded and rewritten as UTF-16LE with CRLF, which no `git apply` or `patch` will accept. `>` stays a
convenience for Unix shells and PowerShell 7. In the browser the same endpoint carries
`Content-Disposition: attachment`, which works with the `HttpOnly` cookie of ticket 07 and needs no new auth path.

### The three surfaces

New or extended endpoints, all additive to `/v1` and all public:

- `POST /v1/roots/{r}/export` — `{ from, paths?, dest, confirm?, resolutions?, skip_unresolvable?, dry_run? }`.
- `GET /v1/roots/{r}/snapshots/{s}/archive?path=` — a streamed **zip64** archive of the selection, for clients that
  cannot receive a `dest`. No resume, by construction.
- `GET /v1/roots/{r}/snapshots/{s}/content?path=&download=1` — the existing endpoint plus `Content-Disposition`,
  which is the whole of single-file download.
- `GET /v1/roots/{r}/snapshots/{s}/stat?path=` — total bytes and file count for a selection, so a size can be shown
  before an action is taken. Git trees store no aggregate, so it is a tree walk in the store or a read of ticket
  03's index.
- `GET /v1/fs?path=` — directory listing on the daemon's machine, so a web client can choose a `dest`.

**The web UI offers both actions, with no threshold**, showing the selection's size next to them: "Save to
directory…" (daemon export, job, 1× disk, survives the tab) and "Download .zip" (small selections, the Downloads
gesture, and the only way to hand someone a single file). A size threshold was rejected for the same reason as
above — the number would be arbitrary — and showing the size is what makes the choice informed rather than a trap.

A browser directory picker cannot serve the daemon path: `<input webkitdirectory>` yields relative names only, and
`showDirectoryPicker()` yields a JS handle rather than a path — usable only if the page itself writes every byte,
and implemented only in Chromium. Hence `GET /v1/fs?path=`: the picker is drawn by the UI and fed by the daemon.

`GET /v1/fs?path=` is **not restricted to a subtree**. Restricting it protects nothing: a client holding the cookie
can already `POST /v1/roots` with `/` and read the whole disk through `entries`, so a home-directory limit would
only tax the honest cases (an external volume, `/srv`). It returns directories only — no files, no sizes — does not
resolve symlinks, marks hidden directories so the UI can offer a "show hidden" toggle (off by default), and reports
an unreadable directory as a per-entry outcome rather than failing the whole listing.

**CLI**: `keepster export [<path>...] --from <id> --to <dir>`, a command of its own, with ticket 10's flags
(`--dry-run`, `--skip-unresolvable`, `--pick`, `--rename`, `--no-input`, `--force`) plus `--overwrite`,
`--archive <file>` and `--detach`. The selectors `latest`, `~N`, `2h` work in `--from` for free, being client-side
(ticket 07). A `restore --to <dir>` flag was rejected: after the rules above the two operations differ in almost
everything (Safety Snapshot, `hardlinked` and `withheld` on one side; `dest_in_root` and `overwrite` on the other),
so one flag would switch a whole regime — the "mode" ticket 07 rejected when it discarded "make identical" — and
`restore --to /tmp/old` is far too easy to type believing it is still a restore.

### Withheld paths

A `Withheld` path is **not created and is reported**: it is absent from the destination, appears in the per-path
outcome as `skipped` with its reason, and is listed by `dry_run` in advance. The same holds inside the archive.

A zero-byte placeholder was rejected as the most dangerous option: it is indistinguishable from a genuinely empty
file, and nothing stops the user copying it back over the real one — the tool that exists to prevent accidental
overwrites would have caused one. Refusing the request, as Restore does, was rejected because `Withheld` is policy,
not failure (`CONTEXT.md` draws that line against `incomplete`): a Root with one over-cap video would fail every
whole-Snapshot export by default, training the user to pass `skip_unresolvable` always — which also silences the
genuinely unresolvable `name_invalid` and `name_collision`.

### Permissions

**Exported files carry the Snapshot's modes**, now available from ticket 17's `.keepster-meta`. No key and no
question: Restore asks because two legitimate current states collide — the file's mode on disk and the Snapshot's —
and a new file in an empty destination has no second state. Where export overwrites an existing file, that second
state exists again and ticket 17's `modes` resolution applies unchanged.

Falling back to the umask was rejected concretely: a `0600` file — which a user may have opted into history past the
secret denylist — would land in `/tmp` as `0644`, and a tool that refuses secrets by default must not downgrade
their permissions on the way out; a tree of scripts that lost `+x` is broken output besides. On Windows no mode is
read or applied (ticket 17). In the zip, modes travel in the external attributes field, which `unzip` honours on
Unix and Explorer ignores.

`.keepster-meta` itself never appears in an export, exactly as it never appears in `entries`, diffs or Restore.
Empty directories do not appear either, because git does not hold them at all — see the map's *Not yet specified*.

### Errors and exit codes

New `code` values, additive under ADR 0005's open set: `dest_in_root` (a confirmation), `dest_in_store` (a flat
refusal), `dest_unwritable`. No new exit code: ticket 07's table already covers this command — `4` no such
Snapshot, `5` unresolved resolutions under `--no-input`, `6` some paths failed, `0` success.

### What this refines

- **Ticket 10**: its preflight is reused for a destination outside the Working Tree, less the three Restore-only
  rules; `Withheld` becomes a reported skip here, while remaining a refusal in Restore.
- **Ticket 17**: answers the question it deferred — an exported file carries the Snapshot's modes.
- **Ticket 07**: `/v1` gains the endpoints above, `keepster diff` gains `-o`, and the CLI gains `export`.
- **Ticket 23**: the `Withheld` reason must reach the client per path, not only `entries`.
