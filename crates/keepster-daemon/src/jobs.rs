//! Jobs (ticket 54, ticket 07): a long or irreversible operation accepted with `202` and an identity, run
//! behind its store's lock, announced as it starts and ends, and reported by `GET /v1/jobs/{id}` — so a
//! client whose connection dropped still learns how it ended.
//!
//! Jobs live in memory only: a restart forgets them, and `GET` then answers `job_not_found`. What a job
//! does is idempotent — a Maintenance Pass, Forget (ticket 81) — so asking again is always safe.

use std::collections::VecDeque;
use std::sync::Mutex;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use keepster_api::{
    ErrorCode, Event, ForgetRequest, Job, JobAccepted, JobProgress, JobResult, Problem,
};
use keepster_app::snapshots::{self, StoreLocation};
use keepster_core::{JobId, JobState, Level, RootId, RootPath, Rule, RuleOrigin};

use crate::DaemonState;
use crate::problem::ProblemResponse;
use crate::roots::RootAddress;
use crate::snapshots::located;

/// How many finished jobs are remembered; the oldest finished one is forgotten first, and a job that
/// has not finished never is.
const REMEMBERED: usize = 256;

/// Every job the daemon remembers. Its lock is the innermost of all: taken with at most a store's lock
/// held, and never while another is taken.
#[derive(Debug, Default)]
pub(crate) struct Jobs {
    table: Mutex<VecDeque<Record>>,
}

/// One job as the daemon holds it.
#[derive(Debug, Clone)]
struct Record {
    id: JobId,
    kind: &'static str,
    /// What was asked of a job of its kind, beyond its Root: a Forget's glob and choice, say.
    request: String,
    root: RootId,
    state: JobState,
    progress: Option<JobProgress>,
    result: Option<JobResult>,
    error: Option<Problem>,
}

impl Jobs {
    /// The job of `kind` asking `request` of `root` still waiting in the queue, if there is one — or a
    /// new one, queued as `id`. Says which, so that asking the same twice while a job waits starts one
    /// job, not two: the second would do nothing the first will not, and each waits on a thread of its
    /// own.
    fn queue_or_join(
        &self,
        id: JobId,
        kind: &'static str,
        request: String,
        root: RootId,
    ) -> (JobId, bool) {
        let mut table = self.table();
        if let Some(waiting) = table.iter().find(|record| {
            record.kind == kind
                && record.request == request
                && record.root == root
                && record.state == JobState::Queued
        }) {
            return (waiting.id, false);
        }
        table.push_back(Record {
            id,
            kind,
            request,
            root,
            state: JobState::Queued,
            progress: None,
            result: None,
            error: None,
        });
        while table.len() > REMEMBERED {
            let Some(oldest) = table.iter().position(|record| record.state.is_finished()) else {
                break;
            };
            table.remove(oldest);
        }

        (id, true)
    }

    /// Move the job `id` on to `to`, with what it did or why it failed. A step its state does not allow
    /// is a bug here, logged and not taken.
    fn advance(&self, id: JobId, to: JobState, result: Option<JobResult>, error: Option<Problem>) {
        let mut table = self.table();
        let Some(record) = table.iter_mut().find(|record| record.id == id) else {
            return;
        };
        match record.state.advance(to) {
            Ok(state) => {
                record.state = state;
                record.result = result;
                record.error = error;
            }
            Err(transition) => {
                tracing::error!(job = %id, %transition, "a job was moved out of order")
            }
        }
    }

    /// The job `id` got further: `done` of `total` steps.
    fn progress(&self, id: JobId, done: u64, total: u64) {
        if let Some(record) = self.table().iter_mut().find(|record| record.id == id) {
            record.progress = Some(JobProgress { done, total });
        }
    }

    /// The job `id` as the API reports it.
    fn report(&self, id: JobId) -> Option<Job> {
        self.table()
            .iter()
            .find(|record| record.id == id)
            .map(|record| Job {
                id: record.id.to_string(),
                kind: record.kind.to_owned(),
                root: record.root.to_string(),
                state: wire(record.state),
                progress: record.progress,
                result: record.result.clone(),
                error: record.error.clone(),
            })
    }

    /// The table, held for one look or one change.
    fn table(&self) -> std::sync::MutexGuard<'_, VecDeque<Record>> {
        self.table
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Run a Maintenance Pass over the Root now, as a job: what is otherwise done daily, at a quiet moment,
/// on request — after a Retention that freed much, say.
#[utoipa::path(
    post,
    path = "/v1/roots/{id}/maintenance",
    operation_id = "maintain_root",
    tag = "jobs",
    params(RootAddress),
    responses(
        (status = 202, description = "The pass is queued behind any capture of the Root in flight — or, asked for again while one still waits, that same job; `Location` and the body name it.", body = JobAccepted,
            headers(("Location" = String, description = "The job, at `/v1/jobs/{id}`."))),
        (status = 404, description = "No such Root is watched, or kept since it was unwatched.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "No `git` a pass can run with is usable, or the Root is unavailable.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn maintain(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
) -> Result<impl IntoResponse, ProblemResponse> {
    let (id, location) = located(&state, &id)?;
    // Refused at the door rather than failed later: `status` already says why and how to fix it.
    if crate::git::current(&state).is_none() {
        return Err(ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::MaintenanceUnavailable,
            "no `git` a Maintenance Pass can run with is usable; `keepster status` says why",
        ));
    }

    let (job, new) = state.jobs.queue_or_join(
        JobId::generate(crate::roots::now()),
        MAINTENANCE,
        String::new(),
        id,
    );
    if new {
        let running = state.clone();
        tokio::task::spawn_blocking(move || {
            run(&running, job, MAINTENANCE, id, |state| {
                maintenance(state, id, &location)
            });
        });
    }

    Ok(accepted(job))
}

/// The answer to a job just accepted: `202`, and where it is.
fn accepted(job: JobId) -> impl IntoResponse {
    (
        StatusCode::ACCEPTED,
        [(header::LOCATION, format!("/v1/jobs/{job}"))],
        Json(JobAccepted {
            job_id: job.to_string(),
        }),
    )
}

/// What a manual Maintenance Pass job is called.
const MAINTENANCE: &str = "maintenance";

/// A Maintenance Pass over the Root `id` now, its failures as the API reports them: a `git` that is not
/// usable as `status` says, and a history or a pass that failed as what went wrong.
fn maintenance(
    state: &DaemonState,
    id: RootId,
    location: &keepster_app::snapshots::StoreLocation,
) -> Result<JobResult, ProblemResponse> {
    let reclaimed = pass(state, id, location)?;

    Ok(JobResult {
        reclaimed_bytes: Some(reclaimed),
        ..JobResult::default()
    })
}

/// A Maintenance Pass over the Root `id` now, and how many bytes it reclaimed — its failures as the API
/// reports them: a `git` that is not usable as `status` says, and a history or a pass that failed as
/// what went wrong.
fn pass(state: &DaemonState, id: RootId, location: &StoreLocation) -> Result<u64, ProblemResponse> {
    use crate::housekeeping::MaintenanceError;

    let unavailable = |detail: String| {
        ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::MaintenanceUnavailable,
            detail,
        )
    };
    let Some((generation, git)) = crate::git::current(state) else {
        return Err(unavailable(
            "the `git` this pass was to run with is no longer usable".to_owned(),
        ));
    };

    match crate::housekeeping::maintain(state, id, location, generation, git, crate::roots::now()) {
        Ok(maintained) => Ok(maintained.reclaimed_bytes),
        Err(MaintenanceError::Unavailable(error)) => Err(unavailable(error.to_string())),
        Err(MaintenanceError::Failed(error)) => Err(ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::MaintenanceFailed,
            error.to_string(),
        )),
        Err(MaintenanceError::History(error)) => Err(crate::snapshots::store_problem(error)),
    }
}

/// Remove every version of a path from the Root's whole history, as a job (ticket 81, ticket 06).
///
/// Irreversible, so the request must say everything itself — what, that the caller knows, and whether
/// the path is excluded from now on; the daemon never asks (ticket 07). It ends with a Maintenance Pass
/// under the same lock, which is what takes the bytes off the disk, so it is refused while there is no
/// `git` to run it with. The Working Tree is never touched, and no Safety Snapshot is taken: one would
/// capture the very thing being removed.
#[utoipa::path(
    post,
    path = "/v1/roots/{id}/forget",
    operation_id = "forget_path",
    tag = "jobs",
    params(RootAddress),
    request_body = ForgetRequest,
    responses(
        (status = 202, description = "Queued behind any capture of the Root in flight — or, asked for again while it still waits, that same job; `Location` and the body name it. Excluding from now on is in force from here.", body = JobAccepted,
            headers(("Location" = String, description = "The job, at `/v1/jobs/{id}`."))),
        (status = 400, description = "Something the request must carry is missing (`confirmation_required`, `details.missing` naming it), or the glob is not one Forget can remove.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root is watched, or kept since it was unwatched.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "Excluding a path on the secret opt-in list, or no `git` to end with a Maintenance Pass.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn forget(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
    body: Result<Json<ForgetRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ProblemResponse> {
    let (id, location) = located(&state, &id)?;
    let Json(request) = body.map_err(|rejection| {
        ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::BodyInvalid,
            rejection.body_text(),
        )
    })?;

    let exclude = match request.on_exclude.as_deref() {
        Some("exclude") => Some(true),
        Some("keep-watching") => Some(false),
        _ => None,
    };
    let missing: Vec<&str> = [
        ("glob", request.glob.is_none()),
        ("confirm", request.confirm != Some(true)),
        ("on_exclude", exclude.is_none()),
    ]
    .into_iter()
    .filter_map(|(field, missing)| missing.then_some(field))
    .collect();
    let (Some(glob), Some(exclude), true) = (request.glob.clone(), exclude, missing.is_empty())
    else {
        return Err(ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::ConfirmationRequired,
            "Forget is irreversible: the request must carry `glob`, `confirm: true` and `on_exclude` \
             (`exclude` or `keep-watching`)",
        )
        .with_detail("missing", missing.join(",")));
    };
    let rule = Rule::parse(
        &glob,
        RootPath::root(),
        RuleOrigin {
            level: Level::Forget,
            source: "keepster forget".to_owned(),
            line: None,
        },
    )
    .ok()
    .filter(|rule| !rule.negated)
    .ok_or_else(|| {
        ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::GlobInvalid,
            "`glob` must be a pattern naming what to remove, and not a negation",
        )
        .with_detail("glob", glob.as_str())
    })?;
    if crate::git::current(&state).is_none() {
        return Err(ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::MaintenanceUnavailable,
            "Forget ends with a Maintenance Pass, and no `git` one can run with is usable; \
             `keepster status` says why",
        ));
    }

    let working_tree = root_path(&state, id);
    // Asked here so the answer is immediate, and again by the job under the store's lock, where nothing
    // can be captured in between.
    if exclude {
        let (checking, path, location, rule) = (
            state.clone(),
            working_tree.clone(),
            location.clone(),
            rule.clone(),
        );
        tokio::task::spawn_blocking(move || {
            refuse_opted_in(&checking, id, path.as_deref(), &location, &rule)
        })
        .await
        .map_err(|joined| {
            ProblemResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::HistoryUnreadable,
                joined.to_string(),
            )
        })??;
    }

    let (job, new) = state.jobs.queue_or_join(
        JobId::generate(crate::roots::now()),
        FORGET,
        format!("{glob}\n{exclude}"),
        id,
    );
    if new {
        let running = state.clone();
        tokio::task::spawn_blocking(move || {
            run(&running, job, FORGET, id, |state| {
                forgetting(
                    state,
                    job,
                    id,
                    &location,
                    (&rule, exclude),
                    working_tree.as_deref(),
                )
            });
        });
    }

    Ok(accepted(job))
}

/// What a Forget job is called.
const FORGET: &str = "forget";

/// Refuse to exclude what the user opted in past the denylist: they have said those secrets are to be
/// captured (ticket 06). Decided on the paths the glob matches in the history, not on how two patterns
/// compare, which in general cannot be known — and for a Root no longer watched too, whose exclusion
/// would be in force the day it is watched again.
fn refuse_opted_in(
    state: &DaemonState,
    id: RootId,
    working_tree: Option<&std::path::Path>,
    location: &StoreLocation,
    rule: &Rule,
) -> Result<(), ProblemResponse> {
    let Some(working_tree) = working_tree else {
        return Ok(());
    };
    let policy = match state.effective(id, working_tree) {
        Ok(effective) => effective.policy,
        // A Root no longer watched whose configuration cannot be read opts nothing in that can be known.
        Err(_) if !state.registry().contains(id) => return Ok(()),
        Err(problem) => return Err(problem),
    };
    let matched =
        snapshots::forgettable(location, rule).map_err(crate::snapshots::store_problem)?;
    match matched.iter().find(|path| policy.opted_in(path).is_some()) {
        Some(path) => Err(ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::SecretOptedIn,
            format!(
                "`{path}` is on the secret opt-in list, which says it is to be captured; \
                 forget it and keep watching, or take it off the list first"
            ),
        )
        .with_detail("path", path.as_str())),
        None => Ok(()),
    }
}

/// Where the Root `id`'s directory is, watched or no longer watched.
fn root_path(state: &DaemonState, id: RootId) -> Option<std::path::PathBuf> {
    let registry = state.registry();
    let watched = registry
        .readable()
        .ok()
        .and_then(|records| records.into_iter().find(|record| record.id == id));
    let record = match watched {
        Some(record) => Some(record),
        None => registry
            .unwatched()
            .ok()
            .and_then(|records| records.into_iter().find(|record| record.id == id)),
    };

    record.map(|record| record.path)
}

/// The Forget itself, under the store's lock: the exclusion from now on, if it was asked for — only once
/// nothing opted in stands in its way, and only by a job that runs — then the rewrite, announced a
/// Segment or Pin at a time, then the Maintenance Pass that takes what it left unreachable off the
/// disk, its last step.
fn forgetting(
    state: &DaemonState,
    job: JobId,
    id: RootId,
    location: &StoreLocation,
    (rule, exclude): (&Rule, bool),
    working_tree: Option<&std::path::Path>,
) -> Result<JobResult, ProblemResponse> {
    if exclude {
        refuse_opted_in(state, id, working_tree, location, rule)?;
        state
            .config()
            .forget_from_now_on(id, &rule.text)
            .map_err(|error| crate::problem::invalid_file(&error))?;
    }
    let announce = |done: u64, total: u64| {
        state.jobs.progress(job, done, total);
        state.publish(Event::JobProgress {
            job: job.to_string(),
            done,
            total,
        });
    };
    let mut steps = 1;
    let forgotten = snapshots::forget(location, rule, working_tree, &mut |done, total| {
        steps = total + 1;
        announce(done, steps);
    })
    .map_err(crate::snapshots::store_problem)?;
    let reclaimed = pass(state, id, location)?;
    announce(steps, steps);
    if !forgotten.still_on_disk.is_empty() {
        tracing::warn!(root = %id, paths = ?forgotten.still_on_disk, "forgotten, and still on disk");
    }

    Ok(JobResult {
        reclaimed_bytes: Some(reclaimed),
        snapshots_touched: Some(forgotten.snapshots_touched),
        blobs_removed: Some(forgotten.blobs_removed),
        removed_paths: forgotten
            .paths
            .iter()
            .map(|path| path.as_str().to_owned())
            .collect(),
        still_on_disk: forgotten
            .still_on_disk
            .iter()
            .map(|path| path.as_str().to_owned())
            .collect(),
        excluded: Some(exclude),
        left_alone: forgotten.left_alone,
    })
}

/// Report a job: where it is, and once it has ended, what it did or why it failed.
#[utoipa::path(
    get,
    path = "/v1/jobs/{id}",
    operation_id = "get_job",
    tag = "jobs",
    params(("id" = String, Path, description = "The job's identity, as `202` answered it.")),
    responses(
        (status = 200, description = "The job.", body = Job),
        (status = 404, description = "No such job: none was started, or the daemon restarted since.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn show(
    State(state): State<DaemonState>,
    Path(id): Path<String>,
) -> Result<Json<Job>, ProblemResponse> {
    let not_found = || {
        ProblemResponse::new(
            StatusCode::NOT_FOUND,
            ErrorCode::JobNotFound,
            format!("no job `{id}` is known"),
        )
        .with_detail("job", id.as_str())
    };
    let job = id.parse::<JobId>().map_err(|_| not_found())?;

    state.jobs.report(job).map(Json).ok_or_else(not_found)
}

/// Run the job `id` of `kind` on `root`: wait for the store as a capture does, then do `work` and say
/// how it ended. Called off the async runtime.
fn run<F>(state: &DaemonState, id: JobId, kind: &'static str, root: RootId, work: F)
where
    F: FnOnce(&DaemonState) -> Result<JobResult, ProblemResponse>,
{
    let store = state.store_lock(root);
    let held = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let refused = if state.housekeeping.is_stopping() {
        Some(ProblemResponse::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::JobInterrupted,
            "the daemon stopped before the job could start",
        ))
    } else if !state.registry().holds(root) {
        Some(crate::roots::not_found(&root.to_string()))
    } else {
        None
    };
    if let Some(refused) = refused {
        drop(held);
        state
            .jobs
            .advance(id, JobState::Failed, None, Some(refused.into_problem()));
        announce_end(state, id, kind, root, JobState::Failed);
        return;
    }

    state.jobs.advance(id, JobState::Running, None, None);
    state.publish(Event::JobStarted {
        job: id.to_string(),
        kind: kind.to_owned(),
        root: root.to_string(),
    });
    // A job that panicked has still ended, and must say so rather than run for ever.
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(state)))
        .unwrap_or_else(|_| {
            Err(ProblemResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::HistoryUnreadable,
                "the job stopped on an internal error",
            ))
        });
    drop(held);

    let to = match ended {
        Ok(result) => {
            state
                .jobs
                .advance(id, JobState::Succeeded, Some(result), None);
            JobState::Succeeded
        }
        Err(problem) => {
            state
                .jobs
                .advance(id, JobState::Failed, None, Some(problem.into_problem()));
            JobState::Failed
        }
    };
    announce_end(state, id, kind, root, to);
}

/// Say on the stream that the job `id` ended, and how.
fn announce_end(state: &DaemonState, id: JobId, kind: &'static str, root: RootId, to: JobState) {
    state.publish(Event::JobCompleted {
        job: id.to_string(),
        kind: kind.to_owned(),
        root: root.to_string(),
        state: wire(to),
    });
}

/// A job's state as the wire spells it.
fn wire(state: JobState) -> keepster_api::JobState {
    match state {
        JobState::Queued => keepster_api::JobState::Queued,
        JobState::Running => keepster_api::JobState::Running,
        JobState::Succeeded => keepster_api::JobState::Succeeded,
        JobState::Failed => keepster_api::JobState::Failed,
    }
}
