//! `/v1/roots/{id}/pins` (ticket 53): pinning and unpinning a Snapshot, synchronously — one commit
//! object written or one ref removed, finished before the answer (ticket 07) — and listing the pins.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use keepster_api::{Pins, Problem};
use keepster_app::snapshots::{self, StoreLocation};
use keepster_core::RootId;
use keepster_store::StoreError;

use crate::DaemonState;
use crate::problem::ProblemResponse;
use crate::roots::{self, RootAddress};
use crate::snapshots::{
    SnapshotAddress, blocking, describe_summary, located, parse_snapshot, store_problem,
};

/// List the Root's pinned Snapshots — including those of a Root no longer watched, whose last state
/// was pinned when it was unwatched.
#[utoipa::path(
    get,
    path = "/v1/roots/{id}/pins",
    operation_id = "list_pins",
    tag = "snapshots",
    params(RootAddress),
    responses(
        (status = 200, description = "The pinned Snapshots, newest first.", body = Pins),
        (status = 404, description = "No such Root is watched, or kept since it was unwatched.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The Root is unavailable.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The history could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn list(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
) -> Result<Json<Pins>, ProblemResponse> {
    let (_, location) = located(&state, &id)?;

    let pinned = blocking(move || snapshots::pins(&location)).await?;

    Ok(Json(Pins {
        snapshots: pinned.iter().map(describe_summary).collect(),
    }))
}

/// Pin a Snapshot, so that it survives Retention unconditionally. Pinning a pinned one changes nothing.
#[utoipa::path(
    put,
    path = "/v1/roots/{id}/pins/{snapshot}",
    operation_id = "pin_snapshot",
    tag = "snapshots",
    params(SnapshotAddress),
    responses(
        (status = 204, description = "The Snapshot is pinned."),
        (status = 404, description = "No such Root or Snapshot.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The Root is unavailable.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The Pin could not be written.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn pin(
    State(state): State<DaemonState>,
    Path(SnapshotAddress { id, snapshot }): Path<SnapshotAddress>,
) -> Result<StatusCode, ProblemResponse> {
    let (id, location) = located(&state, &id)?;
    let snapshot = parse_snapshot(&snapshot)?;

    locked(state, id, location, move |location| {
        snapshots::pin(location, snapshot).map(|_| ())
    })
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Unpin a Snapshot, which is Retention's again for as long as its Segment holds it. Unpinning one that
/// is not pinned changes nothing.
#[utoipa::path(
    delete,
    path = "/v1/roots/{id}/pins/{snapshot}",
    operation_id = "unpin_snapshot",
    tag = "snapshots",
    params(SnapshotAddress),
    responses(
        (status = 204, description = "The Snapshot is not pinned."),
        (status = 404, description = "No such Root, or no such Snapshot identity.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The Root is unavailable.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The Pin could not be removed.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn unpin(
    State(state): State<DaemonState>,
    Path(SnapshotAddress { id, snapshot }): Path<SnapshotAddress>,
) -> Result<StatusCode, ProblemResponse> {
    let (id, location) = located(&state, &id)?;
    let snapshot = parse_snapshot(&snapshot)?;

    locked(state, id, location, move |location| {
        snapshots::unpin(location, snapshot).map(|_| ())
    })
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Run `operation` on the Root's store under the store's lock, as a capture would: a Maintenance Pass
/// pruning between reading a Snapshot and writing its Pin would leave the Pin naming a tree that is
/// gone (ticket 06).
async fn locked<F>(
    state: DaemonState,
    id: RootId,
    location: StoreLocation,
    operation: F,
) -> Result<(), ProblemResponse>
where
    F: FnOnce(&StoreLocation) -> Result<(), StoreError> + Send + 'static,
{
    let held = tokio::task::spawn_blocking(move || {
        let store = state.store_lock(id);
        let _held = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Erased while this waited for the store: writing now would bring it back as an orphan.
        if !state.registry().holds(id) {
            return Ok(Err(roots::not_found(&id.to_string())));
        }

        operation(&location).map(Ok)
    });

    held.await
        .map_err(|joined| {
            ProblemResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                keepster_api::ErrorCode::HistoryUnreadable,
                joined.to_string(),
            )
        })?
        .map_err(store_problem)?
}
