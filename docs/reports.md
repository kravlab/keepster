# Reports

Three reports describe what the test suite covers on each platform (ticket 70). None of them gates anything: an
ordinary run stays green whatever they say, and only the release checklist below asks something of one. Each is
written to `target/reports/`, never committed, because each names the machine it was made on.

| Report | Answers | Locally | File |
| --- | --- | --- | --- |
| Platform table | what each degenerate case came to on each platform | `make platform-table` | `platform-table.md`, from `platform-<os>.json` |
| Watcher conformance | what this platform's watcher delivers for each documented class of change | `make conformance` | `conformance-<os>.md` |
| Coverage | which crate no test reaches at all | `make coverage` | `coverage-<os>.md`, from `coverage.json` |

`.github/workflows/reports.yml` makes all three on Linux, macOS and Windows, when run by hand or for a `v*` tag.
Each platform's reports are the artifact `reports-<OS>`, and the table of every platform is `platform-table`.

## The platform table

One row per case of the degenerate catalogue (`crates/keepster-testkit/src/degenerate.rs`), one column per
platform. A case comes to exactly one of three outcomes on a platform, and the cell says which, with the reason
for either of the last two:

- **ran**: the case was made and every check on it passed;
- **impossible**: the platform cannot hold the case at all, decided when the code was built (`#[cfg]`), so it is
  never a skip;
- **unavailable**: the platform can hold the case but this machine could not make it, because a capability probe
  failed, such as a filesystem that refuses a name.

A fourth word appears where no run said anything: **untested**. A platform whose column is missing, because the
catalogue's test was not run there or failed, is untested in every row; a failed test writes no column.

The column is written by the catalogue's own test, `every_case_makes_exactly_the_entries_it_declares`, when
`KEEPSTER_REPORT_DIR` names an absolute directory; unset, it writes nothing. A relative path fails the test,
since a test runs in its own crate's directory. A second run on the same platform replaces its column.
`keepster-report table <dir>` assembles the table from the columns in `<dir>`.

No test anywhere prints that it skipped something or carries `#[ignore]`: `tests/printed_skips.rs` in
`keepster-testkit` fails on either and names the file and line. What a test cannot do on a machine is an
outcome in this table, or a test that fails.

## Watcher conformance

For each documented class of change (an edit in place, an atomic save, a move into or out of the Root, a write
through a second hard link, a permission-only change, and the rest in
`crates/keepster-testkit/src/conformance.rs`), the events the operating system's watcher delivered, as `notify`
reports them before `keepster-watch` filters anything: their kind, their paths relative to the watched root,
and whether one asked for a rescan. A class nothing was delivered for is a finding, not a failure: research 02
documents several no platform reports. Writing through `mmap` is not among the classes, since making one needs
`unsafe`, which the workspace denies.

Each class is acted out in a fresh directory, watched once it is prepared; the observation ends after 2 s with
nothing delivered, or at 10 s.

## Coverage

`make coverage` runs every test under `cargo llvm-cov` and sums, per crate, the lines of its own source
(`crates/<name>/src/`) that any test reaches. There is no threshold: the report answers which crate no test
reaches at all, listing a crate no test binary links as one with nothing measured. The report is made from
whatever the tests reached even when some failed, and `make coverage` then fails as they did. It needs the
`llvm-tools-preview` component and `cargo-llvm-cov` 0.9.1:

```sh
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --version 0.9.1 --locked
```

## At release

- [ ] `reports.yml` has run on the release commit on all three platforms
- [ ] `keepster-report table <dir> --release`, over the `platform-table` artifact, exits 0: no case possible on
      the reference platform (Linux) is `unavailable` or `untested` on any platform. It prints each gap otherwise
      and exits 1
- [ ] The watcher conformance and coverage reports read, and any crate no test reaches either given a test or a
      ticket
