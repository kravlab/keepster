//! Taking a Snapshot on demand.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Json, response::IntoResponse};
use keepster_api::Snapshot;
use keepster_app::snapshots::{self, CaptureError};
use keepster_store::CapturedSnapshot;

use crate::DaemonState;
use crate::problem::ProblemResponse;
use crate::roots;

/// Capture the Root's current contents now.
///
/// Ticket 43 adds the Triggers that make this happen by itself; until then a capture is something a
/// person or a script asks for, which is also what lets the write path be exercised end to end.
pub(crate) async fn create(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ProblemResponse> {
    let id = roots::parse_id(&id)?;
    let now = roots::now();

    // Converted before anything is written, so a clock outside the representable range is refused with a
    // reason rather than producing a Snapshot whose recorded moment is a blank.
    let taken_at = jiff::Timestamp::try_from(now).map_err(|_| {
        ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "clock_unusable",
            "the system clock is outside the range a Snapshot can record",
        )
    })?;

    // The Registry is read and released before the walk begins: a capture can take seconds, and an
    // unrelated Root has no reason to wait for it.
    let (root, store_path) = {
        let registry = state.registry();
        let entry = roots::record(&registry, id)?;

        (
            entry,
            keepster_app::snapshots::store_path(&registry.root_dir(id)),
        )
    };

    // One capture at a time per store: git documents a corruption risk under concurrent access, and two
    // captures racing would leave one of them unreferenced whatever git did (ticket 06).
    let store = state.store_lock(id);
    let _held = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let captured = snapshots::capture(&root, &store_path, now).map_err(capture_problem)?;

    Ok((StatusCode::CREATED, Json(describe(&captured, taken_at))))
}

/// How a Snapshot appears on the wire.
///
/// The moment crosses the wire as RFC 3339 in UTC and nothing else: a client wanting a local reading can
/// convert, while an offset recorded here would make two Snapshots taken an hour apart incomparable as
/// text.
fn describe(captured: &CapturedSnapshot, taken_at: jiff::Timestamp) -> Snapshot {
    Snapshot {
        id: captured.id.to_string(),
        segment: captured.segment.as_str().to_owned(),
        taken_at: taken_at.to_string(),
        incomplete: !captured.unreadable.is_empty(),
        omitted: captured
            .unreadable
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    }
}

fn capture_problem(error: CaptureError) -> ProblemResponse {
    match error {
        CaptureError::NotFound { id } => {
            ProblemResponse::new(StatusCode::NOT_FOUND, "root_not_found", error.to_string())
                .with_detail("root", id.to_string())
        }
        CaptureError::RootUnavailable { ref path } => {
            ProblemResponse::new(StatusCode::CONFLICT, "root_unavailable", error.to_string())
                .with_detail("path", path.to_string_lossy())
        }
        CaptureError::Unreadable { id, .. } => {
            ProblemResponse::new(StatusCode::CONFLICT, "root_unavailable", error.to_string())
                .with_detail("root", id.to_string())
        }
        CaptureError::Registry(_) | CaptureError::Store(_) => ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "capture_failed",
            error.to_string(),
        ),
    }
}
