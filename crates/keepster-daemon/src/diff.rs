//! What changed between two versions of a Root, computed here where both are (ticket 46).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use keepster_api::{Diff, DiffFile, DiffHunk, DiffLine, ErrorCode, FileChange, LineKind, Problem};
use keepster_app::snapshots;
use keepster_core::{RootPath, SnapshotId};
use keepster_store::{DiffTarget, FileDiff};

use crate::DaemonState;
use crate::problem::ProblemResponse;
use crate::roots::{self, RootAddress};
use crate::snapshots::{blocking, located, parse_path, prepare};

/// The `to` that names the Working Tree rather than a Snapshot.
const WORKING: &str = "working";

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
    /// One file, Root-relative, by either name if it was renamed: its hunks rather than the summary.
    path: Option<String>,
    /// `json` (the default) for the structured answer, or `unified` for the patch text git prints.
    format: Option<String>,
}

/// The newer side, as asked for.
enum Side {
    Snapshot(SnapshotId),
    Working,
}

/// What changed between a Snapshot and another Snapshot or the Working Tree.
#[utoipa::path(
    get,
    path = "/v1/roots/{id}/diff",
    operation_id = "diff_root",
    tag = "snapshots",
    params(RootAddress, DiffParameters),
    responses(
        (status = 200, description = "Without `path`, every changed file in path order, without hunks; with it, that file and its hunks. A rename is one change. A binary file, and one whose either version is larger than 8 MiB, is flagged and has no hunks. `format=unified` answers with the patch text git prints instead — every changed file's, or the one `path` names — so no client reassembles it from hunks; it is the exact bytes, where the JSON shows text that is not UTF-8 with replacement characters.", content(
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
    Query(parameters): Query<DiffParameters>,
) -> Result<Response, ProblemResponse> {
    let from = parameters
        .from
        .as_deref()
        .and_then(|from| from.parse::<SnapshotId>().ok())
        .ok_or_else(|| comparison_invalid("from", parameters.from.as_deref()))?;
    let to = match parameters.to.as_deref() {
        Some(WORKING) => Side::Working,
        Some(to) => Side::Snapshot(to.parse().map_err(|_| comparison_invalid("to", Some(to)))?),
        None => return Err(comparison_invalid("to", None)),
    };
    let path = parameters.path.as_deref().map(parse_path).transpose()?;
    let unified = match parameters.format.as_deref() {
        None | Some("json") => false,
        Some("unified") => true,
        Some(other) => {
            return Err(ProblemResponse::new(
                StatusCode::BAD_REQUEST,
                ErrorCode::FormatInvalid,
                "`format` must be `json` or `unified`",
            )
            .with_detail("format", other));
        }
    };

    let answer = match to {
        Side::Snapshot(to) => {
            let (_, location) = located(&state, &id)?;
            blocking(move || {
                compute(
                    &location,
                    from,
                    &DiffTarget::Snapshot(to),
                    path.as_ref(),
                    unified,
                )
            })
            .await?
        }
        Side::Working => {
            let id = roots::parse_id(&id)?;
            let (root, location, loaded) = prepare(&state, id)?;
            if !root.path.is_dir() {
                return Err(ProblemResponse::new(
                    StatusCode::CONFLICT,
                    ErrorCode::RootUnavailable,
                    format!("the Root's directory {} is missing", root.path.display()),
                )
                .with_detail("root", id.to_string()));
            }
            let now = roots::now();
            let state = state.clone();
            blocking(move || {
                // The Working Tree is written into the store as a capture writes it, so it takes the
                // store's lock as a capture does: never two writers in one store (ticket 06).
                let store = state.store_lock(id);
                let _held = store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let working = DiffTarget::WorkingTree {
                    root_path: &root.path,
                    policy: &loaded.policy,
                    now,
                };
                compute(&location, from, &working, path.as_ref(), unified)
            })
            .await?
        }
    };

    Ok(match answer {
        Answer::Unified(patch) => ([(header::CONTENT_TYPE, "text/x-diff")], patch).into_response(),
        Answer::Files(files) => Json(Diff {
            from: from.to_string(),
            to: match to {
                Side::Snapshot(to) => to.to_string(),
                Side::Working => WORKING.to_owned(),
            },
            files: files.into_iter().map(wire).collect(),
        })
        .into_response(),
    })
}

/// A diff, in the form asked for.
enum Answer {
    Files(Vec<FileDiff>),
    Unified(Vec<u8>),
}

fn compute(
    location: &snapshots::StoreLocation,
    from: SnapshotId,
    to: &DiffTarget<'_>,
    path: Option<&RootPath>,
    unified: bool,
) -> Result<Answer, keepster_store::StoreError> {
    if unified {
        snapshots::unified(location, from, to, path).map(Answer::Unified)
    } else {
        snapshots::diff(location, from, to, path).map(Answer::Files)
    }
}

fn comparison_invalid(side: &str, given: Option<&str>) -> ProblemResponse {
    let message = match side {
        "from" => "`from` must be a Snapshot's identity",
        _ => "`to` must be a Snapshot's identity or `working`",
    };
    let problem = ProblemResponse::new(
        StatusCode::BAD_REQUEST,
        ErrorCode::ComparisonInvalid,
        message,
    );

    match given {
        Some(given) => problem.with_detail(side, given),
        None => problem,
    }
}

/// One file's part of a diff as the wire carries it.
fn wire(file: FileDiff) -> DiffFile {
    DiffFile {
        path: file.path.as_str().to_owned(),
        old_path: file.old_path.map(|path| path.as_str().to_owned()),
        change: file.change.map(|change| match change {
            keepster_store::FileChange::Added => FileChange::Added,
            keepster_store::FileChange::Modified => FileChange::Modified,
            keepster_store::FileChange::Deleted => FileChange::Deleted,
            keepster_store::FileChange::Renamed => FileChange::Renamed,
        }),
        binary: file.binary,
        too_large: file.too_large,
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
