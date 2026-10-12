//! Every integration test of `keepster-core`, linked into one binary: each file is a module, as it was once
//! a crate of its own. One binary links once, where one per file linked dozens of times over — most of
//! what a test run cost on Windows.

mod archive;
mod export;
mod follow;
mod git;
mod job;
mod paths;
mod policy;
mod preview;
mod restore;
mod retention;
mod roots;
mod rules;
mod segments;
mod sidecar;
mod snapshots;
mod triggers;
mod volumes;
