//! `keepster preview` and `keepster watch --dry-run` (ticket 57): what a Snapshot of a directory would
//! hold, right now, rendered as the daemon streams it — and, before registering a directory past what one
//! Root is claimed to hold, the confirmation that registering it is meant.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use keepster_api::{PreviewConfig, PreviewGroup, PreviewRecord, PreviewSize, RuleReference};

use crate::client::{ClientError, DaemonClient};
use crate::command::PreviewArgs;
use crate::exit;
use crate::names::{printed_all, write_path};
use crate::roots::exit_code_for;

/// What a person is told about the one number a preview never gives.
const NO_GROWTH: &str = "Store growth is not estimated: only the capture itself can know what the \
     history store will hold.";

/// Preview a directory: the one given, or else the Root holding the working directory, found as every
/// command finds it (ticket 07), or else the working directory itself, as a directory that is not a Root
/// yet. A directory inside a Root is classified by that Root's rules.
pub async fn preview(args: &PreviewArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);
    let directory = match &args.path {
        Some(path) => match std::path::absolute(path) {
            Ok(path) => path,
            Err(error) => {
                eprintln!(
                    "keepster: {} could not be resolved: {error}",
                    path.display()
                );
                return exit::OTHER;
            }
        },
        None => match std::env::current_dir() {
            Ok(here) => holding_root(&client, &here).await.unwrap_or(here),
            Err(error) => {
                eprintln!("keepster: the working directory could not be read: {error}");
                return exit::OTHER;
            }
        },
    };
    let mut query = String::new();
    if let Some(depth) = &args.depth {
        query.push_str(&format!(
            "&depth={}",
            keepster_api::encode_path_query(depth)
        ));
    }
    if let Some(only) = &args.only {
        query.push_str(&format!("&only={}", keepster_api::encode_path_query(only)));
    }
    if args.measure_excluded {
        query.push_str("&measure_excluded=true");
    }
    if let Some(top) = args.top {
        query.push_str(&format!("&top={top}"));
    }

    match run(&client, &directory, &query, Shown::Rendered(args.json)).await {
        Ok(_) => exit::SUCCESS,
        Err(code) => code,
    }
}

/// How a preview's records reach the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shown {
    /// Every record as it arrives: verbatim JSON when `true`, rendered otherwise.
    Rendered(bool),
    /// Nothing, only the summary returned: to decide whether to say anything at all. A failure is
    /// `STATE_CONFLICT` when another preview is walking, and `OTHER` whatever else it is.
    Quiet,
}

/// Preview `directory`, showing it as `shown` says, and return its summary record.
pub(crate) async fn run(
    client: &DaemonClient,
    directory: &Path,
    query: &str,
    shown: Shown,
) -> Result<PreviewRecord, u8> {
    let url = format!(
        "/v1/preview?path={}{query}",
        keepster_api::encode_path_query(directory.to_string_lossy().as_bytes())
    );
    let mut lines = match client.lines(&url).await {
        Ok(lines) => lines,
        // Quiet says nothing: whatever stopped the preview, the command that asked for it says it.
        Err(ClientError::Refused(response)) if shown == Shown::Quiet => {
            let busy = serde_json::from_slice::<keepster_api::Problem>(&response.body)
                .is_ok_and(|problem| problem.code == keepster_api::ErrorCode::PreviewBusy.as_str());
            return Err(if busy {
                exit::STATE_CONFLICT
            } else {
                exit::OTHER
            });
        }
        Err(_) if shown == Shown::Quiet => return Err(exit::OTHER),
        Err(ClientError::Refused(response)) => {
            let problem = serde_json::from_slice::<keepster_api::Problem>(&response.body).ok();
            if let Some(problem) = &problem {
                eprintln!("keepster: {}", problem.detail);
            }
            return Err(exit_code_for(problem.as_ref(), response.status.as_u16()));
        }
        // Unreachable, or broken off: said as every other command says it.
        Err(error) => {
            return Err(crate::roots::body_of(Err(error))
                .err()
                .unwrap_or(exit::OTHER));
        }
    };

    let mut out = io::stdout().lock();
    let mut summary = None;
    loop {
        let line = match lines.next().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                if shown != Shown::Quiet {
                    eprintln!("keepster: the preview broke off: {error}");
                }
                return Err(exit::OTHER);
            }
        };
        if shown == Shown::Rendered(true) {
            let _ = writeln!(out, "{line}");
        }
        // A record this build does not know is passed over, as an event is.
        let Ok(record) = serde_json::from_str::<PreviewRecord>(&line) else {
            continue;
        };
        if shown == Shown::Rendered(false) {
            let _ = render(&record, &mut out);
        }
        match record {
            PreviewRecord::Summary { .. } => summary = Some(record),
            PreviewRecord::Error { detail } => {
                if shown != Shown::Quiet {
                    eprintln!("keepster: the preview stopped: {detail}");
                }
                return Err(exit::OTHER);
            }
            _ => {}
        }
    }

    summary.ok_or_else(|| {
        if shown != Shown::Quiet {
            eprintln!("keepster: the preview ended before its summary");
        }
        exit::OTHER
    })
}

/// The Root holding `directory`, as the daemon resolves a path to its Root — and nothing, quietly, when
/// none does or the daemon cannot say: the preview that follows says whatever is wrong.
async fn holding_root(client: &DaemonClient, directory: &Path) -> Option<PathBuf> {
    let response = client
        .get(&format!(
            "/v1/roots?path={}",
            keepster_api::encode_path_query(directory.to_string_lossy().as_bytes())
        ))
        .await
        .ok()?;
    if !response.status.is_success() {
        return None;
    }
    let found: Vec<keepster_api::Root> = serde_json::from_slice(&response.body).ok()?;

    found
        .into_iter()
        .next()
        .map(|root| PathBuf::from(root.path))
}

/// Render one record for a person.
pub(crate) fn render(record: &PreviewRecord, out: &mut impl Write) -> io::Result<()> {
    match record {
        PreviewRecord::Header {
            path,
            root,
            within,
            within_bytes,
            overlaps,
            config,
        } => {
            write!(out, "Capture Preview of {path}")?;
            match (root, within, overlaps) {
                (Some(root), Some(within), _) => {
                    write!(out, " (")?;
                    write_path(out, within, within_bytes.as_deref())?;
                    writeln!(out, " in the Root {root}, classified by the Root's rules)")?
                }
                (Some(root), None, _) => writeln!(out, " (the Root {root})")?,
                (None, _, Some(other)) => writeln!(
                    out,
                    " (not a Root; it overlaps the Root {other}, so watching it would be refused)"
                )?,
                (None, _, None) => writeln!(out, " (not a Root)")?,
            }
            if let Some(config) = config {
                render_config(config, out)?;
            }
            Ok(())
        }
        PreviewRecord::Entry {
            path,
            path_bytes,
            class,
            reason,
            bytes,
            rule,
            pruned,
            ..
        } => {
            let what = match reason {
                Some(reason) => format!("{class} {reason}"),
                None => class.clone(),
            };
            let decided = rule
                .as_ref()
                .map(|rule| format!("  — {}", rule_text(rule)))
                .unwrap_or_default();
            if *pruned {
                write!(out, "  {what:<12} ")?;
                write_path(out, path, path_bytes.as_deref())?;
                writeln!(out, "/ not entered{decided}")
            } else {
                write!(out, "  {what:<12} {:>10}  ", size(*bytes))?;
                write_path(out, path, path_bytes.as_deref())?;
                writeln!(out, "{decided}")
            }
        }
        // The walk's pulse, for a reader that wants one; a person has the entries.
        PreviewRecord::Progress { .. } => Ok(()),
        // Said on the error stream by whoever reads the records.
        PreviewRecord::Error { .. } => Ok(()),
        PreviewRecord::Summary {
            captured,
            excluded,
            withheld,
            unreadable,
            groups,
            largest_captured,
            largest_left_out,
            past_ceiling,
        } => {
            writeln!(
                out,
                "captured:   {} files, {} directories, {}",
                captured.files,
                captured.directories,
                size(captured.bytes)
            )?;
            write!(
                out,
                "excluded:   {} files, {}",
                excluded.files,
                size(excluded.bytes)
            )?;
            if excluded.pruned > 0 {
                write!(out, ", and {} directories not entered", excluded.pruned)?;
            }
            writeln!(out)?;
            for (reason, counted) in withheld {
                writeln!(
                    out,
                    "withheld:   {} paths, {} — {reason}",
                    counted.files + counted.directories,
                    size(counted.bytes)
                )?;
            }
            if *unreadable > 0 {
                writeln!(out, "unreadable: {unreadable} paths")?;
            }
            render_groups(groups, out)?;
            render_largest("Largest captured", largest_captured, out)?;
            render_largest("Largest left out", largest_left_out, out)?;
            if !past_ceiling.is_empty() {
                writeln!(
                    out,
                    "This is past what one Root is claimed to hold, in {}.",
                    past_ceiling.join(", ")
                )?;
            }
            writeln!(out, "{NO_GROWTH}")
        }
    }
}

/// The directory's own file: what it sets, so that a rule a repository's author wrote is never read as
/// Keepster's own default.
fn render_config(config: &PreviewConfig, out: &mut impl Write) -> io::Result<()> {
    if config.sets.is_empty() {
        writeln!(out, "Its own {} sets nothing.", config.file)?;
    } else {
        writeln!(
            out,
            "Its own {} sets, honoured here exactly as watching it would honour it:",
            config.file
        )?;
        for setting in &config.sets {
            let values: Vec<String> = setting.values.iter().map(crate::config::shown).collect();
            writeln!(out, "  {} = {}", setting.key, values.join(", "))?;
        }
    }
    for warning in &config.warnings {
        writeln!(out, "  warning: {}", warning.message)?;
    }
    Ok(())
}

/// Every rule that decided something, and what it decided; what no rule decided is in the totals.
fn render_groups(groups: &[PreviewGroup], out: &mut impl Write) -> io::Result<()> {
    let decided: Vec<&PreviewGroup> = groups.iter().filter(|group| group.rule.is_some()).collect();
    if decided.is_empty() {
        return Ok(());
    }
    writeln!(out, "By the rule that decided:")?;
    for group in decided {
        let rule = group.rule.as_ref().map(rule_text).unwrap_or_default();
        if group.files + group.directories > 0 {
            writeln!(
                out,
                "  {:<9} {} files, {} — {rule}",
                group.class,
                group.files,
                size(group.bytes)
            )?;
        }
        for pruned in printed_all(&group.pruned, &group.pruned_bytes) {
            write!(out, "  {:<9} ", group.class)?;
            out.write_all(&pruned)?;
            writeln!(out, "/ not entered — {rule}")?;
        }
    }
    Ok(())
}

/// One side's largest files, under `title`, largest first.
fn render_largest(title: &str, side: &[PreviewSize], out: &mut impl Write) -> io::Result<()> {
    if side.is_empty() {
        return Ok(());
    }
    writeln!(out, "{title}:")?;
    for entry in side {
        write!(out, "  {:>10}  ", size(entry.bytes))?;
        write_path(out, &entry.path, entry.path_bytes.as_deref())?;
        writeln!(out)?;
    }
    Ok(())
}

/// A rule as a person reads it: the pattern, and where it was written.
fn rule_text(rule: &RuleReference) -> String {
    format!(
        "{} ({})",
        rule.pattern,
        crate::explain::where_set(&rule.origin)
    )
}

/// A size as a person reads it, in binary units.
pub(crate) fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::size;

    #[test]
    fn a_size_reads_in_binary_units() {
        assert_eq!(size(40), "40 B");
        assert_eq!(size(1536), "1.5 KiB");
        assert_eq!(size(10 * 1024 * 1024 * 1024), "10.0 GiB");
    }
}
