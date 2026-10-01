//! Use-cases over the concrete snapshot store, synchronous.
//!
//! A use-case takes the time it needs as a parameter and returns values; health, scheduling and
//! transport belong to the daemon above it (ticket 08). Nothing here decides how anything is rendered.

pub mod roots;
pub mod snapshots;
