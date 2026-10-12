//! `keepster maintain` and `keepster job` (ticket 54): a job is started, then followed off the event
//! stream until it ends — or, with `--detach`, its identity is printed and the command ends at once.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use keepster_api::{Event, Job, JobAccepted, JobState};

use crate::client::DaemonClient;
use crate::command::{JobArgs, MaintainArgs};
use crate::events::EventStream;
use crate::exit;
use crate::roots::{answered, exit_code_for};

/// How often a job is asked about once the event stream is gone.
const POLL: Duration = Duration::from_millis(500);

/// Run a Maintenance Pass over a Root now.
pub async fn maintain(args: &MaintainArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);
    let root = match crate::resolve::resolve(&client, args.root.as_deref()).await {
        Ok(root) => root,
        Err(code) => return code,
    };

    start(
        &client,
        &format!("/v1/roots/{}/maintenance", root.id),
        b"{}",
        args.detach,
    )
    .await
}

/// Report a job, as a person reads it or as the daemon answered.
pub async fn job(args: &JobArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);
    let answer = client.get(&format!("/v1/jobs/{}", args.id)).await;

    if args.json {
        return match crate::roots::body_of(answer) {
            Ok(body) => {
                let mut out = io::stdout().lock();
                match out.write_all(&body).and_then(|()| writeln!(out)) {
                    Ok(()) => exit::SUCCESS,
                    Err(_) => exit::OTHER,
                }
            }
            Err(code) => code,
        };
    }
    match answered::<Job>(answer) {
        Ok(job) => {
            println!(
                "{}  {}  {}  {}",
                job.id,
                job.kind,
                job.root,
                state_name(job.state)
            );
            ended(&job)
        }
        Err(code) => code,
    }
}

/// Start the job `url` answers with, and follow it unless `detach`.
pub(crate) async fn start(client: &DaemonClient, url: &str, body: &[u8], detach: bool) -> u8 {
    // Subscribed before the job is asked for, so its start and end cannot be missed.
    let stream = if detach {
        None
    } else {
        client.events(None).await.ok()
    };
    let accepted: JobAccepted = match answered(client.post(url, body).await) {
        Ok(accepted) => accepted,
        Err(code) => return code,
    };
    if detach {
        println!("{}", accepted.job_id);
        return exit::SUCCESS;
    }

    eprintln!("keepster: job {} queued", accepted.job_id);
    follow(client, &accepted.job_id, stream).await
}

/// Follow the job `id` until it ends, then report how.
async fn follow(client: &DaemonClient, id: &str, mut stream: Option<EventStream>) -> u8 {
    loop {
        let Some(events) = stream.as_mut() else {
            // No stream, or it went away: asking is slower and never misses the end.
            return match finished(client, id).await {
                Ok(Some(job)) => ended(&job),
                Ok(None) => {
                    tokio::time::sleep(POLL).await;
                    continue;
                }
                Err(code) => code,
            };
        };
        let event = match events.next().await {
            Ok(Some(event)) => event,
            Ok(None) | Err(_) => {
                stream = None;
                continue;
            }
        };
        // An event this build does not know, or another job's, is passed over.
        match serde_json::from_str::<Event>(&event.data) {
            Ok(Event::JobStarted { job, .. }) if job == id => eprintln!("keepster: running"),
            Ok(Event::JobProgress {
                job, done, total, ..
            }) if job == id => eprintln!("keepster: {done} of {total}"),
            Ok(Event::JobCompleted { job, .. }) if job == id => {
                return match finished(client, id).await {
                    Ok(Some(job)) => ended(&job),
                    Ok(None) => exit::OTHER,
                    Err(code) => code,
                };
            }
            // Events were lost: whether the job ended meanwhile is a question for the job itself.
            Ok(Event::Resync) => {
                if let Ok(Some(job)) = finished(client, id).await {
                    return ended(&job);
                }
            }
            _ => {}
        }
    }
}

/// The job `id`, if it has ended.
async fn finished(client: &DaemonClient, id: &str) -> Result<Option<Job>, u8> {
    let job: Job = answered(client.get(&format!("/v1/jobs/{id}")).await)?;

    Ok(matches!(job.state, JobState::Succeeded | JobState::Failed).then_some(job))
}

/// Say how a job ended, and give it its exit code: a failed job earns the code its problem does.
fn ended(job: &Job) -> u8 {
    match (job.state, &job.error) {
        (JobState::Succeeded, _) => {
            match job
                .result
                .as_ref()
                .and_then(|result| result.reclaimed_bytes)
            {
                Some(bytes) => println!("succeeded: {bytes} bytes reclaimed"),
                None => println!("succeeded"),
            }
            exit::SUCCESS
        }
        (JobState::Failed, Some(problem)) => {
            eprintln!("keepster: the job failed: {}", problem.detail);
            exit_code_for(Some(problem), problem.status)
        }
        (JobState::Failed, None) => {
            eprintln!("keepster: the job failed");
            exit::OTHER
        }
        // Still under way: nothing to say yet, and nothing failed.
        (JobState::Queued | JobState::Running, _) => exit::SUCCESS,
    }
}

/// A job's state as the wire spells it, so the two cannot disagree.
fn state_name(state: JobState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_default()
}
