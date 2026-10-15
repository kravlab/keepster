# 77: A Root registered through a link

**Blocked by:** 49 (Symlinks — stored as links, optionally followed)

**Status:** migrated

**Migrated to:** #16

**What to build:** registering a directory reached through a symlink asks whether the link or its real path is the
Root, and a Root registered as the link follows it when it is retargeted, without escaping the overlap rule.

Split out of ticket 49. Ticket 10's "A Root registered through a link" section is the design.

- [ ] `POST /v1/roots` with a path through a link refuses with `confirmation_required` until `path_mode: "link" |
      "canonical"` is given; `keepster watch` asks on a TTY and otherwise takes `--keep-link` / `--canonical`
- [ ] A canonical Root ignores later changes to the link, and the CLI canonicalises the cwd when walking up to find a
      Root
- [ ] A link-mode Root follows a retargeted link silently; a new target containing another Root or a History Store
      has those subtrees Withheld, and a new target inside another Root makes the Root `Unavailable` with
      `root_overlaps` until the overlap ends
