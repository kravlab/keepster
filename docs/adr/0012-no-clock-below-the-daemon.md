# Nothing below the daemon reads a clock

Ticket 08 made time a parameter of every use-case, and ticket 06 put a 30-day ladder behind decisions whose
consequences are physical — a re-chained Segment, a `gc --prune=now` that reclaims, a pin that survives it — so the
tests that matter cannot be written over values and cannot wait a week. We therefore widen "time is a parameter"
into a workspace invariant: **every timestamp the store records derives from a `now` passed in from
`keepster-daemon`**, and `std::time::SystemTime::now` and `git2::Signature::now` are banned below the daemon
through `disallowed-methods` in `clippy.toml`. A month of history is then produced by the real capture path at
injected times, in seconds.

## Considered Options

- **Fabricating refs and backdated commits directly in test code.** Fast, and it needs no invariant. Rejected
  because the rig becomes a second implementation of ADR 0001 — leaf-only ref names, parentless Segment starts, the
  `SnapshotId` trailer, normalised modes — so Retention would be proved against a store no daemon ever writes, and
  the two implementations drift in the direction that keeps the test green. This is ADR 0004's objection to state
  that disagrees with reality silently, applied to fixtures.
- **Compressing wall-clock**: a 1 s Segment period and tiers in seconds, with real time passing. Rejected because
  the suite then sleeps, the boundary invariant is never exercised at the 24 h / 7 d / 30 d durations it is written
  in, and the run's duration becomes the thing the test measures.
- **A `Clock` trait.** Rejected already by ticket 08, and nothing here reopens it: a parameter covers every use of
  time that remains once scheduling is an `Action::ArmTimer`.

## Consequences

- **A lint, not a convention, carries it.** One `Signature::now()` in `keepster-store` leaves every ladder test
  passing while proving nothing, and the failure is invisible in review — which is why enforcement is mechanical.
- **`keepster-daemon` is the only clock reader in the workspace**, alongside being the only place that wires
  dependencies. A future crate that needs the time takes it as an argument or does not get it.
- **The history builder in `keepster-testkit` is the sole aged-store mechanism**, shared by Retention, disk-cap and
  pin tests, so those tests exercise the write path rather than a description of it.
- **Timestamps become an explicit part of every store-facing signature**, which is more verbose than calling the
  clock in place. That verbosity is the point: a signature without a time cannot silently acquire one.
