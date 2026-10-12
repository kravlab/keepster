# 58: Export a version into a directory of the user's choosing

**Blocked by:** 47 (Restore — whole-request preflight, confirmations and the Safety Snapshot), 48 (Withheld, the
metadata sidecar, special files and paths as bytes)

**Status:** done

**What to build:** a user can put an old version, or a whole Snapshot, into a directory they choose — so they can
look at an old state beside the current one without overwriting anything — and can also download one file from the
browser.

Export is a daemon-side write to a destination the client names: the browser may name one because it always runs on
the daemon's machine, which keeps the endpoint unprivileged. A streamed archive endpoint survives for the download
gesture and for handing someone one file, with its size shown before the click, and it can never resume.

- [x] Exporting writes the chosen paths into the given destination, preserving the Snapshot's permission bits
- [x] A synchronous preflight reports totals and supports a dry run; the write itself is a job on the existing job
      resource
- [x] The preflight reuses Restore's rules minus the three that are Restore-only, and adds an explicit overwrite
      confirmation, off by default
- [x] A destination inside a Root is allowed only behind its own confirmation and takes a Safety Snapshot when it
      overwrites; a destination inside a History Store is refused
- [x] A Withheld path is absent from the output and reported as skipped with its reason, never as a zero-byte file
- [x] A directory-picking browse endpoint lists directories only, so a web client can choose a destination
- [x] The streamed archive endpoint serves the browser download gesture, states its size beforehand, and handles
      large files
- [x] `keepster export` mirrors it, and a diff written to a file is unaffected by the shell's own encoding

## Comments

- 2026-10-11: ticket 46 built `keepster diff -o FILE`, which writes the daemon's patch bytes itself (through a
  `.partial` file renamed into place), so the "a diff written to a file is unaffected by the shell's own encoding"
  criterion holds for the CLI already. What remains here is the browser's side: `Content-Disposition: attachment`
  on `GET /v1/roots/{r}/diff?format=unified`, which ticket 46 did not set.
- **The export** (`POST /v1/roots/{id}/export`): `{ from, paths?, dest, dry_run?, confirm?, resolutions?,
  skip_unresolvable?, modes? }`. The plan is made at once: a dry run answers `200` with every file's outcome and
  the selection's `files` and `bytes`; a blocked plan answers `400 confirmation_required` with `paths`; anything
  else is `202` and an `export` job on the job resource, which plans again under the stores' locks, then writes.
  Its result carries `dest`, every path's outcome and the `safety_snapshot`, if one was taken. No event of its
  own: `job.*` covers it, as ticket 19 decided.
- **The rules** (`keepster_core::export_plan`): Restore's preflight without `hardlinked`, without
  `overwrite_unrecoverable` outside a Root, and without the Safety Snapshot outside a Root.
  - A missing directory is made without asking (decided): making one destroys nothing, and an Export into a new
    directory makes every one, so `create_parents` would only ever be noise here.
  - `overwrite` is a new confirmation, off by default, and so is `dest_in_root`, asked of every path that writes
    when the destination is inside a Root. `--force` / `"all"` covers both.
  - Overwriting inside a Root takes that Root's Safety Snapshot first, under its store's lock (both stores are
    locked in identity order). Its Trigger is the new `export` (decided: a Safety Snapshot that names its cause;
    the trigger set is open on the wire).
  - A path that would land inside *another* Root beneath the destination is refused (`other_root`, no
    confirmation): that Root's Safety Snapshot is not this Export's to take. Export into that Root itself.
  - A destination inside a History Store is refused (`dest_in_store`); one where a file stands is
    `dest_unwritable`. `dest` is made absolute-only and rid of `..` by name before it is compared.
  - A Withheld path, asked for by name or beneath a directory, is skipped with its reason; nothing is written for
    it. Exported files carry the Snapshot's permission bits (none on Windows); a file already there with other
    bits is the `modes` question.
- **Downloads**: `GET …/snapshots/{s}/stat?path=` gives files, bytes, the exact archive size and the Withheld
  paths; `GET …/snapshots/{s}/archive?path=` streams a zip64 archive, every entry stored, so `Content-Length`
  is that size and a file past 4 GiB needs nothing special. Modes travel in the external attributes; it is dated
  at the Snapshot's moment, so it is the same bytes every time — and still not resumable. Checked against
  Python's `zipfile` and Info-ZIP `unzip`. `content?download=1` and `diff?format=unified` carry
  `Content-Disposition: attachment`.
- **`GET /v1/fs?path=`**: directories only (a link is not one), hidden ones marked (a dot name, or the Windows
  hidden attribute), each said to be listable; the home directory when `path` is absent.
- **CLI**: `keepster export [PATH...] --to DIR | --archive FILE`, `--from` (default `latest`), `--dry-run`,
  `--overwrite`, `--replace-type`, `--dest-in-root`, `--overwrite-unrecoverable`, `--force`, `--detach`,
  `--json`, and Restore's `--pick`, `--rename`, `--name-case`, `--modes`, `--skip-unresolvable`, `--no-input`,
  now one flag group shared by both commands, as is the asking. Exit codes as ticket 07's table: 4, 5, 6.
- **Not built / known**: writing a Snapshot's link on Windows needs the symlink privilege (ticket 76), so such a
  file fails there. A file already in the destination that is read-only on Windows cannot be replaced and fails
  with its error. `skip_unresolvable` passes over unconfirmed `overwrite` and `dest_in_root` too, as it does
  Restore's confirmations; each such path is reported skipped with its reason.
