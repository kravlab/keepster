//! Every integration test of `keepster-store`, linked into one binary: each file is a module, as it was once
//! a crate of its own. One binary links once, where one per file linked dozens of times over — most of
//! what a test run cost on Windows.

mod diff;
mod export;
mod forget;
mod formats;
mod held;
mod history;
mod links;
mod maintenance;
mod modes;
#[cfg(any(target_os = "linux", windows))]
mod names;
mod pins;
mod preferences;
mod preview;
mod reading;
mod reconciliation;
mod registry;
mod retention;
mod search;
mod topology;
mod withheld;
