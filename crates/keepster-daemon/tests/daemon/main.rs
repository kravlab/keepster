//! Every integration test of `keepster-daemon`, linked into one binary: each file is a module, as it was once
//! a crate of its own. One binary links once, where one per file linked dozens of times over — most of
//! what a test run cost on Windows.

mod alarms;
mod bundle;
mod cli_alarms;
mod cli_diagnose;
mod cli_diff;
mod cli_follow;
mod cli_history;
mod cli_lifecycle;
mod cli_logging;
mod cli_restore;
mod cli_roots;
mod cli_service;
mod cli_status;
mod cli_ui;
mod cli_uninstall;
mod configuration;
mod diagnostics;
mod diff;
mod events;
mod export;
mod forget;
mod housekeeping;
mod jobs;
mod lifecycle;
mod links;
mod listener;
mod logging;
mod modes;
mod openapi;
mod pins;
mod preferences;
mod preview;
mod reading;
mod reconciliation;
mod restore;
mod root_states;
mod roots;
mod rules;
mod search;
mod snapshots;
mod status;
mod triggers;
mod version;
mod volumes;
#[cfg(feature = "embed-ui")]
mod web_ui;
mod withheld;
