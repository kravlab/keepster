# 49: Symlinks — stored as links, optionally followed

**Blocked by:** 48 (Withheld, the metadata sidecar, special files and paths as bytes), 44 (Reconciliation — the
guarantee, and what counts as evidence of change)

**Status:** ready-for-agent

**What to build:** a symlink is captured as a link and restored as a link, a user can opt specific links into being
followed so a linked-in tree is captured, and Windows' several kinds of link surrogate each behave predictably.

Following is a pattern list, empty by default, layered like the Include Rules, and rules over followed content use
the **logical** path — so a denylist that matches names still refuses a private key reached through a link.
**Coverage of followed targets is periodic Reconciliation, not extra watches**, because watches are a shared
per-user resource and a mutable real-to-logical path mapping loses events silently.

- [ ] A symlink is stored with the link mode and its target text, is never followed by default, and is restored as a
      link
- [ ] The follow list opts specific links in; following stops at a target inside another Root or a History Store, or
      one that closes a cycle, which is stored as a link instead
- [ ] A followed target that *contains* another Root or a History Store is followed, with those subtrees Withheld
- [ ] A Root with a non-empty follow list reconciles its followed targets on their own interval, never below the rate
      limit, and reports that as its own fact in `status` rather than as "coverage not guaranteed"
- [ ] An unavailable target carries the subtree's previous content forward and marks the Snapshot incomplete, naming
      the link with its errno, so an unmount does not read as thousands of deletions
- [ ] On Windows a junction is stored with its marker and restored as a junction; a real symlink that cannot be
      created is refused with its own reason and never written as a plain file
- [ ] Volume mount points and links whose target cannot be read are Withheld as special files
- [ ] Registering a Root through a link asks whether the link or its real path is the Root, and refuses until told;
      a link-mode Root follows a retargeted link, with the overlap rule still enforced
- [ ] Restoring through a followed link writes through it only while the link is still matched and leads nowhere
      forbidden; otherwise the path is refused with its reason

## Comments

- 2026-10-11: ticket 47's Restore passes over a link in a Snapshot (`source()` in
  `keepster-store/src/history/restore.rs` returns `None` for mode `120000`), since none are captured yet; this
  ticket makes it restore one as a link, and raise `symlink_unsupported` where the platform cannot. The
  `link_check_failed` refusal there today refuses every link above a file, because no link is followed yet —
  `files.follow_symlinks` decides which may be.
