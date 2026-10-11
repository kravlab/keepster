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
    let root = {
        let registry = state.registry();
        roots::record(&registry, id)?
    };

    let resolved = state.configuration(id, &root.path);
    let Some(effective) = resolved.effective else {
        return Err(config_problem(&resolved.errors));
    };

    Ok(Json(RootConfiguration {
        root: id.to_string(),
        keys: effective
            .keys
            .iter()
            .map(|key| {
                let (value, origin, entries) = match &key.setting {
                    Setting::Scalar(entry) => (wire(&entry.value), origin(&entry.origin), None),
                    Setting::List(entries) => (
                        entries.iter().map(|entry| wire(&entry.value)).collect(),
                        entries
                            .last()
                            .map(|entry| origin(&entry.origin))
                            .unwrap_or_else(default_origin),
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
                    origin,
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
        // Reading files blocks, and the runtime's threads are for requests.
        let _ = tokio::task::spawn_blocking(move || reload(&state)).await;
    }
}

fn reload(state: &DaemonState) {
    let records = match state.registry().readable() {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(%error, "the Registry could not be read to reload configuration");
            return;
        }
    };

    for record in &records {
        state.configuration(record.id, &record.path);
    }
    state.forget_configuration_except(&records);
}

/// A value as the wire types it.
fn wire(value: &Value) -> serde_json::Value {
    match value {
        Value::Bool(switch) => json!(switch),
        Value::Bytes(bytes) => json!(bytes),
        Value::Pattern(pattern) => json!(pattern),
        Value::SizePattern { pattern, max } => json!({ "pattern": pattern, "max": max }),
    }
}

/// Where an empty list's value comes from: the built-in default, which is no list at all.
fn default_origin() -> keepster_api::Origin {
    keepster_api::Origin {
        level: keepster_api::ConfigLevel::Default,
        source: "the built-in default".to_owned(),
        line: None,
    }
}
