//! Jobs (ticket 54, ticket 07): a long or irreversible operation accepted with `202` and an identity, run
//! behind its store's lock, announced as it starts and ends, and reported by `GET /v1/jobs/{id}` — so a
//! client whose connection dropped still learns how it ended.
//!
//! Jobs live in memory only: a restart forgets them, and `GET` then answers `job_not_found`. What a job
//! does is idempotent — a Maintenance Pass, and Forget in ticket 81 — so asking again is always safe.

use std::collections::VecDeque;
use std::sync::Mutex;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use keepster_api::{ErrorCode, Event, Job, JobAccepted, JobResult, Problem};
use keepster_core::{JobId, JobState, RootId};

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
    root: RootId,
    state: JobState,
    result: Option<JobResult>,
    error: Option<Problem>,
}

impl Jobs {
    /// The job of `kind` on `root` still waiting in the queue, if there is one — or a new one, queued as
    /// `id`. Says which, so that asking twice while a job waits starts one job, not two: the second would
    /// do nothing the first will not, and each waits on a thread of its own.
    fn queue_or_join(&self, id: JobId, kind: &'static str, root: RootId) -> (JobId, bool) {
        let mut table = self.table();
        if let Some(waiting) = table.iter().find(|record| {
            record.kind == kind && record.root == root && record.state == JobState::Queued
        }) {
            return (waiting.id, false);
        }
        table.push_back(Record {
            id,
            kind,
            root,
            state: JobState::Queued,
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
                progress: None,
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

    let (job, new) =
        state
            .jobs
            .queue_or_join(JobId::generate(crate::roots::now()), MAINTENANCE, id);
    if new {
        let running = state.clone();
        tokio::task::spawn_blocking(move || {
            run(&running, job, MAINTENANCE, id, |state| {
                maintenance(state, id, &location)
            });
        });
    }

    Ok((
        StatusCode::ACCEPTED,
        [(header::LOCATION, format!("/v1/jobs/{job}"))],
        Json(JobAccepted {
            job_id: job.to_string(),
        }),
    ))
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
        Ok(maintained) => Ok(JobResult {
            reclaimed_bytes: Some(maintained.reclaimed_bytes),
        }),
        Err(MaintenanceError::Unavailable(error)) => Err(unavailable(error.to_string())),
        Err(MaintenanceError::Failed(error)) => Err(ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::MaintenanceFailed,
            error.to_string(),
        )),
        Err(MaintenanceError::History(error)) => Err(crate::snapshots::store_problem(error)),
    }
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
