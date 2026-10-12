//! `keepster export`: a Snapshot's versions written into another directory by the daemon, or into one
//! zip archive here, leaving the Working Tree untouched (ticket 58, ticket 19).
//!
//! The daemon never asks, it refuses: what an Export needs settled comes back as `confirmation_required`,
//! asked here at a terminal and the request sent again, exactly as for a Restore. The write is a job,
//! followed to its end unless `--detach`.

use std::fs::File;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use keepster_api::{
    Confirm, ConfirmAll, Confirmation, ErrorCode, ExportPlan, ExportRequest, JobAccepted,
    JobResult, Problem, RestoreOutcome, RestoredPath, Root,
};

use crate::client::{ClientError, DaemonClient};
use crate::command::ExportArgs;
use crate::exit;
use crate::history::{located, selected};
use crate::restore::{Answers, Operation, ask, paths_within, report_blocked, within_root};
use crate::roots::{answered, body_of};

/// What an Export is called where the person is told what it did not do.
const EXPORT: Operation = Operation {
    past: "exported",
    verb: "export",
};

/// Export a Snapshot's versions of files.
pub async fn export(args: &ExportArgs, control_path: &Path, now: SystemTime) -> u8 {
    let client = DaemonClient::new(control_path);
    let (root, _) = match located(
        &client,
        args.root.as_deref(),
        args.paths.first().map(PathBuf::as_path),
    )
    .await
    {
        Ok(found) => found,
        Err(code) => return code,
    };
    let paths = match paths_within(&root, &args.paths) {
        Ok(paths) => paths,
        Err(code) => return code,
    };
    let from = match selected(
        &client,
        &root,
        args.from.as_deref().unwrap_or("latest"),
        now,
    )
    .await
    {
        Ok(from) => from,
        Err(code) => return code,
    };

    if let Some(file) = &args.archive {
        return archive(&client, &root, &from, &paths, file).await;
    }
    let Some(to) = &args.to else {
        eprintln!("keepster: say where to write: --to DIR, or --archive FILE");
        return exit::USAGE;
    };
    let dest = match std::path::absolute(to) {
        Ok(dest) => dest,
        Err(error) => {
            eprintln!("keepster: {} could not be resolved: {error}", to.display());
            return exit::OTHER;
        }
    };
    let within = |path: &Path| within_root(&root, path);
    let mut answers = match Answers::from_flags(&args.resolving, confirmations(args), &within) {
        Ok(answers) => answers,
        Err(code) => return code,
    };

    // Subscribed before the job is asked for, so its start and end cannot be missed.
    let mut stream = if args.detach || args.dry_run || args.json {
        None
    } else {
        client.events(None).await.ok()
    };
    let url = format!("/v1/roots/{}/export", root.id);
    let interactive = !args.resolving.no_input && !args.json && io::stdin().is_terminal();
    loop {
        let request = ExportRequest {
            from: from.clone(),
            paths: paths.clone(),
            dest: dest.to_string_lossy().into_owned(),
            dry_run: args.dry_run,
            confirm: answers.confirm.clone(),
            resolutions: answers.resolutions.clone(),
            skip_unresolvable: answers.skip_unresolvable,
            modes: answers.modes,
        };
        let body = serde_json::to_vec(&request).expect("an export request serialises");
        let answer = client.post(&url, &body).await;

        if let Ok(response) = &answer {
            if let Ok(problem) = serde_json::from_slice::<Problem>(&response.body) {
                if problem.code == ErrorCode::ConfirmationRequired.as_str() {
                    if args.json {
                        print_body(&response.body);
                        return exit::STATE_CONFLICT;
                    }
                    if interactive && ask(&mut answers, &problem.paths, EXPORT) {
                        continue;
                    }
                    report_blocked(&problem.paths, EXPORT);
                    return exit::STATE_CONFLICT;
                }
            }
        }

        if args.dry_run {
            let body = match body_of(answer) {
                Ok(body) => body,
                Err(code) => return code,
            };
            if args.json {
                print_body(&body);
                return exit::SUCCESS;
            }
            return match serde_json::from_slice::<ExportPlan>(&body) {
                Ok(plan) => {
                    report(&plan.paths, true);
                    println!(
                        "{} files, {} bytes, into {}",
                        plan.files, plan.bytes, plan.dest
                    );
                    exit::SUCCESS
                }
                Err(error) => {
                    eprintln!("keepster: the daemon's answer could not be read: {error}");
                    exit::OTHER
                }
            };
        }

        let accepted: JobAccepted = match answered(answer) {
            Ok(accepted) => accepted,
            Err(code) => return code,
        };
        if args.detach {
            println!("{}", accepted.job_id);
            return exit::SUCCESS;
        }
        if args.json {
            return match crate::jobs::finished_job(&client, &accepted.job_id).await {
                Ok(job) => {
                    print_body(&serde_json::to_vec(&job).expect("a job serialises"));
                    crate::jobs::exit_code(&job)
                }
                Err(code) => code,
            };
        }
        eprintln!("keepster: job {} queued", accepted.job_id);
        return crate::jobs::follow(&client, &accepted.job_id, stream.take()).await;
    }
}

/// The confirmations the flags give.
fn confirmations(args: &ExportArgs) -> Confirm {
    if args.force {
        return Confirm::All(ConfirmAll::All);
    }

    Confirm::These(
        [
            (args.overwrite, Confirmation::Overwrite),
            (args.replace_type, Confirmation::ReplaceType),
            (args.dest_in_root, Confirmation::DestInRoot),
            (
                args.overwrite_unrecoverable,
                Confirmation::OverwriteUnrecoverable,
            ),
        ]
        .into_iter()
        .filter_map(|(given, confirmation)| given.then_some(confirmation))
        .collect(),
    )
}

/// What an ended Export did to every file, and how to undo what it overwrote inside a Root.
pub(crate) fn report_ended(result: &JobResult) {
    report(&result.paths, false);
    if let Some(safety) = &result.safety_snapshot {
        println!(
            "Safety Snapshot {safety}: what this overwrote in the Root can be restored from it"
        );
    }
}

/// One line per file: what happened to it, or would.
fn report(paths: &[RestoredPath], dry_run: bool) {
    let mut out = io::stdout().lock();
    for path in paths {
        let verb = match (dry_run, path.outcome) {
            (true, RestoreOutcome::Restored) => "would overwrite",
            (true, RestoreOutcome::Created) => "would create",
            (true, RestoreOutcome::Skipped) => "would skip",
            (_, RestoreOutcome::Restored) => "overwrote",
            (_, RestoreOutcome::Created) => "created",
            (_, RestoreOutcome::Unchanged) => "unchanged",
            (_, RestoreOutcome::Skipped) => "skipped",
            (_, RestoreOutcome::Failed) => "failed",
            (_, RestoreOutcome::Blocked) => "blocked",
        };
        let _ = write!(out, "{verb}  {}", path.path);
        if let Some(target) = &path.target {
            let _ = write!(out, " -> {target}");
        }
        if let Some(error) = &path.error {
            let _ = write!(out, "  ({error})");
        } else if !path.issues.is_empty() {
            let reasons: Vec<String> = path
                .issues
                .iter()
                .map(|issue| match &issue.why {
                    Some(why) => format!("{}: {why}", issue.reason),
                    None => issue.reason.clone(),
                })
                .collect();
            let _ = write!(out, "  ({})", reasons.join(", "));
        }
        let _ = writeln!(out);
    }
    let _ = out.flush();
}

/// Write the selection as one zip archive to `file`, through a file beside it renamed into place only
/// once the archive is whole — so a refusal or a broken transfer leaves whatever was there.
async fn archive(
    client: &DaemonClient,
    root: &Root,
    from: &str,
    paths: &[String],
    file: &Path,
) -> u8 {
    if paths.len() > 1 {
        eprintln!("keepster: an archive holds one file or directory; name one, or none for all");
        return exit::USAGE;
    }
    let mut url = format!("/v1/roots/{}/snapshots/{from}/archive", root.id);
    if let Some(path) = paths.first() {
        url.push_str(&format!("?path={}", keepster_api::encode_path_query(path)));
    }
    let partial = crate::diff::partial_path(file);
    let mut out = match File::create(&partial) {
        Ok(out) => io::BufWriter::new(out),
        Err(error) => {
            eprintln!(
                "keepster: {} could not be written: {error}",
                partial.display()
            );
            return exit::OTHER;
        }
    };

    let answer = client.get_to(&url, &mut out).await;
    // Closed before it is renamed or removed: Windows does neither to a file still open.
    let flushed = out
        .into_inner()
        .map(drop)
        .map_err(|error| error.into_error());
    let code = match (answer, flushed) {
        (Err(ClientError::Output(error)), _) | (_, Err(error)) => {
            eprintln!(
                "keepster: {} could not be written: {error}",
                partial.display()
            );
            exit::OTHER
        }
        (answer, Ok(())) => match body_of(answer) {
            Ok(_) => exit::SUCCESS,
            Err(code) => code,
        },
    };
    if code != exit::SUCCESS {
        let _ = std::fs::remove_file(&partial);
        return code;
    }
    if let Err(error) = std::fs::rename(&partial, file) {
        eprintln!("keepster: {} could not be written: {error}", file.display());
        let _ = std::fs::remove_file(&partial);
        return exit::OTHER;
    }

    exit::SUCCESS
}

/// The daemon's answer, verbatim, ending its line.
fn print_body(body: &[u8]) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(body);
    let _ = writeln!(out);
}
