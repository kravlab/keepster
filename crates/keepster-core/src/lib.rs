//! Pure decision kernel: domain types, policy, Retention, Include Rules. No traits, no I/O, no clock.
//!
//! Deliberately empty: ticket 36 creates the crate so the workspace has ticket 08's shape from the
//! first commit. Cargo enforces a rule as soon as the crates involved have content — a crate cannot
//! use what it does not depend on — and the one rule that already matters, that `keepster-cli` links
//! no store, is checked by the merge gate.
