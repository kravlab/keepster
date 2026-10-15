# 88: One search for the machine's `git`

**Blocked by:** 69 (The performance harness — fixtures, the tripwire and the first baseline)

**Status:** migrated

**Migrated to:** #25

**What to build:** the daemon and every test that runs `git` find it by one function, so the places `git` is
looked for cannot drift apart between the product and the tests that stand in for it.

The search is written out four times. The daemon's `locate` (`keepster-daemon/src/git.rs`) builds the platform's
candidates — `keepster_service::git_for_windows()` on Windows, `keepster_core::git_candidates` with the home
directory and `xcode-select -p` elsewhere — and names macOS's `/usr/bin/git` stub. Three test helpers in
`keepster-store` (`tests/store/forget.rs`, `maintenance.rs` and `pins.rs`, each `fn git()`, each documented as "found
as the daemon finds it") and `keepster_testkit::environment::git()` repeat the candidates without Xcode and without
the stub, so on macOS they already look in fewer places than the daemon does.

The daemon's helpers `platform()` and `xcode()` are private to it, which is why the tests copied the composition
rather than calling it; ticket 69 kept the daemon's `locate` private rather than widening its API for a bench.

- [ ] One public function, in a crate the daemon, `keepster-store`'s tests and `keepster-testkit` can all depend on,
      returns the platform's candidates and the stub, with Xcode's place on macOS; `keepster-service`, which holds
      the Windows half, does not depend on `keepster-core`, which holds the rest, so where it lives is decided here
- [ ] The daemon's `locate` calls it; its behaviour, including the configured path and the stub, is unchanged
- [ ] The three `keepster-store` test helpers and `keepster_testkit::environment::git()` call it; no copy of the
      candidate list is left
- [ ] Tests on the shared function pin the candidates each platform yields, run on every platform CI runs

## Comments

- Found while finishing ticket 69.
