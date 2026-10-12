//! `keepster restore`: a Snapshot's versions back over the Working Tree (ticket 47).
//!
//! The daemon never asks, it refuses: every question it raises comes back as `confirmation_required`
//! with what each file needs. On a terminal the person is asked here and the request sent again; without
//! one — or with `--no-input` — each answer is named by the flag that gives it, and the exit code says a
//! state is waiting on someone (ticket 07).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use keepster_api::{
    Confirm, ConfirmAll, Confirmation, ErrorCode, ModesChoice, NameCaseChoice, PathResolution,
    Problem, RestoreIssue, RestoreOutcome, RestoreRequest, Restored, RestoredPath, Root,
};

use crate::client::DaemonClient;
use crate::command::{ResolutionArgs, RestoreArgs};
use crate::exit;
use crate::history::{inside, located, selected};
use crate::roots::{answered, body_of};

/// Put a Snapshot's versions of files back.
pub async fn restore(args: &RestoreArgs, control_path: &Path, now: SystemTime) -> u8 {
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
    let within = |path: &Path| within_root(&root, path);
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

    let mut answers = match Answers::from_flags(&args.resolving, confirmations(args), &within) {
        Ok(answers) => answers,
        Err(code) => return code,
    };

    let url = format!("/v1/roots/{}/restore", root.id);
    let interactive = !args.resolving.no_input && !args.json && io::stdin().is_terminal();
    loop {
        let request = RestoreRequest {
            from: from.clone(),
            paths: paths.clone(),
            dry_run: args.dry_run,
            confirm: answers.confirm.clone(),
            resolutions: answers.resolutions.clone(),
            skip_unresolvable: answers.skip_unresolvable,
            modes: answers.modes,
        };
        let body = serde_json::to_vec(&request).expect("a restore request serialises");
        let answer = client.post(&url, &body).await;

        if let Ok(response) = &answer {
            if let Ok(problem) = serde_json::from_slice::<Problem>(&response.body) {
                if problem.code == ErrorCode::ConfirmationRequired.as_str() {
                    if args.json {
                        let _ = io::stdout().write_all(&response.body);
                        let _ = writeln!(io::stdout());
                        return exit::STATE_CONFLICT;
                    }
                    if interactive && ask(&mut answers, &problem.paths, RESTORE) {
                        continue;
                    }
                    report_blocked(&problem.paths, RESTORE);
                    return exit::STATE_CONFLICT;
                }
            }
        }

        if args.json {
            return match body_of(answer) {
                Ok(body) => {
                    let mut out = io::stdout().lock();
                    let _ = out.write_all(&body);
                    let _ = writeln!(out);
                    // The same exit code as the printed form: some file failed is a partial success.
                    let failed = serde_json::from_slice::<Restored>(&body).is_ok_and(|restored| {
                        restored
                            .paths
                            .iter()
                            .any(|path| path.outcome == RestoreOutcome::Failed)
                    });
                    if failed { exit::PARTIAL } else { exit::SUCCESS }
                }
                Err(code) => code,
            };
        }
        let restored: Restored = match answered(answer) {
            Ok(restored) => restored,
            Err(code) => return code,
        };
        return report(&restored, &args.paths);
    }
}

/// `path` relative to the Root, as every endpoint takes it — or the exit code of a path outside it.
pub(crate) fn within_root(root: &Root, path: &Path) -> Result<String, u8> {
    match inside(Path::new(&root.path), path) {
        Ok(Some(relative)) => Ok(relative),
        Ok(None) => {
            eprintln!(
                "keepster: {} is not inside the Root at {}",
                path.display(),
                root.path
            );
            Err(exit::USAGE)
        }
        Err(error) => {
            eprintln!(
                "keepster: {} could not be resolved: {error}",
                path.display()
            );
            Err(exit::OTHER)
        }
    }
}

/// The paths typed, relative to the Root — none when one is the Root itself, which is every file and
/// what no path means to the daemon.
pub(crate) fn paths_within(root: &Root, typed: &[PathBuf]) -> Result<Vec<String>, u8> {
    let mut paths = typed
        .iter()
        .map(|path| within_root(root, path))
        .collect::<Result<Vec<_>, _>>()?;
    if paths.iter().any(String::is_empty) {
        paths.clear();
    }

    Ok(paths)
}

/// What a Restore is called where the person is told what it did not do.
const RESTORE: Operation = Operation {
    past: "restored",
    verb: "restore",
};

/// An operation that asks the same questions as a Restore, as messages name it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Operation {
    /// What a file it wrote was: `restored`.
    pub(crate) past: &'static str,
    /// What it does: `restore`.
    pub(crate) verb: &'static str,
}

/// The answers a request carries to what a Restore or an Export asks: from the flags, then from the
/// person at a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Answers {
    /// The changes agreed to.
    pub(crate) confirm: Confirm,
    /// Each path's answer.
    pub(crate) resolutions: BTreeMap<String, PathResolution>,
    /// Whether what is still in the way is passed over.
    pub(crate) skip_unresolvable: bool,
    /// The answer about diverging modes for every file without its own.
    pub(crate) modes: Option<ModesChoice>,
}

impl Answers {
    /// What `flags` answer, beside the confirmations `confirm` gives, each path made Root-relative by
    /// `within` — or the exit code of a flag that cannot be read.
    pub(crate) fn from_flags(
        flags: &ResolutionArgs,
        confirm: Confirm,
        within: &dyn Fn(&Path) -> Result<String, u8>,
    ) -> Result<Self, u8> {
        let mut answers = Self {
            confirm,
            resolutions: BTreeMap::new(),
            skip_unresolvable: flags.skip_unresolvable,
            modes: None,
        };
        for path in &flags.pick {
            let relative = within(path)?;
            answers.resolutions.entry(relative).or_default().write = Some(true);
        }
        for given in &flags.rename {
            let Some((path, name)) = given.rsplit_once('=') else {
                eprintln!("keepster: --rename takes PATH=NAME, not `{given}`");
                return Err(exit::USAGE);
            };
            let relative = within(Path::new(path))?;
            answers.resolutions.entry(relative).or_default().rename_to = Some(name.to_owned());
        }
        for given in &flags.name_case {
            let (path, choice) = match given.rsplit_once('=') {
                Some((path, "keep")) => (path, NameCaseChoice::KeepExisting),
                Some((path, "snapshot")) => (path, NameCaseChoice::UseSnapshot),
                _ => {
                    eprintln!(
                        "keepster: --name-case takes PATH=keep or PATH=snapshot, not `{given}`"
                    );
                    return Err(exit::USAGE);
                }
            };
            let relative = within(Path::new(path))?;
            answers.resolutions.entry(relative).or_default().name_case = Some(choice);
        }
        for given in &flags.modes {
            let (path, answer) = match given.rsplit_once('=') {
                Some((path, answer)) => (Some(path), answer),
                None => (None, given.as_str()),
            };
            let Some(choice) = modes_choice(answer) else {
                eprintln!(
                    "keepster: --modes takes snapshot, keep, PATH=snapshot or PATH=keep, not `{given}`"
                );
                return Err(exit::USAGE);
            };
            match path {
                None => answers.modes = Some(choice),
                Some(path) => {
                    let relative = within(Path::new(path))?;
                    answers.resolutions.entry(relative).or_default().modes = Some(choice);
                }
            }
        }

        Ok(answers)
    }
}

fn modes_choice(answer: &str) -> Option<ModesChoice> {
    match answer {
        "snapshot" | "s" => Some(ModesChoice::Snapshot),
        "keep" | "k" => Some(ModesChoice::Keep),
        _ => None,
    }
}

/// An issue's two modes, `0644 on disk, 0600 in the Snapshot`, when it is about modes.
fn modes_of(issue: &RestoreIssue) -> Option<String> {
    Some(format!(
        "{:04o} on disk, {:04o} in the Snapshot",
        issue.current_mode?, issue.snapshot_mode?
    ))
}

/// The confirmations the flags give.
fn confirmations(args: &RestoreArgs) -> Confirm {
    if args.force {
        return Confirm::All(ConfirmAll::All);
    }

    Confirm::These(
        [
            (args.create_parents, Confirmation::CreateParents),
            (args.replace_type, Confirmation::ReplaceType),
            (args.break_hardlinks, Confirmation::BreakHardlinks),
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

/// What happened to every file, and how to undo it.
fn report(restored: &Restored, typed: &[PathBuf]) -> u8 {
    let mut out = io::stdout().lock();
    for path in &restored.paths {
        let verb = match (restored.dry_run, path.outcome) {
            (true, RestoreOutcome::Restored) => "would restore",
            (true, RestoreOutcome::Created) => "would create",
            (true, RestoreOutcome::Skipped) => "would skip",
            (_, RestoreOutcome::Restored) => "restored",
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
            let reasons: Vec<&str> = path
                .issues
                .iter()
                .map(|issue| issue.reason.as_str())
                .collect();
            let _ = write!(out, "  ({})", reasons.join(", "));
        }
        let _ = writeln!(out);
    }

    if let Some(safety) = &restored.safety_snapshot {
        let paths: Vec<String> = typed
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        let _ = writeln!(
            out,
            "Safety Snapshot {safety}: undo with `keepster restore --from {safety} {}`",
            paths.join(" ")
        );
        if restored
            .paths
            .iter()
            .any(|path| path.outcome == RestoreOutcome::Created)
        {
            let _ = writeln!(
                out,
                "Files this created stay after an undo: a restore never deletes."
            );
        }
    }
    let _ = out.flush();

    if restored
        .paths
        .iter()
        .any(|path| path.outcome == RestoreOutcome::Failed)
    {
        exit::PARTIAL
    } else {
        exit::SUCCESS
    }
}

/// Every file that stopped the request, with what would let it through.
pub(crate) fn report_blocked(paths: &[RestoredPath], operation: Operation) {
    eprintln!(
        "keepster: nothing was {}; these need an answer first:",
        operation.past
    );
    for path in paths
        .iter()
        .filter(|path| path.outcome == RestoreOutcome::Blocked)
    {
        for issue in &path.issues {
            let hint = match (issue.confirm.as_deref(), issue.resolution.as_deref()) {
                (Some(confirm), _) => format!("pass --{} (or --force)", confirm.replace('_', "-")),
                (_, Some("write")) => {
                    format!("pass --pick {} to write it and skip the others", path.path)
                }
                (_, Some("rename_to")) => format!("pass --rename {}=<name>", path.path),
                (_, Some("name_case")) => format!("pass --name-case {}=keep|snapshot", path.path),
                (_, Some("modes")) => format!(
                    "{}; pass --modes {}=snapshot|keep, or --modes snapshot|keep for every file",
                    modes_of(issue).unwrap_or_default(),
                    path.path
                ),
                _ => format!(
                    "it cannot be {}; pass --skip-unresolvable to {} the rest",
                    operation.past, operation.verb
                ),
            };
            eprintln!("  {}: {} — {hint}", path.path, issue.reason);
        }
    }
}

/// Ask about everything that stopped the request, and amend its answers; `false` when the person
/// declined.
pub(crate) fn ask(request: &mut Answers, paths: &[RestoredPath], operation: Operation) -> bool {
    let asked = request.clone();
    let blocked: Vec<&RestoredPath> = paths
        .iter()
        .filter(|path| path.outcome == RestoreOutcome::Blocked)
        .collect();

    let wanted: BTreeSet<String> = blocked
        .iter()
        .flat_map(|path| path.issues.iter())
        .filter_map(|issue| issue.confirm.clone())
        .collect();
    for confirm in wanted {
        let affected: Vec<&str> = blocked
            .iter()
            .filter(|path| {
                path.issues
                    .iter()
                    .any(|issue| issue.confirm.as_deref() == Some(confirm.as_str()))
            })
            .map(|path| path.path.as_str())
            .collect();
        let question = match confirm.as_str() {
            "create_parents" => "Create the missing directories for",
            "replace_type" => "Replace what is of another kind in the way of",
            "break_hardlinks" => "Break the other names (they keep the old content) of",
            "overwrite" => "Write over what is already there, which nothing can bring back, at",
            "dest_in_root" => "Write into a Watched Root, which will capture it,",
            _ => "Overwrite what the Safety Snapshot cannot hold, at",
        };
        if !yes(&format!("{question} {}?", affected.join(", "))) {
            return false;
        }
        let Some(confirmation) = parse_confirmation(&confirm) else {
            return false;
        };
        match &mut request.confirm {
            Confirm::These(these) => these.push(confirmation),
            Confirm::All(_) => {}
        }
    }

    // Modes first, and for every file at once where there are several: a `chmod -R` diverges everywhere.
    let diverging: Vec<(&str, &RestoreIssue)> = blocked
        .iter()
        .flat_map(|path| {
            path.issues
                .iter()
                .filter(|issue| issue.resolution.as_deref() == Some("modes"))
                .map(|issue| (path.path.as_str(), issue))
        })
        .collect();
    if diverging.len() > 1 {
        let names: Vec<&str> = diverging.iter().map(|(path, _)| *path).collect();
        let answer = line(&format!(
            "The permission bits of {} differ from the Snapshot's. Take the Snapshot's, keep those \
             on disk, or answer for each? [s/k/e] ",
            names.join(", ")
        ));
        match answer.as_str() {
            "e" | "each" => {}
            answer => match modes_choice(answer) {
                Some(choice) => request.modes = Some(choice),
                None => return false,
            },
        }
    }

    let mut refused = Vec::new();
    for path in &blocked {
        for issue in &path.issues {
            let resolution = request.resolutions.entry(path.path.clone()).or_default();
            match issue.resolution.as_deref() {
                Some("write") => {
                    let others = issue.paths.join(", ");
                    if yes(&format!(
                        "{} and {others} are one file here. Write {}?",
                        path.path, path.path
                    )) {
                        resolution.write = Some(true);
                    }
                }
                Some("rename_to") => {
                    let name = line(&format!(
                        "{} cannot be named so here. New name (empty to stop): ",
                        path.path
                    ));
                    if name.is_empty() {
                        return false;
                    }
                    resolution.rename_to = Some(name);
                }
                Some("name_case") => {
                    let existing = issue.existing.as_deref().unwrap_or("another name");
                    let answer = line(&format!(
                        "{existing} is already there. Keep that name, or use the Snapshot's {}? [k/s] ",
                        path.path
                    ));
                    resolution.name_case = match answer.as_str() {
                        "k" | "keep" => Some(NameCaseChoice::KeepExisting),
                        "s" | "snapshot" => Some(NameCaseChoice::UseSnapshot),
                        _ => return false,
                    };
                }
                Some("modes") if request.modes.is_none() => {
                    let answer = line(&format!(
                        "{}: permission bits {}. Take the Snapshot's, or keep those on disk? [s/k] ",
                        path.path,
                        modes_of(issue).unwrap_or_default()
                    ));
                    match modes_choice(&answer) {
                        Some(choice) => resolution.modes = Some(choice),
                        None => return false,
                    }
                }
                Some(_) => {}
                None if issue.confirm.is_none() => {
                    refused.push(format!("{}: {}", path.path, issue.reason))
                }
                None => {}
            }
        }
    }
    request
        .resolutions
        .retain(|_, resolution| *resolution != PathResolution::default());

    if !refused.is_empty() {
        eprintln!("These cannot be {}: {}", operation.past, refused.join("; "));
        if !yes(&format!("Skip them and {} the rest?", operation.verb)) {
            return false;
        }
        request.skip_unresolvable = true;
    }

    // Declining every question leaves the request as it was, and sending it again would only ask again.
    *request != asked
}

fn parse_confirmation(name: &str) -> Option<Confirmation> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}

/// A yes-or-no answer from the terminal, no unless said.
pub(crate) fn yes(question: &str) -> bool {
    matches!(line(&format!("{question} [y/N] ")).as_str(), "y" | "yes")
}

/// One line typed at the terminal, trimmed.
pub(crate) fn line(prompt: &str) -> String {
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    let _ = io::stdin().lock().read_line(&mut answer);

    answer.trim().to_owned()
}
