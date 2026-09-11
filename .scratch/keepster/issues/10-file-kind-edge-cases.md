# 10 — Binary files, symlinks, permissions and path edge cases

Type: grilling
Status: resolved
Blocked by: 01

## Question

Snapshotting an arbitrary directory means meeting files that a source tree usually does not contain. What does
v1 do with each?

Settle:

- large files: a size cap, and what the user sees when a file is skipped for exceeding it;
- binary files: stored as-is, or skipped by default;
- symlinks: stored as links, followed, or ignored — and what restore does with them;
- file modes: which permission bits survive a snapshot and a restore, given Windows has no equivalent. Taken as
  given: the History Store sets `core.filemode = false` (ADR 0001), so no mode bit reaches history through git's
  own filesystem probing. What is left to settle is whether the executable bit is captured out of band — in the
  index we build, or as Snapshot metadata — so that a Restore can put it back, or whether v1 accepts losing it and
  says so;
- case-insensitive filesystems, and paths that are valid on one platform but not another;
- files that vanish between the event and the snapshot, and files that are being written while read;
- restore of a file that no longer exists in the working tree, and restore into a directory that was deleted.

## Answer

### Withheld: a policy omission, distinct from incomplete

A Snapshot may deliberately not hold a path's content, and that is recorded rather than hidden: the path is listed
in the Snapshot as **Withheld** with a reason code, appears in `entries`, and does not make the Snapshot incomplete.
`incomplete` stays reserved for files that were meant to be captured and could not be read, so it remains an alarm
instead of becoming the permanent state of any Root that holds one large file. Treating a size cap as an Include
Rule was rejected because the file would vanish exactly where the user looks for it; folding it into `incomplete`
was rejected because the flag would stop meaning anything. `CONTEXT.md` gains the term.

Reasons: `too_large`, `special_file` (kind `fifo`, `socket`, `char_device`, `block_device`, `mount_point`,
`wsl_symlink`), `reserved_name`, `unrepresentable_path`, `other_root`, `history_store`, `cloud_placeholder`. A
Restore of a Withheld path is refused with `withheld`.

### Size and binary files

- **Binary files are stored as-is**; binariness is a display property only (the `entries` flag of ticket 07, the UI
  card of ticket 09). Detection is a heuristic — NUL in the first 8,000 bytes, which UTF-16 text also has — and the
  insured files of a `~/Documents` Root (`.docx`, `.xlsx`, `.psd`) are binary.
- **A size cap is on by default**: `files.max_size = "100MiB"`, a scalar, plus `files.max_size_by_pattern =
  [{ pattern, max }]` layered like Include Rules (last match wins, negation, visible through `explain?path=`).
  Over-cap files are Withheld `too_large`. The cap bounds daemon memory as well as disk: libgit2 has no streaming
  read for packed objects (`odb_pack.c:902-914`, `pack.c:899-900`), so serving `content` with `Range` or a diff
  inflates the whole blob, and everything is packed after `gc`; writing already streams (`blob_path`). An exemption
  list without a value was rejected because an exempt file would have no memory bound at all.

### Symlinks

- **Default: stored as a link** — tree mode `120000`, blob = target text, never followed. Include Rules apply to
  the link's own path.
- **`files.follow_symlinks`** is a pattern list, empty by default, `["**"]` meaning the whole Root, layered like
  Include Rules. There is no "follow only inside the Root" mode, because a target inside the Root is captured at its
  own path anyway, and no "withhold" mode, because Include Rules already exclude a link.
- **Where following stops.** A target inside another Root or a History Store, or one closing a cycle (an ancestor of
  the link or the Root, or a `(dev, ino)` already on the walk), is stored as a link instead. A target that
  *contains* another Root or a History Store is followed, with those subtrees Withheld `other_root` /
  `history_store`.
- **Rules use the logical path.** Content under a followed link is judged by its Root-relative path, for Include
  Rules and the secret denylist alike; the denylist matches names, so `keys -> ~/.ssh` still refuses
  `keys/id_rsa`.
- **Coverage is Reconciliation, not extra watches.** Arming a watch on every target's real path was rejected: it
  spends the shared per-user watch budget (research 02, F-L9), lets unrelated churn in the target overflow the shared
  inotify queue and force Reconciliation of every Root (F-L1), and makes the real-to-logical path mapping mutable
  state whose errors lose events silently. A Root with a non-empty `follow_symlinks` instead runs a periodic
  Reconciliation of its followed targets only, every `files.follow_reconcile_interval` (default 15 min, never below
  the rate limit); the rest of the Root stays on the watcher. `status` reports it as its own per-Root fact,
  `follow_targets { count, last_reconciled_at, interval }`, not as "coverage not guaranteed", which keeps its ticket
  04 meaning.
- **An unavailable target** (unmounted, deleted, unreadable) carries the subtree's previous content forward and
  marks the Snapshot incomplete, naming the link path with its errno, so an unmount never reads as thousands of
  deletions followed by thousands of creations.
- **Restore through a followed link** writes through it — the temp file in the target's own directory — only if the
  link is still matched by `follow_symlinks` and does not lead into another Root or a History Store; otherwise the
  path is refused with `link_check_failed`. Ticket 07's confinement rule for `open` is aligned to the same
  definition: logical paths plus the follow rules.

#### Windows links

- A link whose symlink cannot be created (no Developer Mode, no `SeCreateSymbolicLinkPrivilege`) is refused with
  `symlink_unsupported` and never silently written as a plain file, which the next Snapshot would record as a type
  change; the UI explains how to enable Developer Mode. The flavour comes from the Snapshot (the target is a directory
  in it), then from the target on disk, else file.
- A junction — reported by Rust std as a symlink, like every name-surrogate reparse point (`sys/fs/windows.rs:
  1187-1193`) — is stored as a link with a `junction` marker in the Snapshot's metadata, obeys the same follow rules,
  and is restored as a junction, which needs no privilege. Volume mount points and WSL symlinks, whose target
  `read_link` cannot read (`:728-754`), are Withheld `special_file` (`mount_point`, `wsl_symlink`).

#### A Root registered through a link

`keepster watch <link>` asks whether to register the link or its canonical real path. `POST /v1/roots` refuses with
`confirmation_required` until `path_mode: "link" | "canonical"` is given; the CLI asks on a TTY and otherwise takes
`--keep-link` / `--canonical`. A canonical Root ignores later changes to the link, and the CLI canonicalises the cwd
when walking up to find a Root. A link-mode Root follows a retargeted link silently — the user chose exactly that at
registration — but ticket 07's overlap rule still holds: a new target containing another Root or a History Store has
those subtrees Withheld, and a new target inside another Root makes the Root `Unavailable` with `root_overlaps`
until the overlap ends.

### Modes and permissions

- **The executable bit is the tree entry mode** (`100755` / `100644`). On Unix it comes from `stat`; on Windows the
  disk is never probed and the mode is carried from the same path's entry in the previous Snapshot, new files getting
  `100644`, as git does under `core.filemode = false`. A Restore sets or clears `x` on Unix and ignores the mode on
  Windows. ADR 0001's "normalised mode" is clarified to mean exactly this.
- **A Restore over an existing file keeps its permissions.** Temp + rename creates a new inode, which on its own
  would turn a `0600` file into `0644`; so the temp file takes the replaced file's permission bits, then the
  Snapshot's `x` is applied (set for the classes that have `r`, or cleared everywhere). A newly created file starts
  from the umask. Ownership, ACLs and extended attributes are not carried.
- **Accepted loss.** Other permission bits and permission-only changes do not reach history in v1, so a Restore
  cannot undo an accidental `chmod 644` on a private key. Whether that should change is
  [17 — Full permission capture and restore](17-full-permission-capture.md).

### Hardlinks, special files, `.git`, cloud files

- **Hardlinks.** Temp + rename breaks a hardlink on every platform (rename(2): "Any other hard links to the file …
  are unaffected"). A Restore of a file whose link count exceeds one is therefore refused with `hardlinked` unless
  confirmed with `break_hardlinks`. The refusal lists the other links inside the Root, found by `(dev, ino)` at
  refusal time, and the number of links outside it (`nlink` minus those found); on Windows it lists every name from
  `FindFirstFileNameW`, for which Linux and macOS have no equivalent. The link count and file identity on Windows are
  not in stable Rust std (`windows_by_handle`, rust-lang/rust#63010), so both calls go through a direct,
  Windows-only `windows-sys` dependency with feature `Win32_Storage_FileSystem`, at the version tokio already pulls
  in (`tokio-1.50.0/Cargo.toml:102-106`), with `unsafe` confined to one module. Grouping links by `(dev, ino)` so
  that a write through one link refreshes the others (research 02, R8) is change tracking and is not decided here.
- **Special files.** Files are opened `O_RDONLY | O_NOFOLLOW | O_NONBLOCK` and read only if `fstat` reports a
  regular file. FIFOs, sockets and devices are Withheld `special_file` and never read — a FIFO without a writer
  would block the capture forever (fifo(7)).
- **`.git` and its aliases** (`.GIT`, `GIT~1`, `.git.`, `.git `) cannot be stored under their names at all:
  libgit2's owner-less index rejects them on every platform (`index.c:974-986`, `path.c:263-287`). A built-in,
  non-overridable rule records such a component once as Withheld `reserved_name` and does not descend into it.
  Storing under an escaped name was rejected: every git command would churn `index` and `objects` into history, and
  restoring them is exactly the interference with a directory's own `.git` that the map rules out of scope.
- **Cloud placeholders are never hydrated.** The daemon exposes placeholders on Windows and turns materialisation
  off on macOS (`setiopolicy_np`, `IOPOL_MATERIALIZE_DATALESS_FILES_OFF`). A placeholder whose size and mtime match
  its last capture carries that version forward, because dehydration does not change content; otherwise — changed
  remotely, or never local — it is Withheld `cloud_placeholder`. Reading placeholders would download the whole cloud
  on the first Reconciliation, fill the disk and undo "free up space".

### Paths and reads

- **Names are bytes.** Tree names are stored as raw bytes. `path=` carries percent-encoded bytes; JSON carries
  `path` as UTF-8 — lossy, with U+FFFD, for a name that is not UTF-8 — plus `path_bytes` (base64url) whenever the name
  is not UTF-8. This is the representation ticket 07 left to this ticket, used by every endpoint. A Windows name with
  an unpaired surrogate is Withheld `unrepresentable_path`: git2's path APIs reject it (`util.rs:179-186`) and Rust's
  WTF-8 bytes are documented as unfit for storage (`ffi/os_str.rs:148-160`).
- **Files that change while read.** `stat` is compared before and after the read (size, mtime and ctime with
  nanoseconds, inode or file id), with up to three retries. A file that never settles carries its previous version
  forward and marks the Snapshot incomplete with `unstable`; one with no previous version is absent and named. A read
  error, such as a sharing or lock violation on Windows, marks it incomplete with its errno. A file already gone when
  opened is simply deleted. No torn version ever reaches history.

### Restore: preflight, confirmations, resolutions

This refines ticket 07's Restore. It still overwrites a file of the same type and creates a file in an existing
directory without asking, but nothing structural happens without confirmation, and nothing is written until the
whole request has been checked.

- **Preflight.** Every path is checked before anything is written. If any path needs a confirmation or a
  resolution, or cannot be restored, nothing is written and the response is `confirmation_required` listing every
  path with its reason. `dry_run: true` returns that per-path plan without writing or taking a Safety Snapshot. The
  resubmitted request takes one Safety Snapshot, preserving ticket 07's one operation, one undo. State that changes
  between preflight and write still yields a per-path `failed`.
- **Yes/no confirmations** — `confirm: [...]`, or `"all"` (CLI `--force`):
  - `parent_missing` → `create_parents`: missing directories are created from the umask;
  - `type_change` → `replace_type`: a leaf is replaced by another kind of leaf (file ↔ symlink, file → directory);
  - `hardlinked` → `break_hardlinks`;
  - `overwrite_unrecoverable` → `overwrite_unrecoverable`: the current version cannot enter the Safety Snapshot
    (over the cap, since excluded, unreadable), so overwriting it is irreversible.
- **Choices** — `resolutions: { "<path>": … }`, keyed by Snapshot path in the representation above. `force` never
  resolves these:
  - `name_collision`: two Snapshot paths are one file on the destination, by case or Unicode normalisation →
    `{ "write": true }` on the chosen path, the others `skipped`;
  - `name_invalid`: the name cannot be created on the destination (`con.txt`, a trailing dot, `:`) →
    `{ "rename_to": "<name>" }`, a bare name in the same directory that is valid on the destination and collides
    with no existing or restored path, re-checked by preflight;
  - `name_case`: Snapshot `foo.txt` meets `Foo.txt`, the same file → `{ "name_case": "keep_existing" |
    "use_snapshot" }`, asked only when the content differs; identical content is `unchanged`.
- **Never restorable**: `type_conflict` (replacing a *directory*, whose contents may lie outside what a Safety
  Snapshot can hold — ticket 07's argument against "make identical"), `withheld`, `link_check_failed`,
  `symlink_unsupported`.
- **`skip_unresolvable: true`** lets the request proceed, reporting every unresolved or unrestorable path as
  `skipped` with its reason; without it, any such path stops the whole request.
- **CLI**: `--create-parents`, `--replace-type`, `--break-hardlinks`, `--overwrite-unrecoverable`, `--force`,
  `--pick <path>`, `--rename <path>=<name>`, `--name-case <path>=keep|snapshot`, `--skip-unresolvable`,
  `--dry-run`. On a TTY it asks interactively; `--no-input` disables prompts even there, and missing resolutions then
  exit with code 5 (ticket 07) listing the reasons. The UI refuses with a plain explanation and asks.
