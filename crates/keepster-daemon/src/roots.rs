//! The Watched Root registry endpoints: registering, listing, resolving, moving and deregistering.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Json, response::IntoResponse};
use keepster_api::{RegisterRoot, Root, RootState};
use keepster_app::roots::{self, RegisterError, RootError};
use keepster_core::RootId;
use keepster_store::{RegistryEntry, RegistryError};

use crate::DaemonState;
use crate::problem::ProblemResponse;

/// Which Roots to return: all of them, or the one holding a path.
#[derive(Debug, serde::Deserialize)]
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
pub(crate) async fn list(
    State(state): State<DaemonState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Root>>, ProblemResponse> {
    let registry = state.registry();

    match &query.path {
        Some(path) => {
            let path = absolute_query_path(path)?;
            let found = roots::resolve(&registry, &path).map_err(registry_problem)?;

            Ok(Json(found.iter().map(describe_record).collect()))
        }
        None => Ok(Json(reported(&registry)?)),
    }
}

/// One Root, addressed by its identity.
pub(crate) async fn show(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> Result<Json<Root>, ProblemResponse> {
    let id = parse_id(&id)?;
    let registry = state.registry();

    reported(&registry)?
        .into_iter()
        .find(|root| root.id == id.to_string())
        .map(Json)
        .ok_or_else(|| not_found(&id.to_string()))
}

/// Register a directory as a Watched Root.
pub(crate) async fn register(
    State(state): State<DaemonState>,
    Json(request): Json<RegisterRoot>,
) -> Result<impl IntoResponse, ProblemResponse> {
    let registry = state.registry();
    let record =
        roots::register(&registry, now(), request.path.as_ref()).map_err(register_problem)?;

    Ok((StatusCode::CREATED, Json(describe_record(&record))))
}

/// Whether a deregistration also erases the History Store.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct DeleteQuery {
    /// Erase the History Store as well. Off unless asked for.
    #[serde(default)]
    purge: bool,
}

/// Deregister a Root, and with `purge=true` erase its History Store.
pub(crate) async fn deregister(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    Query(query): Query<DeleteQuery>,
) -> Result<StatusCode, ProblemResponse> {
    let id = parse_id(&id)?;
    let registry = state.registry();
    roots::deregister(&registry, id, query.purge).map_err(root_problem)?;

    Ok(StatusCode::NO_CONTENT)
}

/// Record that a Root's directory has moved.
pub(crate) async fn move_root(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    Json(request): Json<RegisterRoot>,
) -> Result<Json<Root>, ProblemResponse> {
    let id = parse_id(&id)?;
    let registry = state.registry();
    let record = roots::move_root(&registry, id, request.path.as_ref()).map_err(root_problem)?;

    Ok(Json(describe_record(&record)))
}

/// Every Root as `status` and the registry listing report it.
///
/// The same rendering both use, because ticket 07 keeps a Root's state on the Root itself rather than in
/// a second health shape.
pub(crate) fn reported(registry: &keepster_store::Registry) -> Result<Vec<Root>, ProblemResponse> {
    let entries = roots::list(registry).map_err(registry_problem)?;

    Ok(entries.iter().map(describe).collect())
}

/// A Root identity from a URL segment.
///
/// Only the canonical form is accepted: giving one resource two URLs would make a bookmarked link
/// depend on which form produced it (ticket 07). Prefixes and paths are the CLI's convenience, resolved
/// before a request is made.
fn parse_id(segment: &str) -> Result<RootId, ProblemResponse> {
    segment.parse().map_err(|_| not_found(segment))
}

fn not_found(segment: &str) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::NOT_FOUND,
        "root_not_found",
        format!("no Root `{segment}` is registered"),
    )
    .with_detail("root", segment)
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
            "path_invalid",
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
fn now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}

/// How a Root appears on the wire, including one this build cannot serve.
///
/// A Root whose record cannot be read is still reported, as `Unavailable` with the reason: ticket 08
/// makes such a Root a value rather than an error, and ticket 26 refuses a newer store format as
/// narrowly as its level allows — this Root, not the whole listing.
fn describe(entry: &RegistryEntry) -> Root {
    match &entry.record {
        Ok(record) => describe_record(record),
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
/// The state is the honest one this build can determine: a directory that is gone means nothing can be
/// captured at all, which `CONTEXT.md` calls `Unavailable`; otherwise capture simply does not exist yet
/// (tickets 43 and 44), which is a promise not kept rather than a Root that cannot be served.
fn describe_record(record: &keepster_store::RootRecord) -> Root {
    let (state, state_code) = if record.path.is_dir() {
        (RootState::Degraded, "capture_unavailable")
    } else {
        (RootState::Unavailable, "root_missing")
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
        | RegisterError::Unresolvable { ref path, .. } => {
            ProblemResponse::new(StatusCode::BAD_REQUEST, "path_invalid", error.to_string())
                .with_detail("path", path.to_string_lossy())
        }
        RegisterError::Overlaps { ref path, existing } => {
            ProblemResponse::new(StatusCode::CONFLICT, "root_overlaps", error.to_string())
                .with_detail("path", path.to_string_lossy())
                .with_detail("root", existing.to_string())
        }
        RegisterError::Registry(error) => registry_problem(error),
    }
}

fn root_problem(error: RootError) -> ProblemResponse {
    match error {
        RootError::NotFound { id } => not_found(&id.to_string()),
        RootError::Overlaps { ref path, existing } => {
            ProblemResponse::new(StatusCode::CONFLICT, "root_overlaps", error.to_string())
                .with_detail("path", path.to_string_lossy())
                .with_detail("root", existing.to_string())
        }
        RootError::NotADirectory { ref path, .. } => {
            ProblemResponse::new(StatusCode::BAD_REQUEST, "path_invalid", error.to_string())
                .with_detail("path", path.to_string_lossy())
        }
        RootError::Registry(error) => registry_problem(error),
    }
}

/// A Registry that cannot be read at all — as opposed to one record that cannot — is a failure of the
/// request, since there is nothing to report and nothing to act on.
fn registry_problem(error: RegistryError) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "registry_failed",
        error.to_string(),
    )
}
