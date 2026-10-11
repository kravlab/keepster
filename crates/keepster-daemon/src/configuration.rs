//! A Root's effective configuration, and the reload that keeps it current (ticket 42).

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use keepster_api::{ConfigEntry, ConfigKey, Problem, RootConfiguration, RootState};
use keepster_app::config::{Resolved, Setting, Value};
use serde_json::json;

use crate::DaemonState;
use crate::explain::origin;
use crate::problem::{ProblemResponse, config_problem, invalid_file};
use crate::roots::{self, RootAddress};

/// How often every file is compared with what was read last, when no request asks sooner.
///
/// A second is what "edit a file and see the effect" means to a person, and reading a few small files
/// that often costs nothing worth measuring.
const RELOAD_INTERVAL: Duration = Duration::from_secs(1);

/// One Root's effective configuration, with the level that set each key.
#[utoipa::path(
    get,
    path = "/v1/roots/{id}/config",
    operation_id = "get_root_config",
    tag = "configuration",
    params(RootAddress),
    responses(
        (status = 200, description = "Every key with its value and provenance. While a file is invalid, the last known-good values are in force and `errors` names the file.", body = RootConfiguration),
        (status = 404, description = "No such Root is registered.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The Root is unavailable, or its configuration has never had a valid version, so it is not captured at all.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn show(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
) -> Result<Json<RootConfiguration>, ProblemResponse> {
    let id = roots::parse_id(&id)?;
    // Resolved while the Registry is held, as everywhere: resolving may write into the Root's store
    // directory, which an erasing deregistration must not be removing at the same moment.
    let resolved = {
        let registry = state.registry();
        let root = roots::record(&registry, id)?;
        state.configuration(id, &root.path)
    };

    let Some(effective) = resolved.effective else {
        return Err(config_problem(&resolved.errors));
    };

    Ok(Json(RootConfiguration {
        root: id.to_string(),
        keys: effective
            .keys
            .iter()
            .map(|key| {
                let (value, entries) = match &key.setting {
                    Setting::Scalar(entry) => (wire(&entry.value), None),
                    Setting::List(entries) => (
                        entries.iter().map(|entry| wire(&entry.value)).collect(),
                        Some(
                            entries
                                .iter()
                                .map(|entry| ConfigEntry {
                                    value: wire(&entry.value),
                                    origin: origin(&entry.origin),
                                })
                                .collect(),
                        ),
                    ),
                };

                ConfigKey {
                    key: key.spec.name.to_owned(),
                    value,
                    origin: origin(&key.setting.origin()),
                    entries,
                    restart_required: key.restart_required,
                }
            })
            .collect(),
        warnings: effective
            .warnings
            .into_iter()
            .map(crate::explain::warning)
            .collect(),
        errors: resolved
            .errors
            .iter()
            .map(|error| invalid_file(error).into_problem())
            .collect(),
    }))
}

/// What a Root's configuration does to its state, if anything: `Unavailable` when it has no version to
/// apply, `Degraded` while a file is invalid and the last known-good stands in for it.
pub(crate) fn state_of(resolved: &Resolved) -> Option<(RootState, &'static str)> {
    if resolved.effective.is_none() {
        Some((RootState::Unavailable, "config_invalid"))
    } else if !resolved.errors.is_empty() {
        Some((RootState::Degraded, "config_invalid"))
    } else {
        None
    }
}

/// Resolve every Root's configuration once a second, for as long as the daemon serves.
pub(crate) async fn reload_continually(state: DaemonState) {
    let mut ticks = tokio::time::interval(RELOAD_INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticks.tick().await;
        let state = state.clone();
        // A pass reads every Root's files, which is blocking work with no request waiting on it.
        let _ = tokio::task::spawn_blocking(move || reload(&state)).await;
    }
}

/// One pass over every registered Root.
///
/// The Registry is held for the whole pass, as every handler holds it while it reports a Root: a
/// known-good copy is written into the Root's store directory, which an erasing deregistration must
/// not be removing at the same moment. The order is always the Registry, then the configuration.
pub(crate) fn reload(state: &DaemonState) {
    let registry = state.registry();
    reload_with(state, &registry);
}

/// The reload pass over a Registry the caller already holds — which a handler that has just changed it
/// does, so that a Root registered or moved is watched at once rather than at the next tick.
pub(crate) fn reload_with(state: &DaemonState, registry: &keepster_store::Registry) {
    let records = match registry.readable() {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(%error, "the Registry could not be read to reload configuration");
            return;
        }
    };

    let resolved = records
        .iter()
        .map(|record| {
            let effective = state.configuration(record.id, &record.path).effective;
            (record.clone(), effective)
        })
        .collect();
    state.forget_configuration_except(&records);
    state.runtimes.sync(state, resolved);

    // A reload that changed what classifies a Root's paths owes it a full comparison: a file the old
    // rules excluded may now belong in history, and no event will ever say so (ADR 0009).
    for id in state.take_reconcile_queue() {
        state.runtimes.reconcile(id);
    }

    crate::events::announce_changes(state, registry);
}

/// A value as the wire types it.
fn wire(value: &Value) -> serde_json::Value {
    match value {
        Value::Bool(switch) => json!(switch),
        Value::Bytes(bytes) => json!(bytes),
        Value::Duration(duration) => json!(keepster_core::format_duration(*duration)),
        Value::Words(words) => json!(words),
        Value::Name(name) => json!(name),
        Value::Pattern(pattern) => json!(pattern),
        Value::SizePattern { pattern, max } => json!({ "pattern": pattern, "max": max }),
        Value::Ladder(ladder) => json!(
            ladder
                .tiers()
                .iter()
                .map(|tier| json!({
                    "age": keepster_core::format_duration(tier.age),
                    "keep": tier.keep.name(),
                }))
                .collect::<Vec<_>>()
        ),
    }
}
