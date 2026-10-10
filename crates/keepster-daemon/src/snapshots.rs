//! Snapshots: taking one on demand, and reading history back — the timeline, a Snapshot's entries and
//! one version's bytes.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Json, response::IntoResponse};
use keepster_api::{Change, Entries, Entry, EntryKind, Snapshot, Timeline};
use keepster_app::snapshots::{self, CaptureError, StoreLocation};
use keepster_core::{ChangeKind, RootId, RootPath, SnapshotId, Trigger};
use keepster_store::{CapturedSnapshot, Depth, SnapshotSummary, StoreError, TimelineQuery};

use crate::DaemonState;
use crate::problem::ProblemResponse;
use crate::roots;

/// How many Snapshots a timeline page holds when the client does not say (ticket 07).
const DEFAULT_LIMIT: usize = 100;

/// The most a timeline page holds whatever the client asks for (ticket 07).
const MAX_LIMIT: usize = 1_000;

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
    jiff::Timestamp::try_from(now).map_err(|_| {
        ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "clock_unusable",
            "the system clock is outside the range a Snapshot can record",
        )
    })?;

    // The Registry is read and released before the walk begins: a capture can take seconds, and an
    // unrelated Root has no reason to wait for it.
    let (root, location) = {
        let registry = state.registry();
        let entry = roots::record(&registry, id)?;

        (entry, snapshots::location(&registry.root_dir(id)))
    };

    // One capture at a time per store: git documents a corruption risk under concurrent access, and two
    // captures racing would leave one of them unreferenced whatever git did (ticket 06).
    let store = state.store_lock(id);
    let _held = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let captured =
        snapshots::capture(&root, &location, Trigger::Manual, now).map_err(capture_problem)?;
    if let Some(reason) = &captured.index_lag {
        // The Snapshot is durable — git came first — so this is not the request's failure. The index
        // lags, every read catches it up, and so does the next start.
        tracing::warn!(root = %id, snapshot = %captured.id, %reason, "the history index lags");
    }

    Ok((StatusCode::CREATED, Json(describe_captured(&captured))))
}

/// Which part of a Root's chronology to list. Every field is text, so a malformed value is answered
/// with a Problem naming it rather than with a framework's own refusal.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct TimelineParameters {
    /// Only the Snapshots that changed this Root-relative path.
    path: Option<String>,
    /// Only Snapshots taken at or before this RFC 3339 moment.
    before: Option<String>,
    /// Only Snapshots taken strictly after this RFC 3339 moment.
    after: Option<String>,
    /// At most this many; 100 by default, 1000 at most.
    limit: Option<String>,
    /// The `next_cursor` of the previous page.
    cursor: Option<String>,
}

/// One page of a Root's chronology, newest first — or, with `path`, of the Snapshots that changed it.
///
/// One endpoint for both questions, because they return the same shape and a client would otherwise
/// render one table with two code paths (ticket 07). A Root never captured has an empty chronology.
pub(crate) async fn list(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
    Query(parameters): Query<TimelineParameters>,
) -> Result<Json<Timeline>, ProblemResponse> {
    let (_, location) = located(&state, &id)?;

    let path = parameters.path.as_deref().map(parse_path).transpose()?;
    let limit = match parameters.limit.as_deref() {
        None => DEFAULT_LIMIT,
        Some(text) => match text.parse::<usize>() {
            Ok(0) | Err(_) => {
                return Err(ProblemResponse::new(
                    StatusCode::BAD_REQUEST,
                    "limit_invalid",
                    "`limit` must be a positive whole number",
                )
                .with_detail("limit", text));
            }
            // Capped rather than refused: asking for more than a page holds is not a mistake worth
            // failing a request over, and the cursor carries the reader on.
            Ok(limit) => limit.min(MAX_LIMIT),
        },
    };
    let cursor = parameters
        .cursor
        .as_deref()
        .map(|text| {
            text.parse::<SnapshotId>().map_err(|_| {
                ProblemResponse::new(
                    StatusCode::BAD_REQUEST,
                    "cursor_invalid",
                    "`cursor` must be a `next_cursor` this API returned",
                )
                .with_detail("cursor", text)
            })
        })
        .transpose()?;
    // "At or before a moment" is "before the first identity after it", so a moment and a cursor are
    // both an upper bound in one order, and the tighter one wins.
    let before = parameters
        .before
        .as_deref()
        .map(|text| parse_moment("before", text))
        .transpose()?
        .map(SnapshotId::first_after);
    let not_before = parameters
        .after
        .as_deref()
        .map(|text| parse_moment("after", text))
        .transpose()?
        .map(SnapshotId::first_after);
    let before = match (cursor, before) {
        (Some(cursor), Some(before)) => Some(cursor.min(before)),
        (cursor, before) => cursor.or(before),
    };

    let query = TimelineQuery {
        path,
        before,
        not_before,
        limit,
    };
    let page = blocking(move || snapshots::timeline(&location, &query)).await?;

    Ok(Json(Timeline {
        next_cursor: if page.more {
            page.snapshots
                .last()
                .map(|snapshot| snapshot.id.to_string())
        } else {
            None
        },
        snapshots: page.snapshots.iter().map(describe_summary).collect(),
    }))
}

/// Where and how deep to list a Snapshot's entries.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct EntriesParameters {
    /// The directory to list, Root-relative; the Root itself when absent.
    path: Option<String>,
    /// `1` for the directory's own children (the default), `infinity` for everything beneath it.
    depth: Option<String>,
}

/// The paths a Snapshot holds under a directory.
///
/// Shallow by default, because a 50 000-file tree is expanded lazily in a UI anyway; `infinity` serves
/// the CLI and scripts (ticket 07).
pub(crate) async fn entries(
    State(state): State<DaemonState>,
    Path((id, snapshot)): Path<(String, String)>,
    Query(parameters): Query<EntriesParameters>,
) -> Result<Json<Entries>, ProblemResponse> {
    let (_, location) = located(&state, &id)?;
    let snapshot = parse_snapshot(&snapshot)?;
    let directory = parse_path(parameters.path.as_deref().unwrap_or_default())?;
    let depth = match parameters.depth.as_deref() {
        None | Some("1") => Depth::One,
        Some("infinity") => Depth::Infinity,
        Some(other) => {
            return Err(ProblemResponse::new(
                StatusCode::BAD_REQUEST,
                "depth_invalid",
                "`depth` must be `1` or `infinity`",
            )
            .with_detail("depth", other));
        }
    };

    let found =
        blocking(move || snapshots::entries(&location, snapshot, &directory, depth)).await?;

    Ok(Json(Entries {
        entries: found
            .into_iter()
            .map(|entry| Entry {
                path: entry.path.as_str().to_owned(),
                kind: match entry.kind {
                    keepster_store::EntryKind::File => EntryKind::File,
                    keepster_store::EntryKind::Directory => EntryKind::Dir,
                    keepster_store::EntryKind::Symlink => EntryKind::Symlink,
                },
                size: entry.size,
                mode: entry.mode,
                binary: entry.binary,
            })
            .collect(),
    }))
}

/// Which version's bytes to read.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ContentParameters {
    /// The file, Root-relative.
    path: Option<String>,
}

/// One version's raw bytes, with `Content-Length`, `ETag` and `Range` (ticket 07).
pub(crate) async fn content(
    State(state): State<DaemonState>,
    Path((id, snapshot)): Path<(String, String)>,
    Query(parameters): Query<ContentParameters>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, ProblemResponse> {
    let (_, location) = located(&state, &id)?;
    let snapshot = parse_snapshot(&snapshot)?;
    let path = parse_path(parameters.path.as_deref().unwrap_or_default())?;

    crate::content::serve(location, snapshot, path, &headers).await
}

/// The Root a reading request is about, and where its history lives.
///
/// The Registry is held only long enough to find the directory: reads take no lock on the store
/// (ticket 06), and they must not wait on a capture's walk either.
fn located(state: &DaemonState, id: &str) -> Result<(RootId, StoreLocation), ProblemResponse> {
    let id = roots::parse_id(id)?;
    let registry = state.registry();
    roots::record(&registry, id)?;

    Ok((id, snapshots::location(&registry.root_dir(id))))
}

/// Run a store operation off the async runtime, which must not block on disk.
pub(crate) async fn blocking<T, F>(operation: F) -> Result<T, ProblemResponse>
where
    F: FnOnce() -> Result<T, StoreError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|joined| {
            ProblemResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "history_unreadable",
                joined.to_string(),
            )
        })?
        .map_err(store_problem)
}

/// A Snapshot identity from a URL segment; anything else names no Snapshot.
fn parse_snapshot(segment: &str) -> Result<SnapshotId, ProblemResponse> {
    segment.parse().map_err(|_| snapshot_not_found(segment))
}

/// A `path=` parameter in the one form every endpoint takes (ticket 07).
fn parse_path(text: &str) -> Result<RootPath, ProblemResponse> {
    RootPath::parse(text).map_err(|reason| {
        ProblemResponse::new(StatusCode::BAD_REQUEST, "path_invalid", reason.to_string())
            .with_detail("path", text)
    })
}

/// An RFC 3339 moment from a query parameter.
fn parse_moment(name: &str, text: &str) -> Result<std::time::SystemTime, ProblemResponse> {
    text.parse::<jiff::Timestamp>()
        .map(std::time::SystemTime::from)
        .map_err(|_| {
            ProblemResponse::new(
                StatusCode::BAD_REQUEST,
                "moment_invalid",
                format!("`{name}` must be an RFC 3339 moment, such as 2026-09-30T14:37:05Z"),
            )
            .with_detail(name, text)
        })
}

/// When a Snapshot was taken, as the wire spells it: RFC 3339 in UTC, from the identity's own moment.
///
/// From the identity rather than the clock reading or the commit, so a capture's answer and the
/// timeline agree to the millisecond, and a rebuilt index reports exactly what the original did.
fn taken_at(id: SnapshotId) -> String {
    jiff::Timestamp::try_from(id.moment())
        .map(|moment| moment.to_string())
        .unwrap_or_default()
}

/// How a Snapshot just taken appears on the wire.
fn describe_captured(captured: &CapturedSnapshot) -> Snapshot {
    Snapshot {
        id: captured.id.to_string(),
        segment: captured.segment.as_str().to_owned(),
        taken_at: taken_at(captured.id),
        trigger: captured.trigger.as_str().to_owned(),
        incomplete: !captured.unreadable.is_empty(),
        omitted: captured
            .unreadable
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        changed_paths: captured.changed_paths,
        change: None,
    }
}

/// How a Snapshot in the timeline appears on the wire.
fn describe_summary(summary: &SnapshotSummary) -> Snapshot {
    Snapshot {
        id: summary.id.to_string(),
        segment: summary.segment.clone(),
        taken_at: taken_at(summary.id),
        trigger: summary.trigger.clone(),
        incomplete: summary.omitted > 0,
        omitted: Vec::new(),
        changed_paths: summary.changed_paths,
        change: summary.change.map(|kind| match kind {
            ChangeKind::Added => Change::Added,
            ChangeKind::Modified => Change::Modified,
            ChangeKind::Deleted => Change::Deleted,
        }),
    }
}

fn snapshot_not_found(segment: &str) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::NOT_FOUND,
        "snapshot_not_found",
        format!("no Snapshot `{segment}` is in this Root's history"),
    )
    .with_detail("snapshot", segment)
}

/// A store failure as the one response shape every endpoint uses.
pub(crate) fn store_problem(error: StoreError) -> ProblemResponse {
    match error {
        StoreError::SnapshotNotFound(id) => snapshot_not_found(&id.to_string()),
        StoreError::PathNotFound(ref path) => {
            ProblemResponse::new(StatusCode::NOT_FOUND, "path_not_found", error.to_string())
                .with_detail("path", path.as_str())
        }
        StoreError::NotAFile(ref path) => {
            ProblemResponse::new(StatusCode::CONFLICT, "not_a_file", error.to_string())
                .with_detail("path", path.as_str())
        }
        StoreError::NotADirectory(ref path) => {
            ProblemResponse::new(StatusCode::CONFLICT, "not_a_directory", error.to_string())
                .with_detail("path", path.as_str())
        }
        StoreError::Git { .. }
        | StoreError::Index { .. }
        | StoreError::IndexUnusable { .. }
        | StoreError::Discard { .. }
        | StoreError::WorkingTree { .. }
        | StoreError::UnnameableMoment(_)
        | StoreError::Delivery(_) => ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "history_unreadable",
            error.to_string(),
        ),
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
