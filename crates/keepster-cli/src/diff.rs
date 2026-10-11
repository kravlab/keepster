//! `keepster diff`: the daemon's diff, as a person reads it or as the patch text itself.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::time::SystemTime;

use keepster_api::{Diff, FileChange, encode_path_query};

use crate::client::{ClientError, DaemonClient};
use crate::command::DiffArgs;
use crate::exit;
use crate::history::{located, resolve_snapshot};
use crate::roots::{answered, body_of};
use crate::selector;

/// The `--to` that names the Working Tree, as the API spells it.
const WORKING: &str = "working";

/// Show what changed between two versions of a Root.
///
/// The daemon computes every line; this prints what it answered — the summary rendered, the unified text
/// and the JSON passed through untouched — so every client shows the same diff.
pub async fn diff(args: &DiffArgs, control_path: &Path, now: SystemTime) -> u8 {
    let client = DaemonClient::new(control_path);

    let (root, relative) = match located(&client, args.root.as_deref(), args.path.as_deref()).await
    {
        Ok(found) => found,
        Err(code) => return code,
    };
    // The Root itself is every path, which is what no path means.
    let relative = relative.filter(|relative| !relative.is_empty());

    let from = match side(
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
    let to = match args.to.as_deref() {
        None | Some(WORKING) => WORKING.to_owned(),
        Some(selected) => match side(&client, &root, selected, now).await {
            Ok(to) => to,
            Err(code) => return code,
        },
    };

    let unified = !args.json && (args.patch || relative.is_some());
    let mut url = format!(
        "/v1/roots/{}/diff?from={from}&to={to}&format={}",
        root.id,
        if unified { "unified" } else { "json" }
    );
    if let Some(relative) = &relative {
        url.push_str(&format!("&path={}", encode_path_query(relative)));
    }

    let mut out: Box<dyn Write + Send> = match &args.output {
        Some(path) => match File::create(path) {
            Ok(file) => Box::new(io::BufWriter::new(file)),
            Err(error) => {
                eprintln!("keepster: {} could not be written: {error}", path.display());
                return exit::OTHER;
            }
        },
        None => Box::new(io::stdout()),
    };

    let written = if unified || args.json {
        // Passed through as the daemon sent it: the patch is bytes, not text to re-encode.
        let answer = client.get_to(&url, &mut out).await;
        if let Err(ClientError::Output(error)) = &answer {
            if error.kind() == io::ErrorKind::BrokenPipe {
                return exit::SUCCESS;
            }
        }
        if let Err(code) = body_of(answer) {
            return code;
        }
        // JSON ends its line, as `log --json` does; a patch already ends with one, or is empty.
        if args.json { writeln!(out) } else { Ok(()) }
    } else {
        let diff: Diff = match answered(client.get(&url).await) {
            Ok(diff) => diff,
            Err(code) => return code,
        };
        write_summary(&diff, &mut out)
    };

    match written.and_then(|()| out.flush()) {
        Ok(()) => exit::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => exit::SUCCESS,
        Err(error) => {
            eprintln!("keepster: could not write the diff: {error}");
            exit::OTHER
        }
    }
}

/// One side of the comparison, as a Snapshot's identity.
async fn side(
    client: &DaemonClient,
    root: &keepster_api::Root,
    selected: &str,
    now: SystemTime,
) -> Result<String, u8> {
    let selected = selector::parse(selected, now).map_err(|message| {
        eprintln!("keepster: {message}");
        exit::USAGE
    })?;

    resolve_snapshot(client, root, &selected).await
}

/// One line per changed path, with a letter for how it changed, as `git diff --name-status` does — and
/// the words that keep a binary or oversized file from reading as an ordinary change.
fn write_summary(diff: &Diff, out: &mut impl Write) -> io::Result<()> {
    for file in &diff.files {
        let letter = match file.change {
            Some(FileChange::Added) => 'A',
            Some(FileChange::Modified) => 'M',
            Some(FileChange::Deleted) => 'D',
            Some(FileChange::Renamed) => 'R',
            None => ' ',
        };
        match &file.old_path {
            Some(old) => write!(out, "{letter}  {old} -> {}", file.path)?,
            None => write!(out, "{letter}  {}", file.path)?,
        }
        if file.binary {
            write!(out, "  (binary)")?;
        }
        if file.too_large {
            write!(out, "  (too large to diff)")?;
        }
        writeln!(out)?;
    }

    Ok(())
}
