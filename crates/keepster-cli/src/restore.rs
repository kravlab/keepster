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
    Problem, RestoreIssue, RestoreOutcome, RestoreRequest, Restored, RestoredPath, Root, path_text,
    paths_bytes, resolutions_to_wire,
};

use crate::client::DaemonClient;
use crate::command::{ResolutionArgs, RestoreArgs};
use crate::exit;
use crate::history::{inside, located, selected};
use crate::names::{exact, joined, printed, printed_all, split_assignment, write_path};
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
        let (resolutions, resolutions_bytes) = resolutions_to_wire(&answers.resolutions);
        let request = RestoreRequest {
            from: from.clone(),
            paths: paths.iter().map(|path| path_text(path)).collect(),
            paths_bytes: paths_bytes(&paths),
            dry_run: args.dry_run,
            confirm: answers.confirm.clone(),
            resolutions,
            resolutions_bytes,
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

/// `path` relative to the Root, as every endpoint takes it, as its bytes — or the exit code of a path
/// outside it.
pub(crate) fn within_root(root: &Root, path: &Path) -> Result<Vec<u8>, u8> {
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
pub(crate) fn paths_within(root: &Root, typed: &[PathBuf]) -> Result<Vec<Vec<u8>>, u8> {
    let mut paths = typed
        .iter()
        .map(|path| within_root(root, path))
        .collect::<Result<Vec<_>, _>>()?;
    if paths.iter().any(Vec::is_empty) {
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
    /// Each path's answer, by the path's bytes.
    pub(crate) resolutions: BTreeMap<Vec<u8>, PathResolution>,
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
        within: &dyn Fn(&Path) -> Result<Vec<u8>, u8>,
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
        // Each `PATH=` names a path as it was typed, which on Unix need not be UTF-8 (ticket 74).
        for given in &flags.rename {
            let Some((path, name)) = split_assignment(given) else {
                eprintln!(
                    "keepster: --rename takes PATH=NAME, not `{}`",
                    given.to_string_lossy()
                );
                return Err(exit::USAGE);
            };
            let relative = within(&path)?;
            answers.resolutions.entry(relative).or_default().rename_to = Some(name);
        }
        for given in &flags.name_case {
            let (path, choice) = match split_assignment(given) {
                Some((path, answer)) if answer == "keep" => (path, NameCaseChoice::KeepExisting),
                Some((path, answer)) if answer == "snapshot" => (path, NameCaseChoice::UseSnapshot),
                _ => {
                    eprintln!(
                        "keepster: --name-case takes PATH=keep or PATH=snapshot, not `{}`",
                        given.to_string_lossy()
                    );
                    return Err(exit::USAGE);
                }
            };
            let relative = within(&path)?;
            answers.resolutions.entry(relative).or_default().name_case = Some(choice);
        }
        for given in &flags.modes {
            let (path, answer) = match split_assignment(given) {
                Some((path, answer)) => (Some(path), answer),
                None => (None, given.to_string_lossy().into_owned()),
            };
            let Some(choice) = modes_choice(&answer) else {
                eprintln!(
                    "keepster: --modes takes snapshot, keep, PATH=snapshot or PATH=keep, not `{}`",
                    given.to_string_lossy()
                );
                return Err(exit::USAGE);
            };
            match path {
                None => answers.modes = Some(choice),
                Some(path) => {
                    let relative = within(&path)?;
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
        let _ = write!(out, "{verb}  ");
        let _ = write_path(&mut out, &path.path, path.path_bytes.as_deref());
        if let Some(target) = &path.target {
            let _ = write!(out, " -> ");
            let _ = write_path(&mut out, target, path.target_bytes.as_deref());
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

/// Every file that stopped the request, with what would let it through. Each path is printed as
/// [`printed`] spells it, so the hint can be typed back as it stands (ticket 74).
pub(crate) fn report_blocked(paths: &[RestoredPath], operation: Operation) {
    let mut err = io::stderr().lock();
    let _ = writeln!(
        err,
        "keepster: nothing was {}; these need an answer first:",
        operation.past
    );
    for path in paths
        .iter()
        .filter(|path| path.outcome == RestoreOutcome::Blocked)
    {
        let name = printed(&path.path, path.path_bytes.as_deref());
        for issue in &path.issues {
            let hint = match (issue.confirm.as_deref(), issue.resolution.as_deref()) {
                (Some(confirm), _) => {
                    format!("pass --{} (or --force)", confirm.replace('_', "-")).into_bytes()
                }
                (_, Some("write")) => [
                    &b"pass --pick "[..],
                    &name,
                    b" to write it and skip the others",
                ]
                .concat(),
                (_, Some("rename_to")) => [&b"pass --rename "[..], &name, b"=<name>"].concat(),
                (_, Some("name_case")) => {
                    [&b"pass --name-case "[..], &name, b"=keep|snapshot"].concat()
                }
                (_, Some("modes")) => [
                    modes_of(issue).unwrap_or_default().as_bytes(),
                    b"; pass --modes ",
                    &name,
                    b"=snapshot|keep, or --modes snapshot|keep for every file",
                ]
                .concat(),
                _ => format!(
                    "it cannot be {}; pass --skip-unresolvable to {} the rest",
                    operation.past, operation.verb
                )
                .into_bytes(),
            };
            let _ = err.write_all(
                &[
                    &b"  "[..],
                    &name,
                    format!(": {} — ", issue.reason).as_bytes(),
                    &hint,
                    b"\n",
                ]
                .concat(),
            );
        }
    }
}

/// Ask about everything that stopped the request, and amend its answers; `false` when the person
/// declined. Each answer is keyed by the exact bytes of its path, and each question names the path as
/// [`printed`] spells it.
pub(crate) fn ask(request: &mut Answers, paths: &[RestoredPath], operation: Operation) -> bool {
    let asked = request.clone();
    let blocked: Vec<(&RestoredPath, Vec<u8>)> = paths
        .iter()
        .filter(|path| path.outcome == RestoreOutcome::Blocked)
        .map(|path| (path, printed(&path.path, path.path_bytes.as_deref())))
        .collect();

    let wanted: BTreeSet<String> = blocked
        .iter()
        .flat_map(|(path, _)| path.issues.iter())
        .filter_map(|issue| issue.confirm.clone())
        .collect();
    for confirm in wanted {
        let affected: Vec<Vec<u8>> = blocked
            .iter()
            .filter(|(path, _)| {
                path.issues
                    .iter()
                    .any(|issue| issue.confirm.as_deref() == Some(confirm.as_str()))
            })
            .map(|(_, name)| name.clone())
            .collect();
        let question = match confirm.as_str() {
            "create_parents" => "Create the missing directories for",
            "replace_type" => "Replace what is of another kind in the way of",
            "break_hardlinks" => "Break the other names (they keep the old content) of",
            "overwrite" => "Write over what is already there, which nothing can bring back, at",
            "dest_in_root" => "Write into a Watched Root, which will capture it,",
            _ => "Overwrite what the Safety Snapshot cannot hold, at",
        };
        if !yes([question.as_bytes(), b" ", &joined(&affected), b"?"].concat()) {
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
    let diverging: Vec<&[u8]> = blocked
        .iter()
        .flat_map(|(path, name)| {
            path.issues
                .iter()
                .filter(|issue| issue.resolution.as_deref() == Some("modes"))
                .map(move |_| name.as_slice())
        })
        .collect();
    if diverging.len() > 1 {
        let answer = line(
            [
                &b"The permission bits of "[..],
                &joined(&diverging),
                b" differ from the Snapshot's. Take the Snapshot's, keep those on disk, or answer \
                  for each? [s/k/e] ",
            ]
            .concat(),
        );
        match answer.as_str() {
            "e" | "each" => {}
            answer => match modes_choice(answer) {
                Some(choice) => request.modes = Some(choice),
                None => return false,
            },
        }
    }

    let mut refused = Vec::new();
    for (path, name) in &blocked {
        for issue in &path.issues {
            let resolution = request
                .resolutions
                .entry(exact(&path.path, path.path_bytes.as_deref()))
                .or_default();
            match issue.resolution.as_deref() {
                Some("write") => {
                    let others = printed_all(&issue.paths, &issue.paths_bytes);
                    if yes([
                        &name[..],
                        b" and ",
                        &joined(&others),
                        b" are one file here. Write ",
                        name,
                        b"?",
                    ]
                    .concat())
                    {
                        resolution.write = Some(true);
                    }
                }
                Some("rename_to") => {
                    let name = line(
                        [
                            &name[..],
                            b" cannot be named so here. New name (empty to stop): ",
                        ]
                        .concat(),
                    );
                    if name.is_empty() {
                        return false;
                    }
                    resolution.rename_to = Some(name);
                }
                Some("name_case") => {
                    let existing = match &issue.existing {
                        Some(existing) => printed(existing, issue.existing_bytes.as_deref()),
                        None => b"another name".to_vec(),
                    };
                    let answer = line(
                        [
                            &existing[..],
                            b" is already there. Keep that name, or use the Snapshot's ",
                            name,
                            b"? [k/s] ",
                        ]
                        .concat(),
                    );
                    resolution.name_case = match answer.as_str() {
                        "k" | "keep" => Some(NameCaseChoice::KeepExisting),
                        "s" | "snapshot" => Some(NameCaseChoice::UseSnapshot),
                        _ => return false,
                    };
                }
                Some("modes") if request.modes.is_none() => {
                    let answer = line(
                        [
                            &name[..],
                            b": permission bits ",
                            modes_of(issue).unwrap_or_default().as_bytes(),
                            b". Take the Snapshot's, or keep those on disk? [s/k] ",
                        ]
                        .concat(),
                    );
                    match modes_choice(&answer) {
                        Some(choice) => resolution.modes = Some(choice),
                        None => return false,
                    }
                }
                Some(_) => {}
                None if issue.confirm.is_none() => {
                    refused.push([&name[..], b": ", issue.reason.as_bytes()].concat())
                }
                None => {}
            }
        }
    }
    request
        .resolutions
        .retain(|_, resolution| *resolution != PathResolution::default());

    if !refused.is_empty() {
        let mut err = io::stderr().lock();
        let _ = err.write_all(
            &[
                format!("These cannot be {}: ", operation.past).as_bytes(),
                &refused.join(&b"; "[..]),
                b"\n",
            ]
            .concat(),
        );
        drop(err);
        if !yes(format!("Skip them and {} the rest?", operation.verb)) {
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

/// A yes-or-no answer from the terminal, no unless said. The question is bytes: it may name a path
/// that is not UTF-8.
pub(crate) fn yes(question: impl AsRef<[u8]>) -> bool {
    matches!(
        line([question.as_ref(), b" [y/N] "].concat()).as_str(),
        "y" | "yes"
    )
}

/// One line typed at the terminal, trimmed. The prompt is bytes: it may name a path that is not UTF-8.
pub(crate) fn line(prompt: impl AsRef<[u8]>) -> String {
    let mut err = io::stderr().lock();
    let _ = err.write_all(prompt.as_ref());
    let _ = err.flush();
    drop(err);
    let mut answer = String::new();
    let _ = io::stdin().lock().read_line(&mut answer);

    answer.trim().to_owned()
}
