//! `keepster find` and `keepster grep` (ticket 59): a path found by its name across every Root's history —
//! a file long deleted included — and text found in every version a Root's history holds, streamed as the
//! daemon finds it.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use keepster_api::{ContentRecord, PathSearch, Root, encode_path_query};

use crate::client::DaemonClient;
use crate::command::{FindArgs, GrepArgs};
use crate::exit;
use crate::roots::{answered, body_of};

/// Find a path by its name in the history of every Root, or of those named.
pub async fn find(args: &FindArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);
    let mut url = format!("/v1/search/paths?q={}", encode_path_query(&args.query));
    if args.glob {
        url.push_str("&glob=true");
    }
    if let Some(limit) = args.limit {
        url.push_str(&format!("&limit={limit}"));
    }
    if !args.root.is_empty() {
        let mut ids = Vec::new();
        for selector in &args.root {
            match crate::resolve::resolve(&client, Some(selector)).await {
                Ok(root) => ids.push(root.id),
                Err(code) => return code,
            }
        }
        url.push_str(&format!("&roots={}", ids.join(",")));
    }

    if args.json {
        return match body_of(client.get(&url).await) {
            Ok(body) => {
                let mut out = io::stdout().lock();
                let _ = out.write_all(&body);
                let _ = writeln!(out);
                exit::SUCCESS
            }
            Err(code) => code,
        };
    }
    let found: PathSearch = match answered(client.get(&url).await) {
        Ok(found) => found,
        Err(code) => return code,
    };
    let places = places(&client).await;

    let mut out = io::stdout().lock();
    for hit in &found.hits {
        let plural = if hit.snapshots == 1 { "" } else { "s" };
        let _ = write!(
            out,
            "{}  {} Snapshot{plural}, {} .. {}",
            on_disk(&places, &hit.root, &hit.path).display(),
            hit.snapshots,
            hit.first,
            hit.last
        );
        if !hit.exists {
            let _ = write!(out, "  (gone from the newest Snapshot)");
        }
        let _ = writeln!(out);
    }
    let _ = out.flush();
    if found.hits.is_empty() {
        eprintln!("keepster: no path in any history matches `{}`", args.query);
    }
    if found.truncated {
        eprintln!("keepster: there are more; narrow the query, or pass a larger --limit");
    }
    for unsearched in &found.unsearched {
        eprintln!(
            "keepster: the Root {} was not searched: {}",
            unsearched.root, unsearched.detail
        );
    }

    exit::SUCCESS
}

/// Find text in every version a Root's history holds — the Root holding the working directory, those
/// named, or every one.
pub async fn grep(args: &GrepArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);
    let mut url = format!("/v1/search/content?q={}", encode_path_query(&args.pattern));
    if args.regex {
        url.push_str("&regex=true");
    }
    let single = if args.all_roots {
        url.push_str("&all=true");
        None
    } else {
        let root = match crate::resolve::resolve(&client, args.root.as_deref()).await {
            Ok(root) => root,
            Err(code) => return code,
        };
        url.push_str(&format!("&roots={}", root.id));
        if let Some(path) = &args.path {
            match crate::restore::within_root(&root, path) {
                Ok(relative) => {
                    url.push_str(&format!("&path={}", encode_path_query(&relative)));
                }
                Err(code) => return code,
            }
        }
        Some(root)
    };

    let mut lines = match client.lines(&url).await {
        Ok(lines) => lines,
        // Refused, unreachable or broken off: said as every other command says it.
        Err(error) => return body_of(Err(error)).err().unwrap_or(exit::OTHER),
    };
    let places = if single.is_some() {
        BTreeMap::new()
    } else {
        places(&client).await
    };

    let mut out = io::stdout().lock();
    let mut ended = false;
    loop {
        let line = match lines.next().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                eprintln!("keepster: the search broke off: {error}");
                return exit::OTHER;
            }
        };
        if args.json {
            let _ = writeln!(out, "{line}");
        }
        // A record this build does not know is passed over, as an event is.
        let Ok(record) = serde_json::from_str::<ContentRecord>(&line) else {
            continue;
        };
        if matches!(record, ContentRecord::Summary { .. }) {
            ended = true;
        }
        if !args.json {
            render(&record, single.is_some(), &places, &mut out);
        }
    }
    let _ = out.flush();
    if !ended {
        eprintln!("keepster: the search ended before its summary");
        return exit::OTHER;
    }

    exit::SUCCESS
}

/// One record for a person: a match on standard output, everything else on standard error.
fn render(
    record: &ContentRecord,
    one_root: bool,
    places: &BTreeMap<String, String>,
    out: &mut impl Write,
) {
    match record {
        ContentRecord::Plan { roots } => {
            let versions: u64 = roots.iter().map(|root| root.versions).sum();
            let bytes: u64 = roots.iter().map(|root| root.bytes).sum();
            let plural = if roots.len() == 1 { "" } else { "s" };
            eprintln!(
                "keepster: searching {versions} versions, {}, in {} Root{plural}",
                crate::preview::size(bytes),
                roots.len()
            );
        }
        ContentRecord::Match {
            root,
            path,
            first,
            last,
            line,
            snippet,
            ..
        } => {
            let shown = if one_root {
                native(PathBuf::new(), path)
            } else {
                on_disk(places, root, path)
            };
            let span = if first == last {
                first.clone()
            } else {
                format!("{first} .. {last}")
            };
            let _ = writeln!(out, "{}:{line}: {snippet}  [{span}]", shown.display());
        }
        ContentRecord::Withheld {
            path, reason, kind, ..
        } => {
            let why = match kind {
                Some(kind) => format!("{reason}, {kind}"),
                None => reason.clone(),
            };
            eprintln!("keepster: {path} was not searched: it is withheld ({why})");
        }
        ContentRecord::Error { root, detail } => {
            eprintln!("keepster: the Root {root} was not searched: {detail}");
        }
        ContentRecord::Progress { .. } => {}
        ContentRecord::Summary {
            versions_scanned,
            binaries_skipped,
            withheld,
            unreadable,
        } => {
            eprintln!(
                "keepster: {versions_scanned} versions searched, {binaries_skipped} binary skipped, \
                 {withheld} withheld paths not searched"
            );
            if *unreadable > 0 {
                eprintln!("keepster: {unreadable} versions could not be read");
            }
        }
    }
}

/// Where each watched Root is, by identity.
async fn places(client: &DaemonClient) -> BTreeMap<String, String> {
    let roots: Vec<Root> = client
        .get("/v1/roots")
        .await
        .ok()
        .and_then(|response| serde_json::from_slice(&response.body).ok())
        .unwrap_or_default();

    roots.into_iter().map(|root| (root.id, root.path)).collect()
}

/// `path` in the Root `root` as a path on this machine — or beside the Root's identity, for one no longer
/// watched, which is nowhere on disk.
fn on_disk(places: &BTreeMap<String, String>, root: &str, path: &str) -> PathBuf {
    let base = places
        .get(root)
        .map_or_else(|| PathBuf::from(root), PathBuf::from);

    native(base, path)
}

/// The Root-relative `path` beneath `base`, spelled with this machine's separator.
fn native(base: PathBuf, path: &str) -> PathBuf {
    path.split('/').fold(base, |joined, name| joined.join(name))
}
