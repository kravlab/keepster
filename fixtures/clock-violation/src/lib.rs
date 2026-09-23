//! Reads the wall clock outside `keepster-daemon`, which ADR 0012 forbids.
//!
//! Compiled only by the test that asserts clippy refuses it, with the project's `clippy.toml` supplied
//! through `CLIPPY_CONF_DIR`.
#![deny(clippy::disallowed_methods)]

/// The violation under test: a moment read from the clock rather than taken as a parameter.
pub fn when() -> std::time::SystemTime {
    std::time::SystemTime::now()
}
