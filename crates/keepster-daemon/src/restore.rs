//! Restore at the API (ticket 47): the request read, the Root locked, and the plan or the outcome said
//! for every path.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use keepster_api::{
    Confirm, ErrorCode, Event, ModesChoice, NameCaseChoice, PathResolution, Problem, RestoreIssue,
    RestoreOutcome, RestoreRequest, Restored, RestoredPath,
};
use keepster_app::restore::{self, RestoreError};
use keepster_core::VolumeVerdict;
use keepster_core::{
    Confirmation, Confirmed, Issue, ModeChoice, NameCase, PathPlan, Plan, Resolution, RootPath,
    SnapshotId, Step,
};
use keepster_store::Written;

use crate::DaemonState;
use crate::problem::ProblemResponse;
use crate::roots::{self, RootAddress};
use crate::snapshots::{announce, capture_problem, prepare, store_problem};

/// Put a Snapshot's versions back over the Working Tree.
#[utoipa::path(
    post,
    path = "/v1/roots/{id}/restore",
    operation_id = "restore_root",
    tag = "snapshots",
    params(RootAddress),
    request_body = RestoreRequest,
    responses(
        (status = 200, description = "Every file of the request, in path order, with what happened to it — or, for a dry run, what would — and the Safety Snapshot that undoes it. Restore overwrites and creates; it never deletes, so a file it created stays after an undo. Each file is written to a temporary name beside it and renamed into place, with the Snapshot's permission bits — or, where those on disk differ, the ones `modes` chose; a file whose content already matches has only its bits set. Windows keeps no permission bits, and sets none. A file that changed between the plan and its write fails without being touched.", body = Restored),
        (status = 400, description = "`confirmation_required`: some file needs a confirmation (`create_parents`, `replace_type`, `break_hardlinks`, `overwrite_unrecoverable`, or `\"all\"` for every one) or a resolution (`write`, `rename_to`, `name_case`, `modes`, which the blanket force never gives — `modes` defaults to the Root's `restore.modes` setting, which by default asks), or cannot be restored (`type_conflict`, `link_check_failed`). Nothing was written and no Safety Snapshot taken; `paths` says what each file needs. Otherwise a malformed path or resolution.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root, Snapshot or path in the Snapshot — or a path asked for by name is Withheld there (`withheld`), so there is nothing to restore.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "The Root is unavailable, or its configuration has never been valid.", body = Problem, content_type = "application/problem+json"),
        (status = 415, description = "The body is not declared as `application/json`.", body = Problem, content_type = "application/problem+json"),
        (status = 422, description = "The body is JSON of the wrong shape.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The history could not be read, or the Safety Snapshot could not be taken — in which case nothing was written.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn restore(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
    body: Result<Json<RestoreRequest>, JsonRejection>,
) -> Result<Json<Restored>, ProblemResponse> {
    let Json(request) = body.map_err(|rejection| {
        ProblemResponse::new(
            rejection.status(),
            ErrorCode::BodyInvalid,
            rejection.body_text(),
        )
    })?;
    let id = roots::parse_id(&id)?;
    let modes = request.modes;
    let mut request = parsed(request)?;
    let (root, location, loaded) = prepare(&state, id)?;
    // The request's own answer about modes, or the Root's standing one; neither leaves it a question.
    if let Some(choice) = modes.map(mode_choice).or(loaded.restore_modes) {
        request.confirmed = std::mem::take(&mut request.confirmed).with_modes(choice);
    }
    let now = roots::now();
    let dry_run = request.dry_run;

    let restored = tokio::task::spawn_blocking({
        let state = state.clone();
        move || {
            // Under the store's lock, as a capture is taken: the Safety Snapshot is one, and nothing else
            // writes into this store while the files are put back.
            let store = state.store_lock(id);
            let _held = store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state.registry().contains(id) {
                return Err(roots::not_found(&id.to_string()));
            }
            // Followed first, holding the store as a capture does, if its volume is mounted somewhere
            // else now (ADR 0011).
            let root = crate::volumes::follow(&state, root);
            // The Safety Snapshot is a capture: its rules are read holding the store, so a Forget this
            // waited behind cannot have excluded a path it then takes back (ticket 81).
            let loaded = state.effective(id, &root.path)?;
            if !root.path.is_dir() {
                return Err(ProblemResponse::new(
                    StatusCode::CONFLICT,
                    ErrorCode::RootUnavailable,
                    format!("{} is not there to restore into", root.path.display()),
                )
                .with_detail("path", root.path.to_string_lossy()));
            }
            // Nothing is written into a tree that is not the one registered (ADR 0011).
            if crate::volumes::verdict(&*state.volumes, &root) == VolumeVerdict::Elsewhere {
                return Err(crate::volumes::elsewhere(&root.path));
            }

            let restored = restore::restore(&root, &location, &loaded.policy, &request, now)
                .map_err(restore_problem)?;
            if let Some(captured) = &restored.captured {
                announce(&state, id, captured);
            }

            Ok(restored)
        }
    })
    .await
    .map_err(|joined| {
        ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::HistoryUnreadable,
            joined.to_string(),
        )
    })??;

    let paths = wire(&restored.plan, restored.written.as_deref());
    if let (false, Some(safety)) = (dry_run, restored.safety_snapshot) {
        let count = |outcome: RestoreOutcome| {
            paths.iter().filter(|path| path.outcome == outcome).count() as u64
        };
        state.publish(Event::RestoreCompleted {
            root: id.to_string(),
            safety_snapshot: safety.to_string(),
            restored: count(RestoreOutcome::Restored),
            created: count(RestoreOutcome::Created),
            failed: count(RestoreOutcome::Failed),
        });
    }

    Ok(Json(Restored {
        safety_snapshot: restored.safety_snapshot.map(|id| id.to_string()),
        dry_run,
        paths,
    }))
}

/// The request in the domain's terms, or the problem with it.
fn parsed(request: RestoreRequest) -> Result<restore::RestoreRequest, ProblemResponse> {
    let from = request
        .from
        .parse::<SnapshotId>()
        .map_err(|_| crate::snapshots::snapshot_not_found(&request.from))?;
    let paths = crate::wire::requested(&request.paths, &request.paths_bytes)?;

    Ok(restore::RestoreRequest {
        from,
        paths,
        confirmed: confirmed_by(request.confirm),
        resolutions: resolutions_of(&request.resolutions, &request.resolutions_bytes)?,
        skip_unresolvable: request.skip_unresolvable,
        dry_run: request.dry_run,
        link_names: link_names(),
    })
}

/// The confirmations a request gives, in the domain's terms.
pub(crate) fn confirmed_by(confirm: Confirm) -> Confirmed {
    match confirm {
        Confirm::All(_) => Confirmed::all(),
        Confirm::These(these) => {
            Confirmed::these(these.into_iter().map(|confirmation| match confirmation {
                keepster_api::Confirmation::CreateParents => Confirmation::CreateParents,
                keepster_api::Confirmation::ReplaceType => Confirmation::ReplaceType,
                keepster_api::Confirmation::BreakHardlinks => Confirmation::BreakHardlinks,
                keepster_api::Confirmation::OverwriteUnrecoverable => {
                    Confirmation::OverwriteUnrecoverable
                }
                keepster_api::Confirmation::Overwrite => Confirmation::Overwrite,
                keepster_api::Confirmation::DestInRoot => Confirmation::DestInRoot,
            }))
        }
    }
}

/// A request's answers, by the Snapshot path each is for — keyed by text in `text`, by bytes in
/// `bytes` (ticket 74) — or the problem with one.
pub(crate) fn resolutions_of(
    text: &BTreeMap<String, PathResolution>,
    bytes: &BTreeMap<String, PathResolution>,
) -> Result<BTreeMap<RootPath, Resolution>, ProblemResponse> {
    crate::wire::resolutions(text, bytes)?
        .into_iter()
        .map(|(path, answer)| {
            let resolution = resolution(&path, answer)?;
            Ok((path, resolution))
        })
        .collect()
}

/// How this platform lists every name of a file: on Windows through `keepster-service`, which holds the
/// workspace's `unsafe`; elsewhere the store finds them by `(dev, ino)`.
fn link_names() -> Option<keepster_store::LinkNames> {
    #[cfg(windows)]
    {
        Some(keepster_service::hard_link_names)
    }

    #[cfg(not(windows))]
    {
        None
    }
}

/// One path's answer, which must answer exactly one question.
fn resolution(path: &RootPath, answer: PathResolution) -> Result<Resolution, ProblemResponse> {
    match answer {
        PathResolution {
            write: Some(true),
            rename_to: None,
            name_case: None,
            modes: None,
        } => Ok(Resolution::Write),
        PathResolution {
            write: None,
            rename_to: Some(name),
            name_case: None,
            modes: None,
        } => Ok(Resolution::RenameTo(name)),
        PathResolution {
            write: None,
            rename_to: None,
            name_case: Some(choice),
            modes: None,
        } => Ok(Resolution::NameCase(match choice {
            NameCaseChoice::KeepExisting => NameCase::KeepExisting,
            NameCaseChoice::UseSnapshot => NameCase::UseSnapshot,
        })),
        PathResolution {
            write: None,
            rename_to: None,
            name_case: None,
            modes: Some(choice),
        } => Ok(Resolution::Modes(mode_choice(choice))),
        _ => Err(resolution_invalid(
            path,
            "a resolution answers exactly one question: `write`, `rename_to`, `name_case` or `modes`",
        )),
    }
}

/// A modes answer in the domain's terms.
pub(crate) fn mode_choice(choice: ModesChoice) -> ModeChoice {
    match choice {
        ModesChoice::Snapshot => ModeChoice::Snapshot,
        ModesChoice::Keep => ModeChoice::Keep,
    }
}

/// The problem with one path's answer.
pub(crate) fn resolution_invalid(path: &RootPath, message: &str) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::BAD_REQUEST,
        ErrorCode::ResolutionInvalid,
        message.to_owned(),
    )
    .with_path(path.as_bytes())
}

fn restore_problem(error: RestoreError) -> ProblemResponse {
    match error {
        RestoreError::Blocked(plan) => ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::ConfirmationRequired,
            "some files need a confirmation or a resolution, or cannot be restored; nothing was written",
        )
        .with_paths(wire(&plan, None)),
        RestoreError::RenameInvalid { ref path, reason } => {
            resolution_invalid(path, &format!("`rename_to` must be a bare name: {reason}"))
        }
        RestoreError::Capture(error) => capture_problem(error),
        RestoreError::Store(error) => store_problem(error),
    }
}

/// Every path as the wire says it: what happened, from `written`, or what is planned.
pub(crate) fn wire(plan: &Plan, written: Option<&[Written]>) -> Vec<RestoredPath> {
    plan.paths
        .iter()
        .enumerate()
        .map(|(position, planned)| {
            let (outcome, error) = match written.and_then(|written| written.get(position)) {
                Some(Written::Restored) => (RestoreOutcome::Restored, None),
                Some(Written::Created) => (RestoreOutcome::Created, None),
                Some(Written::Unchanged) => (RestoreOutcome::Unchanged, None),
                Some(Written::Skipped) => (RestoreOutcome::Skipped, None),
                Some(Written::Failed(error)) => (RestoreOutcome::Failed, Some(error.clone())),
                None => (planned_outcome(planned), None),
            };
            let moved = (planned.target != planned.path).then_some(&planned.target);

            RestoredPath {
                path: crate::wire::text(&planned.path),
                path_bytes: crate::wire::companion(&planned.path),
                target: moved.map(crate::wire::text),
                target_bytes: moved.and_then(crate::wire::companion),
                outcome,
                issues: planned.issues.iter().map(issue).collect(),
                error,
            }
        })
        .collect()
}

fn planned_outcome(planned: &PathPlan) -> RestoreOutcome {
    match planned.step {
        Step::Restore => RestoreOutcome::Restored,
        Step::Create => RestoreOutcome::Created,
        Step::Unchanged => RestoreOutcome::Unchanged,
        Step::Skipped => RestoreOutcome::Skipped,
        Step::Blocked => RestoreOutcome::Blocked,
    }
}

/// One reason, with what settles it and the particulars a client shows.
fn issue(issue: &Issue) -> RestoreIssue {
    let mut wire = RestoreIssue {
        reason: issue.code().to_owned(),
        confirm: issue
            .confirmation()
            .map(|confirmation| confirmation.as_str().to_owned()),
        resolution: issue.resolution().map(ToOwned::to_owned),
        at: None,
        at_bytes: None,
        paths: Vec::new(),
        paths_bytes: Vec::new(),
        links_outside: None,
        existing: None,
        existing_bytes: None,
        why: None,
        snapshot_mode: None,
        current_mode: None,
    };
    match issue {
        Issue::TypeChange { at } | Issue::LinkCheckFailed { at } => {
            wire.at = Some(crate::wire::text(at));
            wire.at_bytes = crate::wire::companion(at);
        }
        Issue::Hardlinked { inside, outside } => {
            wire.paths = crate::wire::texts(inside);
            wire.paths_bytes = crate::wire::companions(inside);
            wire.links_outside = Some(*outside);
        }
        Issue::OverwriteUnrecoverable { why } => wire.why = Some(why.as_str().to_owned()),
        Issue::Withheld { reason } => wire.why = Some(reason.as_str().to_owned()),
        Issue::NameCollision { with } => {
            wire.paths = crate::wire::texts(with);
            wire.paths_bytes = crate::wire::companions(with);
        }
        Issue::NameCase { existing } => {
            wire.existing = Some(keepster_api::path_text(existing));
            wire.existing_bytes = keepster_api::path_bytes(existing);
        }
        Issue::ModesDiffer { snapshot, current } => {
            wire.snapshot_mode = Some(*snapshot);
            wire.current_mode = Some(*current);
        }
        Issue::ParentMissing
        | Issue::NameInvalid
        | Issue::TypeConflict
        | Issue::Overwrite
        | Issue::DestInRoot
        | Issue::OtherRoot => {}
    }

    wire
}
