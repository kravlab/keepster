//! What changed from a Snapshot to another Snapshot or the Working Tree, computed here where both
//! sides are (ticket 46).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use keepster_api::{
    Diff, DiffFile, DiffHunk, DiffLine, ErrorCode, FileChange, LineKind, Problem, WORKING_TREE,
};
use keepster_app::snapshots;
use keepster_core::{RootPath, SnapshotId};
use keepster_store::{Comparison, DiffTarget, FileDiff, StoreError};

use crate::DaemonState;
use crate::path_parameter::PathParameter;
use crate::problem::ProblemResponse;
use crate::roots::{self, RootAddress};
use crate::snapshots::{blocking, located, prepare, store_problem};

/// What to compare, and how to answer.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct DiffParameters {
    /// The older side: a Snapshot's identity. "Versus the previous Snapshot" is not a mode — a client
    /// has the previous identity from the timeline already.
    from: Option<String>,
    /// The newer side: a Snapshot's identity, or `working` for the Working Tree as the next Snapshot
    /// would hold it — under the Root's Include Rules, so an excluded file is no change.
    to: Option<String>,
    /// `json` (the default) for the structured answer, or `unified` for the patch text git prints.
    format: Option<String>,
}

/// The newer side, as asked for.
#[derive(Clone, Copy)]
enum Side {
    Snapshot(SnapshotId),
    Working,
}

impl Side {
    fn parse(given: Option<&str>) -> Result<Self, ProblemResponse> {
        match given {
            Some(WORKING_TREE) => Ok(Self::Working),
            Some(to) => to.parse().map(Self::Snapshot).map_err(|_| {
                comparison_invalid("`to` must be a Snapshot's identity or `working`")
                    .with_detail("to", to)
            }),
            None => Err(comparison_invalid(
                "`to` must be a Snapshot's identity or `working`",
            )),
        }
    }

    /// The side as the answer names it.
    fn name(self) -> String {
        match self {
            Self::Snapshot(snapshot) => snapshot.to_string(),
            Self::Working => WORKING_TREE.to_owned(),
        }
    }
}

/// How the answer is written.
#[derive(Clone, Copy)]
enum Format {
    Json,
    Unified,
}

impl Format {
    fn parse(given: Option<&str>) -> Result<Self, ProblemResponse> {
        match given {
            None | Some("json") => Ok(Self::Json),
            Some("unified") => Ok(Self::Unified),
            Some(other) => Err(ProblemResponse::new(
                StatusCode::BAD_REQUEST,
                ErrorCode::FormatInvalid,
                "`format` must be `json` or `unified`",
            )
            .with_detail("format", other)),
        }
    }
}

/// What changed between a Snapshot and another Snapshot or the Working Tree.
#[utoipa::path(
    get,
    path = "/v1/roots/{id}/diff",
    operation_id = "diff_root",
    tag = "snapshots",
    params(
        RootAddress,
        DiffParameters,
        ("path" = Option<String>, Query, description = "One file, Root-relative, its bytes percent-encoded, by either name if it was renamed: its hunks rather than the summary."),
    ),
    responses(
        (status = 200, description = "Without `path`, every changed file in path order, without hunks; with it, that file and its hunks. A rename is one change; when a candidate is larger than 8 MiB, renames are found by identical content only. A binary file, and one whose either version is larger than 8 MiB, is flagged and has no hunks. Against the Working Tree, a file the next Snapshot would withhold says why, and files that could not be read are listed in `omitted`. `format=unified` answers with the patch text git prints instead — every changed file's, or the one `path` names — so no client reassembles it from hunks; it is the exact bytes, where the JSON shows text that is not UTF-8 with replacement characters, and carries `Content-Disposition: attachment` so a browser saves it as it is.", content(
            (Diff = "application/json"),
            (String = "text/x-diff"),
        )),
        (status = 400, description = "`from`, `to`, `path` or `format` is malformed or missing.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root or Snapshot, or the path is on neither side.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The path is not a file; or, comparing with the Working Tree, the Root is unavailable or its configuration has never been valid.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The history or the Working Tree could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn diff(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
    PathParameter(path): PathParameter,
    Query(parameters): Query<DiffParameters>,
) -> Result<Response, ProblemResponse> {
    let from = parameters
        .from
        .as_deref()
        .and_then(|from| from.parse::<SnapshotId>().ok())
        .ok_or_else(|| {
            let problem = comparison_invalid("`from` must be a Snapshot's identity");
            match parameters.from.as_deref() {
                Some(given) => problem.with_detail("from", given),
                None => problem,
            }
        })?;
    let to = Side::parse(parameters.to.as_deref())?;
    let format = Format::parse(parameters.format.as_deref())?;

    let answer = match to {
        Side::Snapshot(to) => {
            let (_, location) = located(&state, &id)?;
            blocking(move || {
                compute(
                    &location,
                    from,
                    &DiffTarget::Snapshot(to),
                    path.as_ref(),
                    format,
                )
            })
            .await?
        }
        Side::Working => working(state, &id, from, path, format).await?,
    };

    Ok(match answer {
        // Saved, not shown: a patch is applied byte for byte, and a browser's text view is no copy of it.
        Answer::Unified(patch) => (
            [
                (
                    header::CONTENT_TYPE,
                    axum::http::HeaderValue::from_static("text/x-diff"),
                ),
                (
                    header::CONTENT_DISPOSITION,
                    crate::content::attachment(&format!("{from}..{}.diff", to.name())),
                ),
            ],
            patch,
        )
            .into_response(),
        Answer::Files(comparison) => Json(Diff {
            from: from.to_string(),
            to: to.name(),
            files: comparison.files.into_iter().map(wire).collect(),
            omitted: crate::wire::texts(
                comparison
                    .incomplete
                    .iter()
                    .map(|incomplete| &incomplete.path),
            ),
            omitted_bytes: crate::wire::companions(
                comparison
                    .incomplete
                    .iter()
                    .map(|incomplete| &incomplete.path),
            ),
        })
        .into_response(),
    })
}

/// The diff from the Snapshot `from` to the Root's Working Tree.
///
/// The Working Tree is written into the store as a capture writes it, so it is compared under the
/// store's lock as a capture is taken: never two writers in one store (ticket 06). A Root deregistered —
/// and perhaps erased — while this waited for the lock is not written into, as a capture re-checks too.
async fn working(
    state: DaemonState,
    id: &str,
    from: SnapshotId,
    path: Option<RootPath>,
    format: Format,
) -> Result<Answer, ProblemResponse> {
    let id = roots::parse_id(id)?;
    let (root, location, loaded) = prepare(&state, id)?;
    let now = roots::now();

    tokio::task::spawn_blocking(move || {
        let store = state.store_lock(id);
        let _held = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.registry().contains(id) {
            return Err(roots::not_found(&id.to_string()));
        }
        let working = DiffTarget::WorkingTree {
            root_path: &root.path,
            policy: &loaded.policy,
            now,
        };

        compute(&location, from, &working, path.as_ref(), format).map_err(|error| match error {
            // The Root's own directory is gone or unreadable: a state the Root is in, as for a capture.
            StoreError::WorkingTree { ref path, .. } => ProblemResponse::new(
                StatusCode::CONFLICT,
                ErrorCode::RootUnavailable,
                error.to_string(),
            )
            .with_detail("path", path.to_string_lossy()),
            other => store_problem(other),
        })
    })
    .await
    .map_err(|joined| {
        ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::HistoryUnreadable,
            joined.to_string(),
        )
    })?
}

/// A diff, in the form asked for.
enum Answer {
    Files(Comparison),
    Unified(Vec<u8>),
}

/// The comparison from `from` to `to`, written as `format` asks.
fn compute(
    location: &snapshots::StoreLocation,
    from: SnapshotId,
    to: &DiffTarget<'_>,
    path: Option<&RootPath>,
    format: Format,
) -> Result<Answer, StoreError> {
    match format {
        Format::Unified => snapshots::unified(location, from, to, path).map(Answer::Unified),
        Format::Json => snapshots::diff(location, from, to, path).map(Answer::Files),
    }
}

/// A side of the comparison that names no Snapshot, or `to` that names neither one nor the Working Tree.
fn comparison_invalid(message: &'static str) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::BAD_REQUEST,
        ErrorCode::ComparisonInvalid,
        message,
    )
}

/// One file's part of a diff as the wire carries it.
fn wire(file: FileDiff) -> DiffFile {
    DiffFile {
        path: crate::wire::text(&file.path),
        path_bytes: crate::wire::companion(&file.path),
        old_path: file.old_path.as_ref().map(crate::wire::text),
        old_path_bytes: file.old_path.as_ref().and_then(crate::wire::companion),
        change: file.change.map(|change| match change {
            keepster_store::FileChange::Added => FileChange::Added,
            keepster_store::FileChange::Modified => FileChange::Modified,
            keepster_store::FileChange::Deleted => FileChange::Deleted,
            keepster_store::FileChange::Renamed => FileChange::Renamed,
        }),
        binary: file.binary,
        too_large: file.too_large,
        withheld: file.withheld.map(|reason| reason.as_str().to_owned()),
        old_mode: file.old_mode,
        new_mode: file.new_mode,
        hunks: file
            .hunks
            .into_iter()
            .map(|hunk| DiffHunk {
                old_start: hunk.old_start,
                old_lines: hunk.old_lines,
                new_start: hunk.new_start,
                new_lines: hunk.new_lines,
                lines: hunk
                    .lines
                    .into_iter()
                    .map(|line| DiffLine {
                        kind: match line.kind {
                            keepster_store::LineKind::Context => LineKind::Context,
                            keepster_store::LineKind::Added => LineKind::Added,
                            keepster_store::LineKind::Deleted => LineKind::Deleted,
                        },
                        text: String::from_utf8_lossy(&line.text).into_owned(),
                        newline: line.newline,
                    })
                    .collect(),
            })
            .collect(),
    }
}
