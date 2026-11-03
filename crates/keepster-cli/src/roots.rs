//! The Watched Root commands: putting a directory under history, listing the Roots, unwatching one,
//! erasing one's history and recording that one has moved.

use std::io::{self, Write};
use std::path::Path;

use keepster_api::{
    ErrorCode, MISSING_PATH_MODE, MoveRoot, PathMode, Pins, Problem, RegisterRoot, Reported, Root,
    Status,
};
use serde::de::DeserializeOwned;

use crate::client::{ClientError, DaemonClient, Response};
use crate::command::{MoveArgs, PathModeArgs, PurgeArgs, UnwatchArgs, WatchArgs};
use crate::exit;
use crate::preview::Shown;
use crate::render::{write_git, write_roots};
use crate::resolve::Target;

/// Register a directory as a Watched Root.
///
/// In order: `--dry-run` only previews; then the claim of what one Root may hold is checked (`--yes`
/// skips it); then the directory is registered, and a path through a link is settled last, since only
/// the daemon's refusal says that it is one — by `--keep-link` or `--canonical`, by asking at a
/// terminal, or by saying which flag to pass (ticket 77).
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
    let can_ask = crate::restore::may_ask(args.no_input);
    if !args.yes {
        if let Err(code) = within_claim(&client, &path, can_ask).await {
            return code;
        }
    }

    match register_through_link(
        &client,
        &Sending::Register,
        &path,
        chosen(&args.path_mode),
        can_ask,
    )
    .await
    {
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
/// watched by surprise. Where nothing can be asked (no terminal, or `--no-input`), `--yes` says it.
///
/// A preview that cannot be had says nothing either way, and registering goes on: it will say for itself
/// whatever is wrong with the directory. One that is busy is the exception, since registering would not
/// notice: the directory is not watched unseen, and the command says to try again.
async fn within_claim(
    client: &DaemonClient,
    path: &std::path::Path,
    can_ask: bool,
) -> Result<(), u8> {
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
    let asked = can_ask
        && crate::restore::yes(format!(
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
        | ErrorCode::PreviewBusy
        | ErrorCode::ListenerUnavailable => exit::STATE_CONFLICT,
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
        | ErrorCode::SearchInvalid
        | ErrorCode::LogLevelInvalid
        | ErrorCode::PreferenceInvalid
        | ErrorCode::MuteInvalid => exit::USAGE,
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
        | ErrorCode::DirectoryUnreadable
        | ErrorCode::TokenFailed
        | ErrorCode::PreferencesFailed
        // Answered to a tray's report, which the CLI never sends.
        | ErrorCode::TrayInvalid
        // Answered by the loopback listener alone, which the CLI never speaks to.
        | ErrorCode::Unauthorized
        | ErrorCode::HostRefused
        | ErrorCode::OriginRefused => exit::OTHER,
    }
}

/// The request body of a registration, built from the contract type rather than from a literal, so the
/// shape lives in `keepster-api` alone. `path_mode` is the choice for a path through a link, absent
/// until one has been made.
fn register_body(path: &std::path::Path, path_mode: Option<PathMode>) -> Vec<u8> {
    let request = RegisterRoot {
        path: path.to_string_lossy().into_owned(),
        path_mode,
    };

    serde_json::to_vec(&request).expect("the request serialises")
}

/// The request body of a move, as [`register_body`] is of a registration.
fn move_body(path: &std::path::Path, path_mode: Option<PathMode>) -> Vec<u8> {
    let request = MoveRoot {
        path: path.to_string_lossy().into_owned(),
        path_mode,
    };

    serde_json::to_vec(&request).expect("the request serialises")
}

/// The choice the flags make for a path through a link; none when neither is given.
fn chosen(flags: &PathModeArgs) -> Option<PathMode> {
    match (flags.keep_link, flags.canonical) {
        (true, _) => Some(PathMode::Link),
        (_, true) => Some(PathMode::Canonical),
        _ => None,
    }
}

/// The two requests that take a path and may find it leads through a link (ticket 77).
enum Sending {
    /// `POST /v1/roots`: watch a directory.
    Register,
    /// `PATCH /v1/roots/{id}`: record that the Root of this identity has moved.
    Move(String),
}

/// What the daemon said a path through a link needs: the choice between the link and where it leads.
#[derive(Debug, PartialEq, Eq)]
struct PathModeNeeded {
    /// The path as it was sent.
    path: String,
    /// The real path the link leads to now.
    real_path: String,
}

/// The question a refusal asks, when it is the daemon asking which a path through a link is
/// (`confirmation_required` missing a `path_mode`); `None` for any other refusal.
fn path_mode_needed(problem: &Problem) -> Option<PathModeNeeded> {
    if problem.code != ErrorCode::ConfirmationRequired.as_str()
        || problem.details.get("missing").map(String::as_str) != Some(MISSING_PATH_MODE)
    {
        return None;
    }

    Some(PathModeNeeded {
        path: problem.details.get("path")?.clone(),
        real_path: problem.details.get("real_path")?.clone(),
    })
}

/// The question put to the person at a terminal; the answer is read by [`path_mode_answer`].
fn path_mode_question(needed: &PathModeNeeded) -> String {
    format!(
        "{} leads through a link to {}. Use the link, following it when it is pointed elsewhere (l), \
         or the real directory (c)? ",
        needed.path, needed.real_path
    )
}

/// The choice an answer to [`path_mode_question`] makes: `l` or `c`, or the word, in any case; `None` for
/// anything else, since a guess would register the wrong directory.
fn path_mode_answer(answer: &str) -> Option<PathMode> {
    match answer.trim().to_ascii_lowercase().as_str() {
        "l" | "link" | "keep-link" => Some(PathMode::Link),
        "c" | "canonical" => Some(PathMode::Canonical),
        _ => None,
    }
}

/// What a command line that cannot ask says: which flags make the choice.
fn path_mode_hint(needed: &PathModeNeeded) -> String {
    format!(
        "{} leads through a link to {}; pass --keep-link to use the link, following it when it is \
         pointed elsewhere, or --canonical to use the real directory",
        needed.path, needed.real_path
    )
}

/// Send `path` as a registration or a move, and settle the choice a path through a link needs.
///
/// A flag has made it already (`path_mode`). Otherwise the daemon refuses the path with
/// `confirmation_required` before changing anything; where `can_ask` the person is asked and the
/// request sent again with the answer, and elsewhere the command says which flags to pass and ends with
/// the exit code of that refusal. An answer that is neither choice changes nothing and ends the command
/// as an unanswered question does elsewhere.
async fn register_through_link(
    client: &DaemonClient,
    sending: &Sending,
    path: &std::path::Path,
    path_mode: Option<PathMode>,
    can_ask: bool,
) -> Result<Root, u8> {
    let mut path_mode = path_mode;
    loop {
        let answer = match sending {
            Sending::Register => {
                client
                    .post("/v1/roots", &register_body(path, path_mode))
                    .await
            }
            Sending::Move(id) => {
                client
                    .patch(&format!("/v1/roots/{id}"), &move_body(path, path_mode))
                    .await
            }
        };

        let needed = match (&answer, path_mode) {
            (Ok(response), None) => serde_json::from_slice::<Problem>(&response.body)
                .ok()
                .and_then(|problem| path_mode_needed(&problem)),
            _ => None,
        };
        let Some(needed) = needed else {
            return answered::<Root>(answer);
        };

        if !can_ask {
            eprintln!("keepster: {}", path_mode_hint(&needed));

            return Err(exit_code_for_known(ErrorCode::ConfirmationRequired));
        }
        match path_mode_answer(&crate::restore::line(path_mode_question(&needed))) {
            Some(chosen) => path_mode = Some(chosen),
            None => {
                eprintln!("keepster: nothing was changed");

                return Err(exit::USAGE);
            }
        }
    }
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

/// Deregister a Root, or every one, keeping its history unless an explicit purge says otherwise.
pub async fn unwatch(args: &UnwatchArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);

    // Only an erasing reaches a Root no longer watched: its history is all that is left of it.
    let selection = Selection {
        root: args.root.as_deref(),
        all: args.all,
        erasing: args.purge,
        yes: args.yes,
    };
    let targets = match selection.consented(&client, "--purge").await {
        Ok(targets) => targets,
        Err(code) => return code,
    };
    if targets.is_empty() {
        println!("no Root is watched; nothing was changed");
    }

    for target in &targets {
        if let Err(code) = unwatch_one(&client, target, args.purge).await {
            return code;
        }
    }

    exit::SUCCESS
}

/// Deregister one Root and say what became of its history.
async fn unwatch_one(client: &DaemonClient, target: &Target, purge: bool) -> Result<(), u8> {
    let Target { id, named } = target;
    let url = if purge {
        format!("/v1/roots/{id}?purge=true")
    } else {
        format!("/v1/roots/{id}")
    };
    accepted(client.delete(&url).await)?;

    if purge {
        println!("erased the history of {named}");
    } else {
        // The history decays down the ladder to its last state, pinned by the daemon, and stays
        // reachable by its identity alone (ticket 53). A Root that never captured anything has
        // neither, and is not told it has.
        let pinned = client
            .get(&format!("/v1/roots/{id}/pins"))
            .await
            .ok()
            .filter(|response| response.status.is_success())
            .and_then(|response| serde_json::from_slice::<Pins>(&response.body).ok())
            .is_some_and(|pins| !pins.snapshots.is_empty());
        if pinned {
            println!(
                "stopped watching {named}; its history is kept, decaying to its last state, which \
                 is pinned: `keepster pins -r {id}` lists it, and watching the directory again \
                 resumes it"
            );
        } else {
            println!("stopped watching {named} (it had no history to keep)");
        }
    }

    Ok(())
}

/// Erase a Root's history, or every Root's, and keep watching it (ticket 68); a Root no longer
/// watched, whose history was all it still had, is erased whole.
pub async fn purge(args: &PurgeArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);

    let selection = Selection {
        root: args.root.as_deref(),
        all: args.all,
        erasing: true,
        yes: args.yes,
    };
    let targets = match selection.consented(&client, "purge").await {
        Ok(targets) => targets,
        Err(code) => return code,
    };
    if targets.is_empty() {
        println!("no Root has a history; nothing was changed");
    }

    for Target { id, named } in &targets {
        if let Err(code) = accepted(client.delete(&format!("/v1/roots/{id}/history")).await) {
            return code;
        }
        println!("erased the history of {named}");
    }

    exit::SUCCESS
}

/// Which Roots an `unwatch` or a `purge` command line selects, and whether it consents to erasing.
struct Selection<'a> {
    /// The `--root` selector, if one was given.
    root: Option<&'a str>,
    /// Every Root rather than one.
    all: bool,
    /// Whether the command erases history.
    erasing: bool,
    /// The separate consent an erasing needs (ticket 27).
    yes: bool,
}

impl Selection<'_> {
    /// The Roots selected, once an erasing has its consent; `command` names the erasing in a refusal.
    ///
    /// Every Root is refused before the daemon is asked, since "every Root" is all a refusal can name;
    /// one Root is found first, so that the refusal names it.
    async fn consented(&self, client: &DaemonClient, command: &str) -> Result<Vec<Target>, u8> {
        let unconsented = self.erasing && !self.yes;
        if unconsented && self.all {
            return Err(refuse(command, "every Root"));
        }
        let targets = crate::resolve::targets(client, self.root, self.all, self.erasing).await?;
        match targets.as_slice() {
            [Target { named, .. }] if unconsented => Err(refuse(command, named)),
            _ => Ok(targets),
        }
    }
}

/// Say that `command` was refused for want of consent, and give the exit code.
fn refuse(command: &str, named: &str) -> u8 {
    eprintln!(
        "keepster: {}",
        crate::consent::refusal(&format!(
            "{command} erases every captured version of {named}"
        ))
    );

    exit::USAGE
}

/// Record that a Root's directory has moved. A new path through a link is settled as `watch` settles
/// one.
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

    match register_through_link(
        &client,
        &Sending::Move(root.id),
        &path,
        chosen(&args.path_mode),
        crate::restore::may_ask(args.no_input),
    )
    .await
    {
        Ok(moved) => print_roots(std::slice::from_ref(&moved)),
        Err(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use keepster_api::Problem;

    use keepster_api::PathMode;

    use super::{
        PathModeNeeded, chosen, exit_code_for, path_mode_answer, path_mode_needed,
        path_mode_question,
    };
    use crate::command::PathModeArgs;
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

    #[test]
    fn the_answer_to_the_question_is_the_letter_or_the_word_whatever_its_case() {
        assert_eq!(path_mode_answer("l"), Some(PathMode::Link));
        assert_eq!(path_mode_answer("L"), Some(PathMode::Link));
        assert_eq!(path_mode_answer("link"), Some(PathMode::Link));
        assert_eq!(path_mode_answer("keep-link"), Some(PathMode::Link));
        assert_eq!(path_mode_answer("c"), Some(PathMode::Canonical));
        assert_eq!(path_mode_answer("C"), Some(PathMode::Canonical));
        assert_eq!(path_mode_answer("canonical"), Some(PathMode::Canonical));
        // Anything else is no answer: a guess would register the wrong directory.
        assert_eq!(path_mode_answer(""), None);
        assert_eq!(path_mode_answer("y"), None);
        assert_eq!(path_mode_answer("lc"), None);
    }

    #[test]
    fn the_flags_make_the_choice_and_without_them_there_is_none() {
        let flags = |keep_link, canonical| PathModeArgs {
            keep_link,
            canonical,
        };

        assert_eq!(chosen(&flags(true, false)), Some(PathMode::Link));
        assert_eq!(chosen(&flags(false, true)), Some(PathMode::Canonical));
        assert_eq!(chosen(&flags(false, false)), None);
    }

    #[test]
    fn only_a_confirmation_missing_the_path_mode_asks_which_a_path_through_a_link_is() {
        let mut asking = problem("confirmation_required");
        asking.details = BTreeMap::from([
            ("path".to_owned(), "/home/u/current".to_owned()),
            ("real_path".to_owned(), "/home/u/release".to_owned()),
            (
                "missing".to_owned(),
                keepster_api::MISSING_PATH_MODE.to_owned(),
            ),
        ]);

        assert_eq!(
            path_mode_needed(&asking),
            Some(PathModeNeeded {
                path: "/home/u/current".to_owned(),
                real_path: "/home/u/release".to_owned(),
            })
        );

        // Another confirmation, another code, or particulars left out are a refusal like any other.
        let mut other = asking.clone();
        other.details.insert("missing".to_owned(), "x".to_owned());
        assert_eq!(path_mode_needed(&other), None);
        let mut other = asking.clone();
        other.code = "path_invalid".to_owned();
        assert_eq!(path_mode_needed(&other), None);
        let mut other = asking.clone();
        other.details.remove("real_path");
        assert_eq!(path_mode_needed(&other), None);
        assert_eq!(path_mode_needed(&problem("confirmation_required")), None);
    }

    #[test]
    fn the_question_names_both_directories_and_the_letter_that_picks_each() {
        let needed = PathModeNeeded {
            path: "/home/u/current".to_owned(),
            real_path: "/home/u/release".to_owned(),
        };

        let question = path_mode_question(&needed);

        assert!(question.contains("/home/u/current"), "{question}");
        assert!(question.contains("/home/u/release"), "{question}");
        assert!(question.contains("(l)"), "{question}");
        assert!(question.contains("(c)"), "{question}");
    }
}
