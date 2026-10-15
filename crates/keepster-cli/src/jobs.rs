//! `keepster maintain`, `keepster forget` and `keepster job` (tickets 54 and 81): a job is started, then
//! followed off the event stream until it ends — or, with `--detach`, its identity is printed and the
//! command ends at once.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use keepster_api::{Event, Job, JobAccepted, JobState};

use crate::client::DaemonClient;
use crate::command::{ForgetArgs, JobArgs, MaintainArgs};
use crate::events::EventStream;
use crate::exit;
use crate::names::{joined, printed_all};
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

/// What a person must be told after any Forget, whatever it found: it never touches their files.
const STILL_ON_DISK: &str = "Forget never touches your files: the secret is still on disk where it was, \
     and it still needs rotating.";

/// Remove every version of a path from a Root's history.
///
/// The daemon never asks (ticket 07): what the request must carry — the confirmation, and whether the
/// path is excluded from now on — is asked here, at a terminal, or refused when nothing can be asked.
pub async fn forget(args: &ForgetArgs, control_path: &Path) -> u8 {
    use std::io::IsTerminal;

    let client = DaemonClient::new(control_path);
    let root = match crate::resolve::resolve(&client, args.root.as_deref()).await {
        Ok(root) => root,
        Err(code) => return code,
    };
    let can_ask = !args.no_input && io::stdin().is_terminal();

    let exclude = match (args.exclude, args.keep_watching) {
        (true, _) => true,
        (_, true) => false,
        _ if can_ask => {
            match crate::restore::line(
                "Keep it out of history from now on (e), or keep watching it (w)? ",
            )
            .as_str()
            {
                "e" | "exclude" => true,
                "w" | "keep-watching" => false,
                _ => {
                    eprintln!("keepster: nothing forgotten");
                    return exit::USAGE;
                }
            }
        }
        _ => {
            eprintln!(
                "keepster: say whether `{}` stays out of history from now on: --exclude or --keep-watching",
                args.glob
            );
            return exit::USAGE;
        }
    };
    let confirmed = args.yes
        || (can_ask
            && crate::restore::yes(format!(
                "Remove every version of `{}` from the history of {}? This cannot be undone.",
                args.glob, root.path
            )));
    if !confirmed {
        eprintln!(
            "keepster: forgetting cannot be undone; pass --yes to confirm{}",
            if can_ask { "" } else { " without being asked" }
        );
        return exit::USAGE;
    }

    let body = serde_json::to_vec(&keepster_api::ForgetRequest {
        glob: Some(args.glob.clone()),
        confirm: Some(true),
        on_exclude: Some(if exclude { "exclude" } else { "keep-watching" }.to_owned()),
    })
    .expect("a JSON body");
    let code = start(
        &client,
        &format!("/v1/roots/{}/forget", root.id),
        &body,
        args.detach,
    )
    .await;
    if code == exit::SUCCESS {
        eprintln!("keepster: {STILL_ON_DISK}");
    }

    code
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
pub(crate) async fn follow(client: &DaemonClient, id: &str, mut stream: Option<EventStream>) -> u8 {
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

/// The job `id` once it has ended, asked about until it has.
pub(crate) async fn finished_job(client: &DaemonClient, id: &str) -> Result<Job, u8> {
    loop {
        if let Some(job) = finished(client, id).await? {
            return Ok(job);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The exit code an ended job earns, said nowhere: a failed job earns its problem's, and an Export some
/// of whose files failed earns a partial success's.
pub(crate) fn exit_code(job: &Job) -> u8 {
    match (job.state, &job.error) {
        (JobState::Succeeded, _) => {
            let failed = job.result.as_ref().is_some_and(|result| {
                result
                    .paths
                    .iter()
                    .any(|path| path.outcome == keepster_api::RestoreOutcome::Failed)
            });
            if failed { exit::PARTIAL } else { exit::SUCCESS }
        }
        (JobState::Failed, Some(problem)) => exit_code_for(Some(problem), problem.status),
        (JobState::Failed, None) => exit::OTHER,
        (JobState::Queued | JobState::Running, _) => exit::SUCCESS,
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
            let result = job.result.clone().unwrap_or_default();
            if job.kind == "export" {
                crate::export::report_ended(&result);
                return exit_code(job);
            }
            if let Some(touched) = result.snapshots_touched {
                if touched == 0 {
                    println!("nothing in the history matched");
                } else {
                    let plural = if touched == 1 { "" } else { "s" };
                    let removed = printed_all(&result.removed_paths, &result.removed_paths_bytes);
                    let mut out = io::stdout().lock();
                    let _ = out.write_all(&[&b"forgot "[..], &joined(&removed)].concat());
                    let _ = writeln!(
                        out,
                        " from {touched} Snapshot{plural}: {} versions removed",
                        result.blobs_removed.unwrap_or(0)
                    );
                }
                for path in printed_all(&result.still_on_disk, &result.still_on_disk_bytes) {
                    let (opening, said): (&[u8], &[u8]) = if result.excluded == Some(true) {
                        (
                            b"keepster: ",
                            b" is still on disk holding what was removed; it is kept out of \
                              history from now on\n",
                        )
                    } else {
                        (
                            b"keepster: warning: ",
                            b" is still on disk holding what was removed, and still watched: \
                              the next Snapshot captures it again\n",
                        )
                    };
                    let _ = io::stderr().write_all(&[opening, &path, said].concat());
                }
                for reference in &result.left_alone {
                    eprintln!(
                        "keepster: warning: {reference} cannot be read by this build and was left as \
                         it is: whatever it holds was not forgotten"
                    );
                }
            }
            match result.reclaimed_bytes {
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
