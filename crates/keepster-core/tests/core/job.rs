//! A job's life (ticket 54): queued, then running, then succeeded or failed — once, in that order, so a
//! client polling or following the stream never sees a job go back.

use keepster_core::{JobState, JobTransition};

#[test]
fn a_job_runs_once_and_ends_once() {
    let queued = JobState::Queued;
    let running = queued.advance(JobState::Running).expect("it starts");

    assert_eq!(running, JobState::Running);
    assert_eq!(
        running.advance(JobState::Succeeded),
        Ok(JobState::Succeeded)
    );
    assert_eq!(running.advance(JobState::Failed), Ok(JobState::Failed));
    assert!(JobState::Succeeded.is_finished() && JobState::Failed.is_finished());
    assert!(!queued.is_finished() && !running.is_finished());
}

#[test]
fn a_job_never_goes_back_or_skips_running() {
    for (from, to) in [
        (JobState::Queued, JobState::Succeeded),
        (JobState::Queued, JobState::Queued),
        (JobState::Running, JobState::Queued),
        (JobState::Running, JobState::Running),
        (JobState::Succeeded, JobState::Failed),
        (JobState::Failed, JobState::Running),
    ] {
        assert_eq!(
            from.advance(to),
            Err(JobTransition { from, to }),
            "{from:?} → {to:?}"
        );
    }
    // A job that cannot even start — its Root gone while it waited — fails from the queue.
    assert_eq!(
        JobState::Queued.advance(JobState::Failed),
        Ok(JobState::Failed)
    );
}

#[test]
fn a_state_is_named_as_the_wire_names_it() {
    for (state, name) in [
        (JobState::Queued, "queued"),
        (JobState::Running, "running"),
        (JobState::Succeeded, "succeeded"),
        (JobState::Failed, "failed"),
    ] {
        assert_eq!(state.as_str(), name);
    }
}
