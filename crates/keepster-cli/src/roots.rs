//! The Watched Root commands: putting a directory under history, listing the Roots, unwatching one
//! and recording that one has moved.

use std::io::{self, Write};
use std::path::Path;

use keepster_api::{ErrorCode, Pins, Problem, RegisterRoot, Reported, Root, Status};
use serde::de::DeserializeOwned;

use crate::client::{ClientError, DaemonClient, Response};
use crate::command::{MoveArgs, UnwatchArgs, WatchArgs};
use crate::exit;
use crate::preview::Shown;
use crate::render::{write_git, write_roots};

/// Register a directory as a Watched Root.
pub async fn watch(args: &WatchArgs, control_path: &Path) -> u8 {
    // The daemon makes the path absolute itself, but it cannot know the caller's working directory, so
    // a relative argument is resolved here — on the client, where the working directory belongs.
    let path = match std::path::absolute(&args.path) {
        Ok(path) => path,
        Err(error) => {
            eprintln!(
                "keepster: {} could not be resolved: {error}",
                args.path.display()
            );
            return exit::OTHER;
        }
    };

    let client = DaemonClient::new(control_path);
    // `--dry-run` is the preview, and watches nothing.
    if args.dry_run {
        return match crate::preview::run(&client, &path, "", Shown::Rendered(false)).await {
            Ok(_) => exit::SUCCESS,
            Err(code) => code,
        };
    }
    if !args.yes {
        if let Err(code) = within_claim(&client, &path).await {
            return code;
        }
    }

    let body = serde_json::json!({ "path": path.to_string_lossy() });
    let answer = client
        .post(
            "/v1/roots",
            &serde_json::to_vec(&body).expect("a JSON body"),
        )
        .await;

    match answered::<Root>(answer) {
        Ok(root) => {
            let code = print_roots(std::slice::from_ref(&root));
            notice_unmaintained(&client).await;
            code
        }
        Err(code) => code,
    }
}

/// Go on registering `path` only once it is known to be within what one Root is claimed to hold, or the
/// person has said to go on past it (ticket 30): nothing is refused, but a directory that big is not
/// watched by surprise. Where nothing can be asked, `--yes` says it.
///
/// A preview that cannot be had says nothing either way, and registering goes on: it will say for itself
/// whatever is wrong with the directory. One that is busy is the exception, since registering would not
/// notice: the directory is not watched unseen, and the command says to try again.
async fn within_claim(client: &DaemonClient, path: &std::path::Path) -> Result<(), u8> {
    use std::io::IsTerminal;

    if io::stderr().is_terminal() {
        eprintln!(
            "keepster: looking at what watching {} would capture…",
            path.display()
        );
    }
    let summary = match crate::preview::run(client, path, "", Shown::Quiet).await {
        Ok(summary) => summary,
        Err(exit::STATE_CONFLICT) => {
            eprintln!(
                "keepster: {} was not watched: another Capture Preview is walking, and this one must \
                 see it first; try again, or pass --yes",
                path.display()
            );
            return Err(exit::STATE_CONFLICT);
        }
        Err(_) => return Ok(()),
    };
    let keepster_api::PreviewRecord::Summary { past_ceiling, .. } = &summary else {
        return Ok(());
    };
    if past_ceiling.is_empty() {
        return Ok(());
    }

    let _ = crate::preview::render(&summary, &mut std::io::stdout().lock());
    let asked = std::io::stdin().is_terminal()
        && crate::restore::yes(&format!(
            "{} is past what one Root is claimed to hold. Watch it anyway?",
            path.display()
        ));
    if asked {
        return Ok(());
    }
    eprintln!(
        "keepster: {} was not watched; pass --yes to watch it past what one Root is claimed to hold",
        path.display()
    );

    // Waiting on the person's word, as a Restore waiting for an answer is.
    Err(exit::STATE_CONFLICT)
}

/// Say, without failing the command, that the history just started will never be reclaimed until the
/// daemon has a `git` — the notice that reaches someone who installed without a package manager to
/// declare the prerequisite (ticket 11). Whatever goes wrong asking is left to `status`.
async fn notice_unmaintained(client: &DaemonClient) {
    let Ok(response) = client.get("/v1/status").await else {
        return;
    };
    let Ok(status) = serde_json::from_slice::<Status>(&response.body) else {
        return;
    };
    if status.daemon.maintenance != Reported::Unavailable {
        return;
    }

    let mut notice = b"keepster: store maintenance is unavailable\n".to_vec();
    let shell_git = crate::status::shell_git();
    write_git(&status.daemon.git, shell_git.as_deref(), &mut notice)
        .expect("writing to a buffer in memory");
    let _ = io::stderr().lock().write_all(&notice);
}

/// List every Watched Root.
pub async fn roots(control_path: &Path) -> u8 {
    let answer = DaemonClient::new(control_path).get("/v1/roots").await;

    match answered::<Vec<Root>>(answer) {
        Ok(roots) => print_roots(&roots),
        Err(code) => code,
    }
}

/// Parse a successful answer, or report the failure and give it its exit code.
pub(crate) fn answered<T: DeserializeOwned>(
    answer: Result<Response, ClientError>,
) -> Result<T, u8> {
    let body = body_of(answer)?;

    serde_json::from_slice(&body).map_err(|error| {
        eprintln!("keepster: the daemon's answer could not be read: {error}");
        exit::OTHER
    })
}

/// Check that the daemon accepted an operation, ignoring whatever it answered with.
///
/// Separate from [`answered`] because a successful deregistration has no body at all, and parsing one
/// that does not exist would report a broken daemon where there is none.
fn accepted(answer: Result<Response, ClientError>) -> Result<(), u8> {
    body_of(answer).map(|_| ())
}

/// The body of a successful answer, or a message and an exit code.
///
/// A transport failure, a refusal carrying a Problem Details body, and a success are three different
/// outcomes with three different exit codes (ticket 07), and every command needs exactly this — so it
/// lives once.
pub(crate) fn body_of(answer: Result<Response, ClientError>) -> Result<Vec<u8>, u8> {
    let response = match answer {
        Ok(response) => response,
        // A refused stream is a refusal like any other, with the same Problem in its body.
        Err(ClientError::Refused(response)) => *response,
        Err(error) => {
            eprintln!("keepster: {error}");

            return Err(match error {
                ClientError::Unreachable { .. } => exit::DAEMON_UNREACHABLE,
                ClientError::Exchange(_)
                | ClientError::Request(_)
                | ClientError::Output(_)
                | ClientError::Refused(_) => exit::OTHER,
            });
        }
    };

    if response.status.is_success() {
        return Ok(response.body);
    }

    let problem = serde_json::from_slice::<Problem>(&response.body).ok();

    match &problem {
        Some(problem) => eprintln!("keepster: {} ({})", problem.detail, problem.code),
        None => eprintln!("keepster: the daemon answered {}", response.status),
    }

    Err(exit_code_for(problem.as_ref(), response.status.as_u16()))
}

/// The exit code a refusal deserves.
///
/// Keyed on `code` rather than on the HTTP status: ticket 07 makes the status the coarse layer and the
/// code the precise one, and the code is what a client branches on. A code this build has never heard of
/// falls back to its status, which is exactly what ADR 0005 requires of any client — the set is open and
/// grows inside `/v1`.
pub(crate) fn exit_code_for(problem: Option<&Problem>, status: u16) -> u8 {
    match problem.and_then(|problem| ErrorCode::parse(&problem.code)) {
        Some(code) => exit_code_for_known(code),
        None => match status {
            404 => exit::NOT_FOUND,
            409 => exit::STATE_CONFLICT,
            400 => exit::USAGE,
            _ => exit::OTHER,
        },
    }
}

/// The exit code each known code earns.
///
/// Exhaustive and without a wildcard on purpose (ADR 0005): a code added to `keepster-api` fails to
/// compile here until someone decides what a script should see, rather than silently becoming `1`.
fn exit_code_for_known(code: ErrorCode) -> u8 {
    match code {
        // A Withheld path has no content in that Snapshot, which is what "not found" tells a script.
        ErrorCode::RootNotFound
        | ErrorCode::SnapshotNotFound
        | ErrorCode::PathNotFound
        | ErrorCode::Withheld
        | ErrorCode::JobNotFound => exit::NOT_FOUND,
        // The same answer `status` gives for it, so a script tells it apart from any other refusal.
        ErrorCode::MaintenanceUnavailable => exit::MAINTENANCE_UNAVAILABLE,
        // A configuration that cannot be read is a state the Root is in until someone edits a file.
        // A Restore waiting for an answer is in a state only the person can move it out of.
        ErrorCode::RootOverlaps
        | ErrorCode::RootUnavailable
        | ErrorCode::ConfigInvalid
        | ErrorCode::ConfirmationRequired
        | ErrorCode::SecretOptedIn
        | ErrorCode::VolumeMismatch
        | ErrorCode::StoreUnreachable
        | ErrorCode::PreviewBusy => exit::STATE_CONFLICT,
        // Asking for the content of a directory, or the entries of a file, is a mistake in what was
        // asked rather than a state anything is in.
        ErrorCode::PathInvalid
        | ErrorCode::LimitInvalid
        | ErrorCode::CursorInvalid
        | ErrorCode::MomentInvalid
        | ErrorCode::DepthInvalid
        | ErrorCode::NotAFile
        | ErrorCode::NotADirectory
        | ErrorCode::PurgeInvalid
        | ErrorCode::ComparisonInvalid
        | ErrorCode::ResolutionInvalid
        | ErrorCode::GlobInvalid
        | ErrorCode::PreviewInvalid
        | ErrorCode::DestInStore
        | ErrorCode::QueryInvalid
        | ErrorCode::SearchInvalid => exit::USAGE,
        // The CLI builds every body and format from the contract types, so a refused one is this build's
        // fault and nothing the person typed.
        ErrorCode::BodyInvalid
        | ErrorCode::FormatInvalid
        | ErrorCode::RegistryFailed
        | ErrorCode::CaptureFailed
        | ErrorCode::ClockUnusable
        | ErrorCode::HistoryUnreadable
        | ErrorCode::MaintenanceFailed
        | ErrorCode::JobInterrupted
        | ErrorCode::PreviewFailed
        | ErrorCode::DestUnwritable
        | ErrorCode::DirectoryUnreadable => exit::OTHER,
    }
}

/// The request body both registering and moving send, built from the contract type rather than from a
/// literal, so the shape lives in `keepster-api` alone.
fn register_body(path: &std::path::Path) -> Vec<u8> {
    let request = RegisterRoot {
        path: path.to_string_lossy().into_owned(),
    };

    serde_json::to_vec(&request).expect("the request serialises")
}

fn print_roots(roots: &[Root]) -> u8 {
    let mut out = io::stdout().lock();

    match write_roots(roots, &mut out).and_then(|()| out.flush()) {
        Ok(()) => exit::SUCCESS,
        Err(error) => {
            eprintln!("keepster: could not write the answer: {error}");
            exit::OTHER
        }
    }
}

/// Deregister a Root, keeping its history unless an explicit purge says otherwise.
pub async fn unwatch(args: &UnwatchArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);

    let root = match crate::resolve::resolve(&client, args.root.as_deref()).await {
        Ok(root) => root,
        Err(code) => return code,
    };

    // Erasing history is irreversible, so selecting it and consenting to it are two separate acts —
    // one mistyped flag must not destroy the insurance (ticket 27).
    if args.purge && !args.yes {
        eprintln!(
            "keepster: --purge erases every captured version of {}; pass --yes to confirm",
            root.path
        );

        return exit::USAGE;
    }

    let url = if args.purge {
        format!("/v1/roots/{}?purge=true", root.id)
    } else {
        format!("/v1/roots/{}", root.id)
    };

    match accepted(client.delete(&url).await) {
        Err(code) => code,
        Ok(()) => {
            if args.purge {
                println!("erased the history of {}", root.path);
            } else {
                // The history decays down the ladder to its last state, pinned by the daemon, and stays
                // reachable by its identity alone (ticket 53). A Root that never captured anything has
                // neither, and is not told it has.
                let pinned = client
                    .get(&format!("/v1/roots/{}/pins", root.id))
                    .await
                    .ok()
                    .filter(|response| response.status.is_success())
                    .and_then(|response| serde_json::from_slice::<Pins>(&response.body).ok())
                    .is_some_and(|pins| !pins.snapshots.is_empty());
                if pinned {
                    println!(
                        "stopped watching {}; its history is kept, decaying to its last state, which \
                         is pinned: `keepster pins -r {}` lists it, and watching the directory again \
                         resumes it",
                        root.path, root.id
                    );
                } else {
                    println!("stopped watching {} (it had no history to keep)", root.path);
                }
            }

            exit::SUCCESS
        }
    }
}

/// Record that a Root's directory has moved.
pub async fn move_root(args: &MoveArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);

    let root = match crate::resolve::resolve(&client, args.root.as_deref()).await {
        Ok(root) => root,
        Err(code) => return code,
    };

    let path = match std::path::absolute(&args.path) {
        Ok(path) => path,
        Err(error) => {
            eprintln!(
                "keepster: {} could not be resolved: {error}",
                args.path.display()
            );

            return exit::OTHER;
        }
    };

    let answer = client
        .patch(&format!("/v1/roots/{}", root.id), &register_body(&path))
        .await;

    match answered::<Root>(answer) {
        Ok(moved) => print_roots(std::slice::from_ref(&moved)),
        Err(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use keepster_api::Problem;

    use super::exit_code_for;
    use crate::exit;

    fn problem(code: &str) -> Problem {
        Problem {
            title: "Title".to_owned(),
            status: 0,
            detail: "detail".to_owned(),
            code: code.to_owned(),
            details: BTreeMap::new(),
            paths: Vec::new(),
        }
    }

    #[test]
    fn a_known_code_decides_the_exit_code_whatever_the_status() {
        // Asking for the entries of a file is a mistake in what was asked, though the daemon answers 409.
        assert_eq!(
            exit_code_for(Some(&problem("not_a_directory")), 409),
            exit::USAGE
        );
        assert_eq!(
            exit_code_for(Some(&problem("snapshot_not_found")), 404),
            exit::NOT_FOUND
        );
        assert_eq!(
            exit_code_for(Some(&problem("root_overlaps")), 409),
            exit::STATE_CONFLICT
        );
        assert_eq!(
            exit_code_for(Some(&problem("root_unavailable")), 409),
            exit::STATE_CONFLICT
        );
        assert_eq!(
            exit_code_for(Some(&problem("depth_invalid")), 400),
            exit::USAGE
        );
        assert_eq!(
            exit_code_for(Some(&problem("history_unreadable")), 500),
            exit::OTHER
        );
    }

    #[test]
    fn an_unknown_code_is_handled_as_its_http_status() {
        // ADR 0005: the set grows inside `/v1`, so a newer daemon may send a code this build never saw.
        assert_eq!(
            exit_code_for(Some(&problem("from_the_future")), 404),
            exit::NOT_FOUND
        );
        assert_eq!(
            exit_code_for(Some(&problem("from_the_future")), 409),
            exit::STATE_CONFLICT
        );
        assert_eq!(
            exit_code_for(Some(&problem("from_the_future")), 400),
            exit::USAGE
        );
        assert_eq!(
            exit_code_for(Some(&problem("from_the_future")), 503),
            exit::OTHER
        );
        assert_eq!(exit_code_for(None, 404), exit::NOT_FOUND);
    }
}
