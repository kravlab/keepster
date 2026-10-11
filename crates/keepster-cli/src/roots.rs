//! The Watched Root commands: putting a directory under history, listing the Roots, unwatching one
//! and recording that one has moved.

use std::io::{self, Write};
use std::path::Path;

use keepster_api::{ErrorCode, Problem, RegisterRoot, Root};
use serde::de::DeserializeOwned;

use crate::client::{ClientError, DaemonClient, Response};
use crate::command::{MoveArgs, UnwatchArgs, WatchArgs};
use crate::exit;
use crate::render::write_roots;

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

    let body = serde_json::json!({ "path": path.to_string_lossy() });
    let answer = DaemonClient::new(control_path)
        .post(
            "/v1/roots",
            &serde_json::to_vec(&body).expect("a JSON body"),
        )
        .await;

    match answered::<Root>(answer) {
        Ok(root) => print_roots(std::slice::from_ref(&root)),
        Err(code) => code,
    }
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
        Err(error) => {
            eprintln!("keepster: {error}");

            return Err(match error {
                ClientError::Unreachable { .. } => exit::DAEMON_UNREACHABLE,
                ClientError::Exchange(_) | ClientError::Request(_) | ClientError::Output(_) => {
                    exit::OTHER
                }
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
fn exit_code_for(problem: Option<&Problem>, status: u16) -> u8 {
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
        ErrorCode::RootNotFound | ErrorCode::SnapshotNotFound | ErrorCode::PathNotFound => {
            exit::NOT_FOUND
        }
        // A configuration that cannot be read is a state the Root is in until someone edits a file.
        ErrorCode::RootOverlaps | ErrorCode::RootUnavailable | ErrorCode::ConfigInvalid => {
            exit::STATE_CONFLICT
        }
        // Asking for the content of a directory, or the entries of a file, is a mistake in what was
        // asked rather than a state anything is in.
        ErrorCode::PathInvalid
        | ErrorCode::LimitInvalid
        | ErrorCode::CursorInvalid
        | ErrorCode::MomentInvalid
        | ErrorCode::DepthInvalid
        | ErrorCode::NotAFile
        | ErrorCode::NotADirectory
        | ErrorCode::PurgeInvalid => exit::USAGE,
        // The CLI builds every body from the contract types, so a refused one is this build's fault and
        // nothing the person typed.
        ErrorCode::BodyInvalid
        | ErrorCode::RegistryFailed
        | ErrorCode::CaptureFailed
        | ErrorCode::ClockUnusable
        | ErrorCode::HistoryUnreadable => exit::OTHER,
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
                println!("stopped watching {} (its history is kept)", root.path);
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
