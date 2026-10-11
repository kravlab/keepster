//! The Watched Root registry endpoints: registering, listing, resolving, moving and deregistering.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Json, response::IntoResponse};
use keepster_api::{ErrorCode, Problem, RegisterRoot, Root, RootState};
use keepster_app::roots::{self, RegisterError, RootError};
use keepster_core::RootId;
use keepster_store::{RegistryEntry, RegistryError, RootRecord};

use crate::DaemonState;
use crate::configuration;
use crate::problem::ProblemResponse;

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
    let record =
        roots::register(&registry, now(), request.path.as_ref()).map_err(register_problem)?;

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
        (status = 204, description = "The Root is no longer watched; its history is kept unless `purge` was asked for."),
        (status = 400, description = "`purge` is not a boolean.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root is registered.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The Registry could not be written.", body = Problem, content_type = "application/problem+json"),
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
    let registry = state.registry();
    roots::deregister(&registry, id, query.purge).map_err(root_problem)?;

    Ok(StatusCode::NO_CONTENT)
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
    let record = roots::move_root(&registry, id, request.path.as_ref()).map_err(root_problem)?;

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
/// `CONTEXT.md` calls a Root that cannot be captured at all.
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

fn not_found(segment: &str) -> ProblemResponse {
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
fn absolute_query_path(path: &str) -> Result<std::path::PathBuf, ProblemResponse> {
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
/// that is gone means nothing can be captured at all, which `CONTEXT.md` calls `Unavailable`, and so
/// does a configuration with no version to apply; one whose current version is invalid leaves the last
/// known-good in force, `Degraded`; otherwise automatic capture simply does not exist yet (tickets 43
/// and 44), which is a promise not kept rather than a Root that cannot be served.
fn describe_record(daemon: &DaemonState, record: &keepster_store::RootRecord) -> Root {
    let (state, state_code) = if !record.path.is_dir() {
        (RootState::Unavailable, "root_missing")
    } else {
        configuration::state_of(&daemon.configuration(record.id, &record.path))
            .unwrap_or((RootState::Degraded, "capture_unavailable"))
    };

    Root {
        id: record.id.to_string(),
        // Ticket 48 adds the byte-exact representation for a path that is not UTF-8; until then a path
        // only reaches the daemon as text, so nothing lossy can be registered in the first place.
        path: record.path.to_string_lossy().into_owned(),
        state,
        state_code: Some(state_code.to_owned()),
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
    }
}

/// A Registry that cannot be read at all — as opposed to one record that cannot — is a failure of the
/// request, since there is nothing to report and nothing to act on.
fn registry_problem(error: RegistryError) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::RegistryFailed,
        error.to_string(),
    )
}
