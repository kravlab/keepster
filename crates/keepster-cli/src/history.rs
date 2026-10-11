//! `keepster log` and `keepster cat`: the timeline and one version's bytes, as a person reads them.

use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use keepster_api::{Change, Root, Snapshot, Timeline, encode_path_query};
use serde::Deserialize;

use crate::client::{ClientError, DaemonClient};
use crate::command::{CatArgs, LogArgs};
use crate::exit;
use crate::roots::{answered, body_of};
use crate::selector::{self, Selector};

/// The most the API returns in one page (ticket 07); a longer listing follows the cursor.
const PAGE: usize = 1_000;

/// List a Root's Snapshots newest first — or, given a path, the Snapshots that changed it.
pub async fn log(args: &LogArgs, control_path: &Path, now: SystemTime) -> u8 {
    let client = DaemonClient::new(control_path);

    let (root, relative) = match located(&client, args.root.as_deref(), args.path.as_deref()).await
    {
        Ok(found) => found,
        Err(code) => return code,
    };

    let mut filters = String::new();
    if let Some(relative) = relative.as_deref().filter(|relative| !relative.is_empty()) {
        filters.push_str(&format!("&path={}", encode_path_query(relative)));
    }
    for (name, value) in [("before", &args.before), ("after", &args.after)] {
        let Some(value) = value else { continue };
        match selector::moment(value, now) {
            Ok(moment) => filters.push_str(&format!(
                "&{name}={}",
                encode_path_query(&moment.to_string())
            )),
            Err(message) => {
                eprintln!("keepster: --{name}: {message}");
                return exit::USAGE;
            }
        }
    }

    // Items are kept as the daemon sent them, so `--json` passes through every field — including one
    // this build was never taught — rather than whatever survives a round trip through our types.
    let mut items: Vec<serde_json::Value> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let wanted = args
            .limit
            .map_or(PAGE, |limit| limit.saturating_sub(items.len()));
        if wanted == 0 {
            break;
        }
        let mut url = format!(
            "/v1/roots/{}/snapshots?limit={}{filters}",
            root.id,
            wanted.min(PAGE)
        );
        if let Some(cursor) = &cursor {
            url.push_str(&format!("&cursor={cursor}"));
        }

        let page: serde_json::Value = match answered(client.get(&url).await) {
            Ok(page) => page,
            Err(code) => return code,
        };
        let next = page["next_cursor"].as_str().map(ToOwned::to_owned);
        if let Some(found) = page["snapshots"].as_array() {
            items.extend(found.iter().cloned());
        }
        match next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    write_log(&items, args.json, &mut io::stdout().lock())
}

/// Write the listing and return the exit code it deserves.
///
/// A reader that stopped early — `keepster log | head -1` — closed the pipe on purpose, so that is
/// success, as it is for `cat`; a script under `pipefail` must not fail because it read enough.
fn write_log(items: &[serde_json::Value], json: bool, out: &mut impl Write) -> u8 {
    let written = if json {
        serde_json::to_writer(&mut *out, items)
            .map_err(io::Error::from)
            .and_then(|()| writeln!(out))
    } else {
        items
            .iter()
            .try_for_each(|item| match Snapshot::deserialize(item) {
                Ok(snapshot) => write_snapshot(&snapshot, out),
                Err(error) => {
                    eprintln!(
                        "keepster: a Snapshot in the daemon's answer could not be read: {error}"
                    );
                    Ok(())
                }
            })
    };

    match written.and_then(|()| out.flush()) {
        Ok(()) => exit::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => exit::SUCCESS,
        Err(error) => {
            eprintln!("keepster: could not write the answer: {error}");
            exit::OTHER
        }
    }
}

/// Write one version of a file to standard output, byte for byte.
pub async fn cat(args: &CatArgs, control_path: &Path, now: SystemTime) -> u8 {
    let client = DaemonClient::new(control_path);

    let (root, relative) = match located(&client, args.root.as_deref(), Some(&args.path)).await {
        Ok(found) => found,
        Err(code) => return code,
    };
    let Some(relative) = relative.filter(|relative| !relative.is_empty()) else {
        eprintln!(
            "keepster: {} is the Root itself, not a file",
            args.path.display()
        );
        return exit::USAGE;
    };

    let selected = match selector::parse(args.snapshot.as_deref().unwrap_or("latest"), now) {
        Ok(selected) => selected,
        Err(message) => {
            eprintln!("keepster: {message}");
            return exit::USAGE;
        }
    };
    let snapshot = match resolve_snapshot(&client, &root, &selected).await {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };

    let url = format!(
        "/v1/roots/{}/snapshots/{snapshot}/content?path={}",
        root.id,
        encode_path_query(&relative)
    );
    let mut out = io::stdout();
    let answer = client.get_to(&url, &mut out).await;
    if let Err(ClientError::Output(error)) = &answer {
        // A reader that stopped early — `keepster cat big.log | head` — is not a failure worth a
        // message, and anything else at least says what went wrong.
        if error.kind() == io::ErrorKind::BrokenPipe {
            return exit::SUCCESS;
        }
    }

    match body_of(answer) {
        Ok(_) => exit::SUCCESS,
        Err(code) => code,
    }
}

/// Reduce a selector to one Snapshot's identity through ordinary timeline queries.
async fn resolve_snapshot(
    client: &DaemonClient,
    root: &Root,
    selected: &Selector,
) -> Result<String, u8> {
    let base = format!("/v1/roots/{}/snapshots", root.id);

    let (query, skip, described) = match selected {
        Selector::Id(id) => return Ok(id.clone()),
        Selector::Latest => (String::new(), 0, "no Snapshot yet".to_owned()),
        Selector::Back(count) => (
            String::new(),
            *count,
            format!("fewer than {} Snapshots", count.saturating_add(1)),
        ),
        Selector::AsOf(moment) => (
            format!("&before={}", encode_path_query(&moment.to_string())),
            0,
            format!("no Snapshot at or before {moment}"),
        ),
    };

    // `~N` is the (N+1)th newest, so it may lie beyond the first page; the cursor carries on.
    let mut remaining = skip;
    let mut cursor: Option<String> = None;
    loop {
        let wanted = usize::try_from(remaining.saturating_add(1))
            .unwrap_or(PAGE)
            .min(PAGE);
        let mut url = format!("{base}?limit={wanted}{query}");
        if let Some(cursor) = &cursor {
            url.push_str(&format!("&cursor={cursor}"));
        }

        let page: Timeline = answered(client.get(&url).await)?;
        let count = page.snapshots.len() as u64;
        if remaining < count {
            let index = usize::try_from(remaining).expect("less than one page");
            return Ok(page.snapshots[index].id.clone());
        }
        remaining -= count;

        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => {
                eprintln!("keepster: {} has {described}", root.path);
                return Err(exit::NOT_FOUND);
            }
        }
    }
}

/// The Root a command is about and, when it was given a path, that path relative to the Root.
///
/// An explicit `--root` wins; otherwise the path decides, and otherwise the working directory — the
/// same order every Root-scoped command uses (ticket 07).
pub(crate) async fn located(
    client: &DaemonClient,
    root: Option<&str>,
    path: Option<&Path>,
) -> Result<(Root, Option<String>), u8> {
    // Made absolute and then rid of `.` and `..` by name, as the shell that typed the path reads them:
    // `std::path::absolute` keeps `..` on Unix, so `../notes.md` from a subdirectory would otherwise
    // never be found inside the Root it is in.
    let absolute = match path
        .map(|path| std::path::absolute(path).map(|path| lexical(&path)))
        .transpose()
    {
        Ok(absolute) => absolute,
        Err(error) => {
            eprintln!("keepster: the path could not be resolved: {error}");
            return Err(exit::OTHER);
        }
    };

    let root = match (root, &absolute) {
        (Some(selector), _) => crate::resolve::resolve(client, Some(selector)).await?,
        (None, Some(absolute)) => {
            crate::resolve::by_path(client, &absolute.to_string_lossy()).await?
        }
        (None, None) => crate::resolve::resolve(client, None).await?,
    };

    let Some(absolute) = absolute else {
        return Ok((root, None));
    };
    match relative_to(&absolute, Path::new(&root.path)) {
        Some(relative) => Ok((root, Some(relative))),
        None => {
            eprintln!(
                "keepster: {} is not inside the Root at {}",
                absolute.display(),
                root.path
            );
            Err(exit::USAGE)
        }
    }
}

/// `path` with `.` and `..` resolved by name, without touching the filesystem.
///
/// By name, as a shell's own `cd ..` does: the file may be one that no longer exists, so there may be
/// nothing on disk to resolve against, and a Root is registered by the path the person typed rather
/// than through its symlinks. A `..` at the filesystem root stays there.
fn lexical(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    resolved.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    resolved.pop();
                }
            }
            other => resolved.push(other),
        }
    }

    resolved
}

/// `path` relative to `root`, `/`-separated as every endpoint takes it — the separators of this
/// platform are the client's to normalise (ticket 07).
fn relative_to(path: &Path, root: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => parts.push(name.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }

    Some(parts.join("/"))
}

/// One timeline line: identity, moment, Trigger, what changed — and the words that keep an
/// empty-looking line from reading as "nothing happened".
fn write_snapshot(snapshot: &Snapshot, out: &mut impl Write) -> io::Result<()> {
    write!(
        out,
        "{}  {}  {}",
        snapshot.id, snapshot.taken_at, snapshot.trigger
    )?;
    match snapshot.change {
        Some(change) => write!(out, "  {}", change_word(change))?,
        None => match snapshot.changed_paths {
            Some(0) => write!(out, "  no changes")?,
            Some(1) => write!(out, "  1 path changed")?,
            Some(count) => write!(out, "  {count} paths changed")?,
            None => write!(out, "  changes not counted")?,
        },
    }
    if snapshot.incomplete {
        write!(out, "  [incomplete]")?;
    }

    writeln!(out)
}

fn change_word(change: Change) -> &'static str {
    match change {
        Change::Added => "added",
        Change::Modified => "modified",
        Change::Deleted => "deleted",
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};

    use super::{lexical, relative_to, write_log};
    use crate::exit;

    /// A reader that has gone away, as `head` has once it read enough.
    struct ClosedPipe;

    impl Write for ClosedPipe {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    fn item() -> serde_json::Value {
        serde_json::json!({
            "id": "01K6DX3QR7M2V8C9WZ4PN5YQ1A",
            "segment": "2026-09-30T14",
            "taken_at": "2026-09-30T14:37:05Z",
            "trigger": "manual",
            "incomplete": false,
            "changed_paths": 1,
        })
    }

    #[test]
    fn a_reader_that_stopped_early_is_not_a_failure() {
        assert_eq!(write_log(&[item()], false, &mut ClosedPipe), exit::SUCCESS);
        assert_eq!(write_log(&[item()], true, &mut ClosedPipe), exit::SUCCESS);
    }

    #[test]
    fn dot_dot_climbs_out_of_a_subdirectory_by_name() {
        assert_eq!(
            lexical(Path::new("/proj/sub/../notes.md")),
            PathBuf::from("/proj/notes.md")
        );
        assert_eq!(
            lexical(Path::new("/proj/./a/b/../../c")),
            PathBuf::from("/proj/c")
        );
        assert_eq!(lexical(Path::new("/../x")), PathBuf::from("/x"));
    }

    #[test]
    fn a_climbed_path_is_still_inside_its_root() {
        let path = lexical(Path::new("/proj/sub/../notes.md"));

        assert_eq!(
            relative_to(&path, Path::new("/proj")),
            Some("notes.md".to_owned())
        );
        assert_eq!(
            relative_to(&lexical(Path::new("/proj/../other")), Path::new("/proj")),
            None
        );
    }
}
