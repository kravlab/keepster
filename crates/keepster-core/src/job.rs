//! A job (ticket 54, ticket 07): a long or irreversible operation with an identity of its own, so that a
//! client whose connection dropped can still learn how it ended.

use crate::identity::ulid_identity;

ulid_identity! {
    /// One job's identity: what `POST` answers with and `GET /v1/jobs/{id}` is addressed by.
    JobId
}

/// Where a job is in its life: queued, then running, then succeeded or failed — once, in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobState {
    /// Accepted, waiting for its store — behind a capture in flight, say.
    Queued,
    /// Under way.
    Running,
    /// Done as asked.
    Succeeded,
    /// Ended without doing what was asked, with a reason.
    Failed,
}

/// A step a job cannot take: back, sideways, or past `running` to success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a job cannot go from {} to {}", from.as_str(), to.as_str())]
pub struct JobTransition {
    /// Where it was.
    pub from: JobState,
    /// Where it was asked to go.
    pub to: JobState,
}

impl JobState {
    /// The state after this one, if a job may go from here to `to`. A queued job may fail without
    /// running — its Root gone while it waited — but succeed only by running.
    pub fn advance(self, to: Self) -> Result<Self, JobTransition> {
        match (self, to) {
            (Self::Queued, Self::Running | Self::Failed)
            | (Self::Running, Self::Succeeded | Self::Failed) => Ok(to),
            _ => Err(JobTransition { from: self, to }),
        }
    }

    /// Whether the job has ended, and so will never change again.
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }

    /// The name the wire uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}
