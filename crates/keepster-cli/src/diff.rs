//! `keepster diff`: the daemon's diff, as a person reads it or as the patch text itself.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use keepster_api::{Diff, FileChange, WORKING_TREE, encode_path_query};

use crate::client::{ClientError, DaemonClient};
use crate::command::DiffArgs;
use crate::exit;
use crate::history::{located, selected};
use crate::roots::{answered, body_of};

/// Show what changed from a Snapshot to another Snapshot or the Working Tree.
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
    let to = match args.to.as_deref() {
        None | Some(WORKING_TREE) => WORKING_TREE.to_owned(),
        Some(typed) => match selected(&client, &root, typed, now).await {
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

    // A file is written beside its destination and put in place only once the whole answer is in it,
    // so a refusal or a failure leaves whatever was there before rather than an empty or partial file.
    let partial = args.output.as_deref().map(partial_path);
    let mut out: Box<dyn Write + Send> = match &partial {
        Some(partial) => match File::create(partial) {
            Ok(file) => Box::new(io::BufWriter::new(file)),
            Err(error) => {
                eprintln!(
                    "keepster: {} could not be written: {error}",
                    partial.display()
                );
                return exit::OTHER;
            }
        },
        None => Box::new(io::stdout()),
    };

    let code = write_answer(&client, &url, unified, args.json, &mut out).await;
    drop(out);
    if let (Some(partial), Some(destination)) = (&partial, &args.output) {
        if code != exit::SUCCESS {
            let _ = fs::remove_file(partial);
        } else if let Err(error) = fs::rename(partial, destination) {
            eprintln!(
                "keepster: {} could not be written: {error}",
                destination.display()
            );
            let _ = fs::remove_file(partial);
            return exit::OTHER;
        }
    }

    code
}

/// Write the daemon's answer to `out` and return the exit code it deserves.
async fn write_answer(
    client: &DaemonClient,
    url: &str,
    unified: bool,
    json: bool,
    out: &mut Box<dyn Write + Send>,
) -> u8 {
    let written = if unified || json {
        // Passed through as the daemon sent it: the patch is bytes, not text to re-encode.
        let answer = client.get_to(url, out.as_mut()).await;
        if let Err(ClientError::Output(error)) = &answer {
            if error.kind() == io::ErrorKind::BrokenPipe {
                return exit::SUCCESS;
            }
        }
        if let Err(code) = body_of(answer) {
            return code;
        }
        // JSON ends its line, as `log --json` does; a patch already ends with one, or is empty.
        if json { writeln!(out) } else { Ok(()) }
    } else {
        let diff: Diff = match answered(client.get(url).await) {
            Ok(diff) => diff,
            Err(code) => return code,
        };
        write_summary(&diff, out)
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

/// Where `-o` writes before the answer is whole: beside the destination, so the rename stays on one
/// filesystem.
fn partial_path(destination: &Path) -> PathBuf {
    let mut name = destination.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");

    destination.with_file_name(name)
}

/// One line per changed path, with a letter for how it changed, as `git diff --name-status` does — and
/// the words that keep a binary, oversized, withheld or re-moded file from reading as an ordinary change.
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
        if let Some(reason) = &file.withheld {
            write!(out, "  (still there, withheld: {reason})")?;
        }
        if let (Some(old), Some(new)) = (file.old_mode, file.new_mode) {
            write!(out, "  (mode {old:o} -> {new:o})")?;
        }
        writeln!(out)?;
    }
    for omitted in &diff.omitted {
        eprintln!("keepster: {omitted} could not be read; its last captured content is compared");
    }

    Ok(())
}
