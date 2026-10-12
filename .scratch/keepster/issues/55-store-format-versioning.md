# 55: Versioning the on-disk store format

**Blocked by:** 48 (Withheld, the metadata sidecar, special files and paths as bytes), 51 (Retention — the ladder,
the Segment Period and thinning)

**Status:** done

**What to build:** a user who upgrades keeps reading their old history, a user who downgrades is not punished, and
nobody's history is ever migrated behind their back.

**There is no migration: the content format is a property of the Segment** (ADR 0010). New Segments are written in
the new format, old ones stay readable, and the ladder retires them within about a month unaided. Four version
numbers govern disjoint things — the container in the Root's registry file (the only one rewritten automatically),
the content format in the Segment's root-commit trailer, the sidecar's record grammar in its header, and the index
schema, whose every mismatch is simply a rebuild.

- [x] A store containing Segments of two content formats is read correctly, and new captures open a new Segment in
      the current format
- [x] The container version is rewritten automatically — small, atomic and not history — while nothing rewrites a
      Segment
- [x] A container version newer than the daemon understands makes the Root `Unavailable` with its own reason, and
      `Unavailable` covers "nothing can be captured" rather than only "directory unreachable"
- [x] A Segment newer than the daemon understands leaves capture running in a new Segment of the daemon's own format
- [x] An unknown sidecar record type is skipped **and reported**; redefining an existing type is forbidden, so an
      older daemon behaves exactly as it always did
- [x] An index schema mismatch triggers a rebuild rather than an error
- [x] The external `git` version floor deliberately does not travel with the store format
- [x] `status` reports the store-format row beside the build and API rows

## Comments

- **The content format** (decided): `Keepster-Format: <n>` on every commit this build writes, `ContentFormat::CURRENT`
  = 1, a Segment with no trailer at all being the first format. Every commit rather than only the root, so a
  Segment Retention thinned of its root and a Pin standing alone still say it; since no build appends across
  formats, a commit that names its format speaks for its whole Segment, so a reader asks the tip and walks to the
  root only for a Segment from before the trailer. A format from the first to this build's own is read; any other
  — newer, or not a number — is unreadable.
- **A capture beside another format** (decided): a capture never appends to a Segment of another format. The hour's
  Segment is used while it is in this build's format (or absent); otherwise the capture opens the hour's
  **sibling**, `<period>.<format>` (`2026-09-30T14.1`), which `SegmentName::parse` accepts, sorts inside its
  period, stays a leaf, and is found again without counting — one Segment per format per period. The baseline is
  the newest Snapshot this build reads in the greatest period at or before the capture's, across siblings, so a
  newer build's tree is never misread as the previous state. Recorded in ADR 0010 and `CONTEXT.md` (Content Format).
- **Unreadable Segments and Pins** are never read and never rewritten: the index does not list their Snapshots and
  records them instead (schema 5, table `unreadable`; any schema mismatch is a rebuild, as ever), the timeline
  answers them as `unreadable` and `keepster log` warns, `GET …/pins` lists unreadable Pins the same way and
  `keepster pins` warns, Retention leaves them for the build that wrote them, and Forget names them in `left_alone`
  — what they hold was not forgotten — while still counting their contents as held.
- **The container** (decided): `Registry::upgrade` rewrites every record (`root.toml` and `unwatched.toml`) of an
  older container version atomically, once, at the daemon's start before anything is served — on every read it
  could race an unwatch's rename and bring the record back. Reading writes nothing. A newer container stays the
  existing `unavailable` / `store_too_new`, which `CONTEXT.md` already defines as "cannot be captured at all".
- **Sidecar records**: an unknown type was already skipped and named by `entries` (`unrecognized`) and the log; a
  pass's count of them now reaches `status` as the Root's `unrecognized_records`, printed under the Root. Every
  record spelling written today is pinned by a test, so a redefinition fails it.
- **Status**: `/v1/version` and `/v1/status` carry `store_format { container, content, sidecar, index }`, absent
  from an older daemon rather than zero; `keepster status` prints `store format container 1, content 1, sidecar 1,
  index 5` beside the build and API rows, or `unknown`.
- **The git floor**: a harness test points `git.path` at a git reporting 2.31.8 — `maintenance: unavailable`,
  reason `too_old` — and history reads as ever, the store-format row unchanged.
- **Not done**: no harness test plants a newer-format Segment in a daemon's store (the daemon's tests do not name
  git); the store tests cover it with real git.

