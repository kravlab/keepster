# 87: Strict object validation left off on an uncached index

**Blocked by:** 69 (The performance harness — fixtures, the tripwire and the first baseline)

**Status:** needs-triage

**What to build:** the invariant ticket 12 measured and left "to implementation" enters the code, and the tripwire
line measured for it starts defending it.

Ticket 12 measured strict object validation as the dominant cost of writing a tree from an uncached index: 100k
entries took 10.19 s with it on, libgit2's default, against 0.83 s with it off, one `git_odb_read_header` per entry.
Nothing in the repository turns it off: `git2::opts::strict_object_creation` is called nowhere. In libgit2 1.9.7,
as libgit2-sys 0.18.8 vendors it, the check starts on (`object.c:22`), and `write_tree_to` checks every entry it
writes against the object database (`tree.c:500`, through `git_object__is_valid` at `object.c:580`). A first
capture, and any capture whose index has no valid TREE cache, pays that price today.

The switch is not the write path's alone: it is one global for the whole process. The same check guards a commit's
tree and parents (`commit.c:96-101`) and a reference's target (`refs.c:405`), so turning it off drops those checks
too. That is a product decision about the write path, which is why ticket 69 records the line as a baseline instead
of making the decision itself.

- [ ] The decision is recorded: validation turned off for the daemon's process, or kept on with the cost accepted,
      with what each choice gives up for commits and references
- [ ] If it is turned off, it is turned off once, before any store is opened, and a test shows a capture into a
      fresh store still yields a tree whose every entry exists in that store
- [ ] `capture.first_uncached` becomes a gated line (`Small::Gated`) with a committed threshold, its baseline reason
      removed; or, if validation stays on, the reason says so and names this ticket's decision
- [ ] A mutation that turns validation back on breaches the gated line, checked as ticket 69 checked the others

## Comments

- Found while planning ticket 69.
