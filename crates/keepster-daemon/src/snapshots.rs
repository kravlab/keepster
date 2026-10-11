//! Snapshots: taking one on demand, and reading history back — the timeline, a Snapshot's entries and
//! one version's bytes.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Json, response::IntoResponse};
use keepster_api::{
    Change, Entries, Entry, EntryKind, ErrorCode, Problem, Reconciled, Snapshot, Timeline, Withheld,
};
use keepster_app::config::Effective;
use keepster_app::snapshots::{self, CaptureError, StoreLocation};
use keepster_core::{ChangeKind, RootId, RootPath, SnapshotId, Trigger, Verify};
use keepster_store::{
    CaptureRequest, CapturedSnapshot, Depth, RootRecord, SnapshotSummary, StoreError, TimelineQuery,
};

use crate::DaemonState;
use crate::problem::ProblemResponse;
use crate::roots::{self, RootAddress};

/// The `{id}` and `{snapshot}` segments a Snapshot is addressed by.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct SnapshotAddress {
    /// The Root's identity, in its canonical form.
    id: String,
    /// The Snapshot's identity, as the timeline returned it.
    snapshot: String,
}

/// How many Snapshots a timeline page holds when the client does not say (ticket 07).
const DEFAULT_LIMIT: usize = 100;

/// The most a timeline page holds whatever the client asks for (ticket 07).
const MAX_LIMIT: usize = 1_000;

/// Capture the Root's current contents now.
///
/// Ticket 43 adds the Triggers that make this happen by itself; until then a capture is something a
/// person or a script asks for, which is also what lets the write path be exercised end to end.
#[utoipa::path(
    post,
    path = "/v1/roots/{id}/snapshots",
    operation_id = "take_snapshot",
    tag = "snapshots",
    params(RootAddress),
    responses(
        (status = 201, description = "The Snapshot just taken. It may be incomplete: some files could not be read, and `omitted` names them.", body = Snapshot),
        (status = 404, description = "No such Root is registered.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The Root is unavailable: its directory or its record is missing or unreadable — or its configuration cannot be read, which refuses the capture rather than dropping the user's rules.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The Snapshot could not be written, or the clock cannot be recorded.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn create(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
) -> Result<impl IntoResponse, ProblemResponse> {
    let id = roots::parse_id(&id)?;
    let pass = run_pass(&state, id, Trigger::Manual, None, true)?;
    let captured = pass
        .snapshot
        .expect("a capture someone asked for writes a Snapshot even of an unchanged tree");

    Ok((StatusCode::CREATED, Json(describe_captured(&captured))))
}

/// What a client sends to reconcile a Root on demand.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub(crate) struct ReconcileRequest {
    /// `hash` reads every file, whatever the last pass observed — the one-off answer to a filesystem
    /// whose mtimes cannot be trusted. `stat`, or nothing, uses the Root's configured mode.
    #[serde(default)]
    verify: Option<VerifyMode>,
}

/// How a pass judges a file it does not read.
#[derive(Debug, Clone, Copy, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerifyMode {
    /// Read only what size, mtime or mode says changed.
    Stat,
    /// Read everything.
    Hash,
}

/// Compare the Root's Working Tree against its latest Snapshot now, and capture what differs.
///
/// The same pass the daemon runs at its start and after the platform loses events, run on request —
/// with `verify: hash`, reading every file. Nothing is written when nothing changed.
#[utoipa::path(
    post,
    path = "/v1/roots/{id}/reconcile",
    operation_id = "reconcile_root",
    tag = "snapshots",
    params(RootAddress),
    request_body = ReconcileRequest,
    responses(
        (status = 200, description = "The pass ran; `snapshot` is the one it wrote, absent when nothing had changed.", body = Reconciled),
        (status = 400, description = "The body is not JSON.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root is registered.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The Root is unavailable, or its configuration has no valid version.", body = Problem, content_type = "application/problem+json"),
        (status = 415, description = "The body is not declared as `application/json`.", body = Problem, content_type = "application/problem+json"),
        (status = 422, description = "The body is JSON of the wrong shape, such as an unknown `verify`.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The Snapshot could not be written, or the clock cannot be recorded.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn reconcile(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
    body: Result<Json<ReconcileRequest>, JsonRejection>,
) -> Result<Json<Reconciled>, ProblemResponse> {
    let Json(request) = body.map_err(|rejection| {
        ProblemResponse::new(
            rejection.status(),
            ErrorCode::BodyInvalid,
            rejection.body_text(),
        )
    })?;
    let id = roots::parse_id(&id)?;
    let verify = request.verify.map(|mode| match mode {
        VerifyMode::Stat => Verify::Stat,
        VerifyMode::Hash => Verify::Hash,
    });

    let pass = run_pass(&state, id, Trigger::Reconcile, verify, false)?;

    Ok(Json(Reconciled {
        snapshot: pass.snapshot.as_ref().map(describe_captured),
        hashed: pass.hashed,
        observed: pass.observed,
        coarse_mtime: pass.coarse_mtime,
    }))
}

/// One pass over the Root `id`, asked for by a client: under the store's lock, waiting for it if a
/// capture holds it, with `verify` overriding the configured mode.
fn run_pass(
    state: &DaemonState,
    id: RootId,
    trigger: Trigger,
    verify: Option<Verify>,
    record_unchanged: bool,
) -> Result<keepster_store::Pass, ProblemResponse> {
    let now = roots::now();

    // Converted before anything is written, so a clock outside the representable range is refused with a
    // reason rather than producing a Snapshot whose recorded moment is a blank.
    jiff::Timestamp::try_from(now).map_err(|_| {
        ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::ClockUnusable,
            "the system clock is outside the range a Snapshot can record",
        )
    })?;

    let (root, location, loaded) = prepare(state, id)?;

    // One capture at a time per store: git documents a corruption risk under concurrent access, and two
    // captures racing would leave one of them unreferenced whatever git did (ticket 06).
    let store = state.store_lock(id);
    let _held = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Deregistered — and perhaps erased — while this capture waited for the store: opening it now would
    // bring an erased store back as an orphan.
    if !state.registry().contains(id) {
        return Err(roots::not_found(&id.to_string()));
    }

    let request = CaptureRequest {
        trigger,
        now,
        verify: verify.unwrap_or(loaded.reconcile.verify),
        record_unchanged,
    };
    let pass =
        snapshots::pass(&root, &location, &loaded.policy, &request).map_err(capture_problem)?;
    if let Some(captured) = &pass.snapshot {
        if let Err(reason) = &captured.changed_paths {
            tracing::warn!(root = %id, snapshot = %captured.id, %reason, "the changes could not be counted");
        }
    }
    if let Some(reason) = &pass.index_lag {
        // The Snapshot is durable — git came first — so this is not the request's failure. The index
        // lags, every read catches it up, and so does the next start.
        tracing::warn!(root = %id, %reason, "the history index lags");
    }

    Ok(pass)
}

/// What a capture of the Root `id` needs: its record, its store and the configuration in force.
///
/// The Registry is read and released before the walk begins: a capture can take seconds, and an
/// unrelated Root has no reason to wait for it. The configuration is resolved while it is held, as
/// everywhere, because resolving may write into the Root's store directory.
///
/// Resolved before the store is locked, and never applied in part: a configuration with no version to
/// apply refuses the capture rather than falling back to defaults, which would drop the user's exclude
/// rules and pull into history exactly what they existed to keep out (ticket 04).
pub(crate) fn prepare(
    state: &DaemonState,
    id: RootId,
) -> Result<(RootRecord, StoreLocation, Effective), ProblemResponse> {
    let (root, location, loaded) = {
        let registry = state.registry();
        let entry = roots::record(&registry, id)?;
        let loaded = state.effective(id, &entry.path)?;

        (entry, snapshots::location(&registry.root_dir(id)), loaded)
    };
    for warning in &loaded.warnings {
        tracing::warn!(root = %id, code = warning.code.as_str(), "{}", warning.message);
    }

    Ok((root, location, loaded))
}

/// Which part of a Root's chronology to list. Every field is text, so a malformed value is answered
/// with a Problem naming it rather than with a framework's own refusal.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct TimelineParameters {
    /// Only the Snapshots that changed this Root-relative path.
    path: Option<String>,
    /// Only Snapshots taken at or before this RFC 3339 moment.
    #[param(format = DateTime)]
    before: Option<String>,
    /// Only Snapshots taken strictly after this RFC 3339 moment.
    #[param(format = DateTime)]
    after: Option<String>,
    /// At most this many; 100 by default, 1000 at most.
    #[param(value_type = Option<u32>, minimum = 1)]
    limit: Option<String>,
    /// The `next_cursor` of the previous page.
    cursor: Option<String>,
}

/// One page of a Root's chronology, newest first — or, with `path`, of the Snapshots that changed it.
///
/// One endpoint for both questions, because they return the same shape and a client would otherwise
/// render one table with two code paths (ticket 07). A Root never captured has an empty chronology.
#[utoipa::path(
    get,
    path = "/v1/roots/{id}/snapshots",
    operation_id = "list_snapshots",
    tag = "snapshots",
    params(RootAddress, TimelineParameters),
    responses(
        (status = 200, description = "One page of the chronology, newest first. A Root never captured has an empty one.", body = Timeline),
        (status = 400, description = "A parameter is malformed.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root is registered.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The Root is unavailable.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The history could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn list(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
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
                    ErrorCode::LimitInvalid,
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
                    ErrorCode::CursorInvalid,
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
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
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
#[utoipa::path(
    get,
    path = "/v1/roots/{id}/snapshots/{snapshot}/entries",
    operation_id = "list_entries",
    tag = "snapshots",
    params(SnapshotAddress, EntriesParameters),
    responses(
        (status = 200, description = "The paths the Snapshot holds under the directory, in byte order.", body = Entries),
        (status = 400, description = "A parameter is malformed.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root, Snapshot or path.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The Root is unavailable, or the path is not a directory.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The history could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn entries(
    State(state): State<DaemonState>,
    Path(SnapshotAddress { id, snapshot }): Path<SnapshotAddress>,
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
                ErrorCode::DepthInvalid,
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
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ContentParameters {
    /// The file, Root-relative.
    path: Option<String>,
}

/// One version's raw bytes, with `Content-Length`, `ETag` and `Range` (ticket 07).
#[utoipa::path(
    get,
    path = "/v1/roots/{id}/snapshots/{snapshot}/content",
    operation_id = "read_content",
    tag = "snapshots",
    params(
        SnapshotAddress,
        ContentParameters,
        ("Range" = Option<String>, Header, description = "One byte range, as `bytes=first-last`, `bytes=first-` or `bytes=-suffix`. Several ranges, or one that cannot be parsed, are answered with the whole content."),
        ("If-None-Match" = Option<String>, Header, description = "An `ETag` this API returned for the same version; a match is answered with `304`."),
    ),
    responses(
        (status = 200, description = "The version's raw bytes, streamed.", content_type = "application/octet-stream", headers(
            ("Content-Length" = u64, description = "The size of the version in bytes."),
            ("ETag" = String, description = "Names the content: equal tags mean equal bytes."),
            ("Accept-Ranges" = String, description = "Always `bytes`."),
        )),
        (status = 206, description = "The requested range of the version's bytes.", content_type = "application/octet-stream", headers(
            ("Content-Length" = u64, description = "The length of the range."),
            ("Content-Range" = String, description = "Which bytes, as `bytes first-last/size`."),
            ("ETag" = String, description = "Names the whole content, not the range."),
            ("Accept-Ranges" = String, description = "Always `bytes`."),
        )),
        (status = 304, description = "The client's copy, named by `If-None-Match`, is current.", headers(
            ("ETag" = String, description = "The tag that matched."),
        )),
        (status = 400, description = "A parameter is malformed.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root, Snapshot or path.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The Root is unavailable, or the path is not a file.", body = Problem, content_type = "application/problem+json"),
        (status = 416, description = "The range lies outside the content. No body.", headers(
            ("Content-Range" = String, description = "The content's size, as `bytes */size`."),
        )),
        (status = 500, description = "The history could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn content(
    State(state): State<DaemonState>,
    Path(SnapshotAddress { id, snapshot }): Path<SnapshotAddress>,
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
                ErrorCode::HistoryUnreadable,
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
pub(crate) fn parse_path(text: &str) -> Result<RootPath, ProblemResponse> {
    RootPath::parse(text).map_err(|reason| {
        ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::PathInvalid,
            reason.to_string(),
        )
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
                ErrorCode::MomentInvalid,
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
        withheld: captured
            .withheld
            .iter()
            .map(|withheld| Withheld {
                path: withheld.path.as_str().to_owned(),
                reason: withheld.reason.as_str().to_owned(),
            })
            .collect(),
        changed_paths: captured.changed_paths.clone().ok(),
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
        withheld: Vec::new(),
        changed_paths: Some(summary.changed_paths),
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
        ErrorCode::SnapshotNotFound,
        format!("no Snapshot `{segment}` is in this Root's history"),
    )
    .with_detail("snapshot", segment)
}

/// A store failure as the one response shape every endpoint uses.
pub(crate) fn store_problem(error: StoreError) -> ProblemResponse {
    match error {
        StoreError::SnapshotNotFound(id) => snapshot_not_found(&id.to_string()),
        StoreError::PathNotFound(ref path) => ProblemResponse::new(
            StatusCode::NOT_FOUND,
            ErrorCode::PathNotFound,
            error.to_string(),
        )
        .with_detail("path", path.as_str()),
        StoreError::NotAFile(ref path) => {
            ProblemResponse::new(StatusCode::CONFLICT, ErrorCode::NotAFile, error.to_string())
                .with_detail("path", path.as_str())
        }
        StoreError::NotADirectory(ref path) => ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::NotADirectory,
            error.to_string(),
        )
        .with_detail("path", path.as_str()),
        StoreError::Git { .. }
        | StoreError::Index { .. }
        | StoreError::IndexUnusable { .. }
        | StoreError::Discard { .. }
        | StoreError::WorkingTree { .. }
        | StoreError::UnnameableMoment(_)
        | StoreError::Delivery(_) => ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::HistoryUnreadable,
            error.to_string(),
        ),
    }
}

fn capture_problem(error: CaptureError) -> ProblemResponse {
    match error {
        CaptureError::NotFound { id } => ProblemResponse::new(
            StatusCode::NOT_FOUND,
            ErrorCode::RootNotFound,
            error.to_string(),
        )
        .with_detail("root", id.to_string()),
        CaptureError::RootUnavailable { ref path } => ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::RootUnavailable,
            error.to_string(),
        )
        .with_detail("path", path.to_string_lossy()),
        CaptureError::Unreadable { id, .. } => ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::RootUnavailable,
            error.to_string(),
        )
        .with_detail("root", id.to_string()),
        CaptureError::Registry(_) | CaptureError::Store(_) => ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::CaptureFailed,
            error.to_string(),
        ),
    }
}
