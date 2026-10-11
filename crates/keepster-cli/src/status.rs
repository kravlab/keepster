//! `keepster status`: the single authority on whether work is being captured (ticket 05).

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use keepster_api::{RootState, Status};

use crate::client::{ClientError, DaemonClient, Response};
use crate::command::StatusArgs;
use crate::exit;
use crate::render::{write_human, write_json};

/// What the command prints and reports.
///
/// Deciding this over values, apart from both the transport and the terminal, is what makes every
/// branch reachable in a test — an unreachable daemon, a refused request, a payload this build cannot
/// parse — without standing up a second server to answer wrongly on purpose.
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Bytes for standard output, empty when there is nothing to show.
    pub stdout: Vec<u8>,
    /// A message for standard error, absent when nothing went wrong.
    pub stderr: Option<String>,
    /// The process exit code.
    pub code: u8,
}

/// Decide what to print and what to return from the daemon's answer, offering `shell_git` — what
/// [`shell_git`] found — when the daemon has no usable `git`.
pub fn outcome(
    answer: Result<Response, ClientError>,
    json: bool,
    shell_git: Option<&Path>,
) -> Outcome {
    let response = match answer {
        Ok(response) => response,
        Err(error) => {
            // Exhaustive on purpose: a new failure must be given its code deliberately instead of
            // inheriting the catch-all, or a script loses the ability to tell failures apart.
            let code = match &error {
                ClientError::Unreachable { .. } => exit::DAEMON_UNREACHABLE,
                ClientError::Exchange(_)
                | ClientError::Request(_)
                | ClientError::Output(_)
                | ClientError::Refused(_) => exit::OTHER,
            };

            return Outcome {
                stdout: Vec::new(),
                stderr: Some(error.to_string()),
                code,
            };
        }
    };

    if !response.status.is_success() {
        return Outcome {
            stdout: Vec::new(),
            stderr: Some(format!("the daemon answered {}", response.status)),
            code: exit::OTHER,
        };
    }

    let parsed = serde_json::from_slice::<Status>(&response.body);

    // Writing into a `Vec` cannot fail, so the results are unwrapped here rather than propagated into
    // a signature that would then have to describe an impossible error.
    let mut stdout = Vec::new();
    match (&parsed, json) {
        (Ok(status), false) => write_human(status, shell_git, &mut stdout),
        // A body this build cannot parse is still shown, unchanged: hiding the daemon's actual answer
        // is the opposite of what diagnosing it needs.
        _ => write_json(&response.body, &mut stdout),
    }
    .expect("writing to a buffer in memory");

    match parsed {
        Ok(status) => Outcome {
            stdout,
            stderr: None,
            code: exit_code_for(&status),
        },
        Err(error) => Outcome {
            stdout,
            stderr: Some(format!(
                "the daemon's status payload could not be read: {error}"
            )),
            code: exit::OTHER,
        },
    }
}

/// Fetch the status, print it, and return the process exit code.
///
/// The code is part of the contract, not a detail: a script must be able to tell "a Root is not being
/// watched" from "the daemon is dead", which is why `status` has its own code rather than sharing `1`
/// with every other failure (ticket 07).
pub async fn status(args: &StatusArgs, control_path: &Path) -> u8 {
    let answer = DaemonClient::new(control_path).get("/v1/status").await;
    let outcome = outcome(answer, args.json, shell_git().as_deref());

    if let Some(message) = &outcome.stderr {
        eprintln!("keepster: {message}");
    }

    let mut out = io::stdout().lock();
    if let Err(error) = out.write_all(&outcome.stdout).and_then(|()| out.flush()) {
        eprintln!("keepster: could not write the status: {error}");
        return exit::OTHER;
    }

    outcome.code
}

/// The exit code a status payload implies.
///
/// Non-zero whenever any Root is not `Watching`, so that the command is usable in a shell prompt or a
/// periodic check without parsing its output (ticket 05) — and, Roots being fine, whenever maintenance
/// cannot run, since then the store only grows (ticket 11).
pub fn exit_code_for(status: &Status) -> u8 {
    if !status
        .roots
        .iter()
        .all(|root| root.state == RootState::Watching)
    {
        exit::ROOT_NOT_WATCHING
    } else if status.daemon.maintenance == keepster_api::Reported::Unavailable {
        exit::MAINTENANCE_UNAVAILABLE
    } else {
        exit::SUCCESS
    }
}

/// The `git` this command's own `PATH` finds — the one the person's shell runs, which the daemon, never
/// reading a `PATH`, may not (ticket 11). macOS's `/usr/bin/git` stub is not one.
pub fn shell_git() -> Option<PathBuf> {
    let name = if cfg!(windows) { "git.exe" } else { "git" };
    let stub = cfg!(target_os = "macos").then(|| Path::new("/usr/bin/git"));

    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file() && Some(candidate.as_path()) != stub)
}
