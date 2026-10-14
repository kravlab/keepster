# 89: Capabilities outside the degenerate catalogue — a named volume, the machine's `git` and an unprivileged user

**Blocked by:** 70 (The report layer — the platform matrix, watcher conformance and coverage)

**Status:** needs-triage

**What to build:** a test that needs something a machine may lack — a temporary directory on a named volume, the
machine's own `git`, a user whom file permissions bind — either runs or is reported `unavailable` with its reason,
so a machine without it neither counts as coverage nor fails the run.

Ticket 31 asked that question of the whole suite: "a test that silently skips there is a claim of coverage that does
not hold", and a missing external `git` is `unavailable`, not a failure. Ticket 70 answers it for the degenerate
catalogue only: its three outcomes (`ran`, `impossible`, `unavailable`) and its generated table are the catalogue's.
Three places outside the catalogue still answer it the old way, in opposite directions.

**A silent skip.** `crates/keepster-service/tests/volume.rs`, `a_volume_that_is_named_is_found_where_it_is_mounted`,
returns early, passing, when the temporary directory's volume has no identity, so `mount_points` is never called and
the test is green all the same. Every platform can hit it: on Linux an overlay, a tmpfs or a disk with no
`/dev/disk/by-uuid` link (`volume/linux.rs`); on macOS `autofs`, `devfs` or a volume with no UUID
(`volume/macos.rs`); on Windows a mapped network drive, whose letter is no identity (`volume/windows.rs`). It does
not even print.

**A failure instead of `unavailable`.** The three store test helpers `fn git()` in
`crates/keepster-store/tests/store/forget.rs`, `pins.rs` and `maintenance.rs` `expect` a `git`, and so do the callers
of `keepster_testkit::environment::git()` in `crates/keepster-testkit/tests/environment.rs` and `tests/tripwire.rs`;
on a machine without one, every test behind them fails. Ticket 88 moves the same helpers onto one search for `git`;
whichever lands second keeps the other's change.

**A failure under a privileged user.** Eight tests make a file or directory unreadable with `chmod 000` and assert
that a Snapshot or a diff reports it: in `keepster-daemon`, `snapshots::a_snapshot_that_could_not_read_a_file_says_so`;
in `keepster-store`, `diff::a_working_file_that_cannot_be_read_is_named`,
`history::an_unreadable_file_does_not_cost_the_whole_snapshot`,
`history::the_first_snapshot_of_a_new_hour_is_still_based_on_the_previous_one`,
`pins::a_pin_carries_the_incomplete_marking_of_its_snapshot`,
`reading::an_incomplete_snapshot_says_so_in_the_timeline_after_a_rebuild_too`,
`withheld::a_directory_that_cannot_be_listed_names_its_errno_and_keeps_what_it_held` and
`withheld::a_read_error_names_its_errno_and_keeps_the_previous_version`. Run as root, which file permissions do not
bind, the file stays readable and all eight fail; a container that runs its tests as root is such a machine.

- [ ] Where the outcome of a capability probe outside the catalogue is recorded and shown, beside ticket 70's table
      or in it, is decided and recorded
- [ ] The volume test no longer passes without calling `mount_points`; a machine with no named volume for it reports
      `unavailable` with its reason, and an ordinary run stays green
- [ ] A test that needs the machine's `git` and finds none reports `unavailable` with its reason rather than failing
- [ ] A test that needs an unreadable file, run by a user whom file permissions do not bind, reports `unavailable`
      with its reason rather than failing
- [ ] No probe prints a runtime skip

## Comments

- Found while planning ticket 70.
- The privileged-user case found in ticket 70's final run of the whole suite, in a container that runs tests as
  root.
