//! The Watched Root registry endpoints: registering, listing, resolving, moving, deregistering and
//! erasing a history.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Json, response::IntoResponse};
use keepster_api::{
    ErrorCode, Event, FollowTargets, Problem, RegisterRoot, Root, RootState, UnwatchedRoot,
};
use keepster_app::roots::{self, RegisterError, RootError};
use keepster_core::{RootId, VolumeVerdict};
use keepster_store::{RegistryEntry, RegistryError, RootRecord};

use crate::DaemonState;
use crate::configuration;
use crate::problem::ProblemResponse;
use crate::triggers;

/// The `{id}` segment every Root endpoint is addressed by: the extractor and its description at once.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct RootAddress {
    /// The Root's identity, in its canonical form. A prefix or a path names no Root here: resolving
    /// those is the client's convenience, done before the request.
    pub(crate) id: String,
}

/// Which Roots to return: all of them, or the one holding a path.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ListQuery {
    /// Resolve this path to the Root that holds it. Absolute, because the daemon's own working
    /// directory means nothing to a client.
    path: Option<String>,
}

/// Every registered Root, in registration order — or, with `path`, the one that holds it.
///
/// Filtering rather than a second endpoint, and an empty answer rather than a refusal: ticket 07 makes
/// `?path=` a filter over a collection wherever it appears, so a client renders one list with one code
/// path and "no Root here" is a value rather than an error.
#[utoipa::path(
    get,
    path = "/v1/roots",
    operation_id = "list_roots",
    tag = "roots",
    params(ListQuery),
    responses(
        (status = 200, description = "Every registered Root, or the one holding `path`; empty when none does.", body = Vec<Root>),
        (status = 400, description = "`path` is not absolute.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The Registry could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn list(
    State(state): State<DaemonState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Root>>, ProblemResponse> {
    let registry = state.registry();

    match &query.path {
        Some(path) => {
            let path = absolute_query_path(path)?;
            let found = roots::resolve(&registry, &path).map_err(registry_problem)?;

            Ok(Json(
                found
                    .iter()
                    .map(|record| describe_record(&state, record))
                    .collect(),
            ))
        }
        None => Ok(Json(reported(&state, &registry)?)),
    }
}

/// Every Root no longer watched whose history is kept (ticket 68): the identities that reach those
/// histories, which `GET /v1/roots` does not list.
#[utoipa::path(
    get,
    path = "/v1/unwatched-roots",
    operation_id = "list_unwatched_roots",
    tag = "roots",
    responses(
        (status = 200, description = "Every Root no longer watched whose history is kept; empty when there is none.", body = Vec<UnwatchedRoot>),
        (status = 500, description = "The Registry could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn list_unwatched(
    State(state): State<DaemonState>,
) -> Result<Json<Vec<UnwatchedRoot>>, ProblemResponse> {
    let held = roots::unwatched(&state.registry()).map_err(registry_problem)?;

    Ok(Json(
        held.into_iter()
            .map(|held| UnwatchedRoot {
                id: held.id.to_string(),
                path: held
                    .record
                    .ok()
                    .map(|record| record.path.to_string_lossy().into_owned()),
            })
            .collect(),
    ))
}

/// One Root, addressed by its identity.
#[utoipa::path(
    get,
    path = "/v1/roots/{id}",
    operation_id = "get_root",
    tag = "roots",
    params(RootAddress),
    responses(
        (status = 200, description = "The Root.", body = Root),
        (status = 404, description = "No such Root is registered.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The Registry could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn show(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
) -> Result<Json<Root>, ProblemResponse> {
    let id = parse_id(&id)?;
    let registry = state.registry();

    reported(&state, &registry)?
        .into_iter()
        .find(|root| root.id == id.to_string())
        .map(Json)
        .ok_or_else(|| not_found(&id.to_string()))
}

/// Register a directory as a Watched Root.
#[utoipa::path(
    post,
    path = "/v1/roots",
    operation_id = "register_root",
    tag = "roots",
    request_body = RegisterRoot,
    responses(
        (status = 201, description = "The directory is now a Watched Root.", body = Root),
        (status = 400, description = "The path is not a directory, or cannot be resolved; or the body is not JSON.", body = Problem, content_type = "application/problem+json"),
        (status = 415, description = "The body is not declared as `application/json`.", body = Problem, content_type = "application/problem+json"),
        (status = 422, description = "The body is JSON of the wrong shape.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The directory overlaps a Root that is already registered.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The Registry could not be written.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn register(
    State(state): State<DaemonState>,
    body: Result<Json<RegisterRoot>, JsonRejection>,
) -> Result<impl IntoResponse, ProblemResponse> {
    let Json(request) = body.map_err(body_problem)?;
    let registry = state.registry();
    let record = roots::register(&registry, now(), request.path.as_ref(), &|path| {
        crate::volumes::place_of(&*state.volumes, path)
    })
    .map_err(register_problem)?;
    // Announced before the reload, so a state the new Root reaches is announced after it exists.
    state.publish(Event::RootRegistered {
        root: record.id.to_string(),
    });
    configuration::reload_with(&state, &registry);

    Ok((StatusCode::CREATED, Json(describe_record(&state, &record))))
}

/// Whether a deregistration also erases the History Store.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct DeleteQuery {
    /// Erase the History Store as well. Off unless asked for.
    #[serde(default)]
    purge: bool,
}

/// Deregister a Root, and with `purge=true` erase its History Store.
#[utoipa::path(
    delete,
    path = "/v1/roots/{id}",
    operation_id = "unwatch_root",
    tag = "roots",
    params(RootAddress, DeleteQuery),
    responses(
        (status = 204, description = "The Root is no longer watched; its history is kept, its last state pinned, unless `purge` was asked for — which also erases the history of a Root no longer watched."),
        (status = 400, description = "`purge` is not a boolean.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root is watched — or, with `purge`, kept since it was unwatched.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The Registry could not be written, or the history could not be pinned.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn deregister(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
    query: Result<Query<DeleteQuery>, QueryRejection>,
) -> Result<StatusCode, ProblemResponse> {
    let Query(query) = query.map_err(|rejection| {
        ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::PurgeInvalid,
            rejection.body_text(),
        )
    })?;
    let id = parse_id(&id)?;
    let purge = query.purge;

    // Erasing waits for a capture already in the store, and a capture that waited for this re-checks the
    // Registry before it opens the store, so an erased store never comes back as an orphan. Keeping the
    // history pins its last state, which a Maintenance Pass must not prune under (ticket 53).
    under_store_lock(state, id, move |state| {
        let registry = state.registry();
        let pinned = roots::deregister(&registry, id, purge).map_err(root_problem)?;
        if let Some(pinned) = pinned {
            tracing::info!(root = %id, snapshot = %pinned, "no longer watched; its last state is pinned");
        }
        state.publish(Event::RootUnregistered {
            root: id.to_string(),
            purged: purge,
        });
        configuration::reload_with(state, &registry);

        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Erase a Root's history and keep watching it: what `keepster purge` asks for (ticket 68).
#[utoipa::path(
    delete,
    path = "/v1/roots/{id}/history",
    operation_id = "erase_root_history",
    tag = "roots",
    params(RootAddress),
    responses(
        (status = 204, description = "The Root's history is erased. A watched Root stays registered and watched, and its next capture starts the history over, reading every file again; one no longer watched is erased whole."),
        (status = 404, description = "No such Root is watched, or kept since it was unwatched.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The history could not be erased.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn erase_history(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
) -> Result<StatusCode, ProblemResponse> {
    let id = parse_id(&id)?;

    // Under the store's lock no capture is mid-write, and the next one opens a fresh store.
    under_store_lock(state, id, move |state| {
        roots::erase_history(&state.registry(), id).map_err(root_problem)?;
        tracing::info!(root = %id, "its history is erased");
        state.publish(Event::HistoryErased {
            root: id.to_string(),
        });
        // A Root still watched captures its tree afresh now rather than at its next change, which would
        // otherwise replace a version no Snapshot holds. One no longer watched has no runtime to owe it.
        state.runtimes.reconcile(id);

        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Run `work` on the Root `id`'s History Store while holding that store's lock, off the async runtime:
/// the lock may wait for a capture's walk, and the work writes to git. The store's lock is taken before
/// any Registry lock `work` takes, the one order every holder of both keeps.
async fn under_store_lock<F>(
    state: DaemonState,
    id: RootId,
    work: F,
) -> Result<StatusCode, ProblemResponse>
where
    F: FnOnce(&DaemonState) -> Result<StatusCode, ProblemResponse> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let store = state.store_lock(id);
        let _held = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        work(&state)
    })
    .await
    .map_err(|joined| {
        ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::RegistryFailed,
            joined.to_string(),
        )
    })?
}

/// Record that a Root's directory has moved.
#[utoipa::path(
    patch,
    path = "/v1/roots/{id}",
    operation_id = "move_root",
    tag = "roots",
    params(RootAddress),
    request_body = RegisterRoot,
    responses(
        (status = 200, description = "The Root, at its new path.", body = Root),
        (status = 400, description = "The new path is not a directory; or the body is not JSON.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root is registered.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The new path overlaps another Root.", body = Problem, content_type = "application/problem+json"),
        (status = 415, description = "The body is not declared as `application/json`.", body = Problem, content_type = "application/problem+json"),
        (status = 422, description = "The body is JSON of the wrong shape.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The Registry could not be written.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn move_root(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
    body: Result<Json<RegisterRoot>, JsonRejection>,
) -> Result<Json<Root>, ProblemResponse> {
    let Json(request) = body.map_err(body_problem)?;
    let id = parse_id(&id)?;
    let registry = state.registry();
    let record = roots::move_root(&registry, id, request.path.as_ref(), &|path| {
        crate::volumes::place_of(&*state.volumes, path)
    })
    .map_err(root_problem)?;
    state.publish(Event::RootMoved {
        root: id.to_string(),
    });
    configuration::reload_with(&state, &registry);

    Ok(Json(describe_record(&state, &record)))
}

/// Every Root as `status` and the registry listing report it.
///
/// The same rendering both use, because ticket 07 keeps a Root's state on the Root itself rather than in
/// a second health shape.
pub(crate) fn reported(
    state: &DaemonState,
    registry: &keepster_store::Registry,
) -> Result<Vec<Root>, ProblemResponse> {
    let entries = roots::list(registry).map_err(registry_problem)?;

    Ok(entries.iter().map(|entry| describe(state, entry)).collect())
}

/// The Registry's record for one Root, or the problem that explains why there is none.
///
/// Shared by every endpoint that acts on a Root rather than reporting on one: a Root that is not
/// registered is not found, and one whose record cannot be read is `Unavailable`, which is what
/// `GLOSSARY.md` calls a Root that cannot be captured at all.
pub(crate) fn record(
    registry: &keepster_store::Registry,
    id: RootId,
) -> Result<RootRecord, ProblemResponse> {
    let entry = roots::list(registry)
        .map_err(registry_problem)?
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| not_found(&id.to_string()))?;

    entry.record.map_err(|error| {
        ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::RootUnavailable,
            error.to_string(),
        )
        .with_detail("root", id.to_string())
    })
}

/// A Root identity from a URL segment.
///
/// Only the canonical form is accepted: giving one resource two URLs would make a bookmarked link
/// depend on which form produced it (ticket 07). Prefixes and paths are the CLI's convenience, resolved
/// before a request is made.
pub(crate) fn parse_id(segment: &str) -> Result<RootId, ProblemResponse> {
    segment.parse().map_err(|_| not_found(segment))
}

pub(crate) fn not_found(segment: &str) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::NOT_FOUND,
        ErrorCode::RootNotFound,
        format!("no Root `{segment}` is registered"),
    )
    .with_detail("root", segment)
}

/// A body the framework could not turn into the request type, in the one shape every endpoint uses.
///
/// The status stays the framework's — `415` for a body that is not declared as JSON, `400` for one that
/// is not JSON, `422` for JSON of the wrong shape — and the code is one, because a client reacts to all
/// three the same way: by fixing what it sends.
fn body_problem(rejection: JsonRejection) -> ProblemResponse {
    ProblemResponse::new(
        rejection.status(),
        ErrorCode::BodyInvalid,
        rejection.body_text(),
    )
}

/// A `path=` query parameter, which must be absolute.
///
/// Resolving a relative path here would resolve it against the *daemon's* working directory, which is
/// meaningless to the client that sent it — so it is refused rather than answered wrongly.
pub(crate) fn absolute_query_path(path: &str) -> Result<std::path::PathBuf, ProblemResponse> {
    let path = std::path::PathBuf::from(path);

    if path.is_absolute() {
        Ok(path)
    } else {
        Err(ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::PathInvalid,
            "a `path` query parameter must be absolute",
        )
        .with_detail("path", path.to_string_lossy()))
    }
}

/// The one wall-clock read this crate performs, and the reason ADR 0012 puts the lint where it does:
/// a Root's identity embeds the moment it was minted, so the moment has to enter the domain from here.
#[allow(
    clippy::disallowed_methods,
    reason = "ADR 0012: the daemon is where a moment enters the domain, and this is that point"
)]
pub(crate) fn now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

/// `moment` as RFC 3339 in UTC, as the wire spells a moment; `None` for one outside what it can spell.
pub(crate) fn rfc3339(moment: std::time::SystemTime) -> Option<String> {
    jiff::Timestamp::try_from(moment)
        .ok()
        .map(|moment| moment.to_string())
}

/// How a Root appears on the wire, including one this build cannot serve.
///
/// A Root whose record cannot be read is still reported, as `Unavailable` with the reason: ticket 08
/// makes such a Root a value rather than an error, and ticket 26 refuses a newer store format as
/// narrowly as its level allows — this Root, not the whole listing.
fn describe(state: &DaemonState, entry: &RegistryEntry) -> Root {
    match &entry.record {
        Ok(record) => describe_record(state, record),
        Err(error) => Root {
            id: entry.id.to_string(),
            // A record too new to trust still parsed, so its path is known; a corrupt one gives nothing,
            // and saying so beats inventing a path.
            path: unreadable_path(error),
            state: RootState::Unavailable,
            pending_paths: 0,
            oldest_pending_seconds: None,
            follow_targets: None,
            unrecognized_records: None,
            state_code: Some(
                match error {
                    RegistryError::TooNew { .. } => "store_too_new",
                    _ => "record_unreadable",
                }
                .to_owned(),
            ),
        },
    }
}

/// How a readable Root appears on the wire.
///
/// The state is the honest one this build can determine, the most serious reason first: a directory
/// that is gone means nothing can be captured at all, which `GLOSSARY.md` calls `Unavailable`, and so
/// does a configuration with no version to apply; one whose current version is invalid leaves the last
/// known-good in force, `Degraded`. Past that, a Root is `Watching` only while the guarantee holds as
/// promised — Reconciliation at the start, a watcher or a periodic pass, and a `stat` filter worth its
/// name — and `Degraded` with the reason otherwise.
fn describe_record(daemon: &DaemonState, record: &keepster_store::RootRecord) -> Root {
    let activity = daemon.runtimes.activity(record.id).unwrap_or_default();
    // From the base rather than the Registry, whose lock a caller may be holding.
    let location =
        keepster_app::snapshots::location(&daemon.store_base.join(record.id.to_string()));
    let state = if !record.path.is_dir() {
        Some((RootState::Unavailable, "root_missing"))
    } else if crate::volumes::verdict(&*daemon.volumes, record) == VolumeVerdict::Elsewhere {
        // A directory, but on a volume other than the one registered — an empty mount point, most
        // likely: nothing may be captured from it, and the history is as readable as ever (ADR 0011).
        Some((RootState::Unavailable, "volume_mismatch"))
    } else if daemon.outcomes.is_unreachable(record.id)
        || (location.store.exists() && !location.store.is_dir())
    {
        // Nowhere to write, and nowhere else is written instead: there is no fallback store.
        Some((RootState::Unavailable, "store_unreachable"))
    } else if let Some(state) =
        configuration::state_of(&daemon.configuration(record.id, &record.path))
    {
        Some(state)
    } else if daemon.outcomes.is_held(record.id) {
        // Every path is gone where nothing proves the tree is the one registered: held, until a
        // capture someone asks for confirms it, or the tree comes back (ADR 0011).
        Some((RootState::Degraded, "capture_held"))
    } else {
        coverage(&activity).map(|code| (RootState::Degraded, code))
    };
    let (state, state_code) = match state {
        Some((state, code)) => (state, Some(code.to_owned())),
        None => (RootState::Watching, None),
    };

    Root {
        id: record.id.to_string(),
        // Ticket 99 adds the byte-exact representation for an absolute path that is not UTF-8 (ticket 74
        // made only the paths inside a Root bytes); until then a path only reaches the daemon as text, so
        // nothing lossy can be registered in the first place.
        path: record.path.to_string_lossy().into_owned(),
        state,
        state_code,
        pending_paths: activity.pending_paths,
        oldest_pending_seconds: activity
            .pending_since
            .map(|since| since.elapsed().as_secs()),
        follow_targets: activity.follow_every.map(|interval| FollowTargets {
            count: activity.followed,
            last_reconciled_at: activity.followed_at.and_then(rfc3339),
            interval: interval.as_secs(),
        }),
        unrecognized_records: (activity.unrecognized > 0).then_some(activity.unrecognized),
    }
}

/// Why a Root that is captured automatically is not covered as promised, if it is not.
fn coverage(activity: &triggers::Activity) -> Option<&'static str> {
    if !activity.automatic {
        // Every Trigger is off: only a capture or a Reconciliation someone asks for ever happens.
        Some("capture_unavailable")
    } else if !activity.at_start || !activity.reconciled {
        // Whatever changed while nobody watched stays uncaptured until a Reconciliation has run — or, with
        // the start's walk switched off, until something else changes.
        Some("coverage_not_guaranteed")
    } else if activity.watch == triggers::Watch::Ceiling {
        Some("watch_ceiling")
    } else if activity.watch == triggers::Watch::Failed {
        Some("watch_unavailable")
    } else if activity.watch == triggers::Watch::Blind {
        // A network or FUSE filesystem the platform's watcher is documented as blind on: reconciled
        // periodically instead, and said so (ticket 29).
        Some("watch_blind")
    } else if activity.coarse_mtime && activity.verify == keepster_core::Verify::Stat {
        // ADR 0014: the user decides whether to pay for hashing everything, so the weakness is said.
        Some("mtime_coarse")
    } else {
        None
    }
}

fn unreadable_path(error: &RegistryError) -> String {
    match error {
        RegistryError::TooNew { path, .. } => path.to_string_lossy().into_owned(),
        _ => String::new(),
    }
}

fn register_problem(error: RegisterError) -> ProblemResponse {
    match error {
        RegisterError::NotADirectory { ref path, .. }
        | RegisterError::Unresolvable { ref path, .. } => ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::PathInvalid,
            error.to_string(),
        )
        .with_detail("path", path.to_string_lossy()),
        RegisterError::Overlaps { ref path, existing } => ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::RootOverlaps,
            error.to_string(),
        )
        .with_detail("path", path.to_string_lossy())
        .with_detail("root", existing.to_string()),
        RegisterError::Registry(error) => registry_problem(error),
    }
}

fn root_problem(error: RootError) -> ProblemResponse {
    match error {
        RootError::NotFound { id } => not_found(&id.to_string()),
        RootError::Overlaps { ref path, existing } => ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::RootOverlaps,
            error.to_string(),
        )
        .with_detail("path", path.to_string_lossy())
        .with_detail("root", existing.to_string()),
        RootError::NotADirectory { ref path, .. } => ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::PathInvalid,
            error.to_string(),
        )
        .with_detail("path", path.to_string_lossy()),
        RootError::Registry(error) => registry_problem(error),
        RootError::Store(error) => crate::snapshots::store_problem(error),
    }
}

/// A Registry that cannot be read at all — as opposed to one record that cannot — is a failure of the
/// request, since there is nothing to report and nothing to act on.
pub(crate) fn registry_problem(error: RegistryError) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::RegistryFailed,
        error.to_string(),
    )
}
