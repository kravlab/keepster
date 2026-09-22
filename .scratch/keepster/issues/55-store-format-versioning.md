# 55: Versioning the on-disk store format

**Blocked by:** 48 (Withheld, the metadata sidecar, special files and paths as bytes), 51 (Retention — the ladder,
the Segment Period and thinning)

**Status:** ready-for-agent

**What to build:** a user who upgrades keeps reading their old history, a user who downgrades is not punished, and
nobody's history is ever migrated behind their back.

**There is no migration: the content format is a property of the Segment** (ADR 0010). New Segments are written in
the new format, old ones stay readable, and the ladder retires them within about a month unaided. Four version
numbers govern disjoint things — the container in the Root's registry file (the only one rewritten automatically),
the content format in the Segment's root-commit trailer, the sidecar's record grammar in its header, and the index
schema, whose every mismatch is simply a rebuild.

- [ ] A store containing Segments of two content formats is read correctly, and new captures open a new Segment in
      the current format
- [ ] The container version is rewritten automatically — small, atomic and not history — while nothing rewrites a
      Segment
- [ ] A container version newer than the daemon understands makes the Root `Unavailable` with its own reason, and
      `Unavailable` covers "nothing can be captured" rather than only "directory unreachable"
- [ ] A Segment newer than the daemon understands leaves capture running in a new Segment of the daemon's own format
- [ ] An unknown sidecar record type is skipped **and reported**; redefining an existing type is forbidden, so an
      older daemon behaves exactly as it always did
- [ ] An index schema mismatch triggers a rebuild rather than an error
- [ ] The external `git` version floor deliberately does not travel with the store format
- [ ] `status` reports the store-format row beside the build and API rows
