//! A Capture Preview over `/v1` (ticket 57, ticket 20): what a Snapshot of a directory would hold, right
//! now, streamed as the walk classifies it — for a Root, and for a directory that is not one yet.
//!
//! It is not a job, because nothing happens: a dropped connection costs a re-run, so the walk stops the
//! moment its reader goes away. It opens no History Store, takes no store lock, is no Trigger and captures
//! nothing, and one runs at a time, a cheap guard on an I/O-heavy read.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use keepster_api::{
    ErrorCode, PreviewConfig, PreviewGroup, PreviewRecord, PreviewSetting, PreviewSize,
    PreviewTotals, Problem,
};
use keepster_core::{
    CapturePolicy, PreviewClass, PreviewEntry, PreviewKind, PreviewSummary, ROOT_CEILING, RootId,
    RootPath, Tally, Totals, WithheldReason,
};
use tokio::sync::mpsc;

use crate::DaemonState;
use crate::problem::ProblemResponse;

/// How many of the largest entries a summary names on each side, unless asked otherwise.
const TOP: usize = 10;

/// The most of the largest entries a summary names on each side, however many are asked for: each is
/// held in memory until the walk ends.
const MOST: usize = 1000;

/// How many paths are classified between two `progress` records.
const PROGRESS_EVERY: u64 = 1000;

/// What to preview, and how much of it to list.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct PreviewParameters {
    /// The directory, absolute: a Root, or a directory that is not one.
    path: String,
    /// `1`, the default, sends the summary alone; `infinity` sends an entry for every path as well.
    depth: Option<String>,
    /// Send an entry for every path of this class: `captured`, `excluded`, `withheld` or `unreadable`.
    only: Option<String>,
    /// Enter excluded directories to measure what they hold, which a capture never does; `depth=infinity`
    /// implies it, since listing what is inside a directory means entering it.
    measure_excluded: Option<bool>,
    /// How many of the largest entries the summary names on each side: 10 unless given, and at most
    /// 1000.
    top: Option<usize>,
}

/// Classify a directory against the rules that would govern it, without capturing anything.
#[utoipa::path(
    get,
    path = "/v1/preview",
    operation_id = "preview_directory",
    tag = "roots",
    params(PreviewParameters),
    responses(
        (status = 200, description = "An `application/x-ndjson` stream: a `header` record at once, an `entry` record for every path asked for as the walk classifies it with a `progress` record now and then, then one `summary` — or an `error`, if the walk fails. Closing the connection stops the walk. A directory inside a Root is classified by that Root's rules. No store growth is estimated: only the capture itself could know it.", body = PreviewRecord, content_type = "application/x-ndjson"),
        (status = 400, description = "`path` is not an absolute directory, or `depth`, `only` or `top` is malformed.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "A preview is already walking, or the directory's configuration cannot be read.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The preview could not be prepared (`preview_failed`).", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn preview(
    State(state): State<DaemonState>,
    parameters: Result<Query<PreviewParameters>, QueryRejection>,
) -> Result<Response, ProblemResponse> {
    let Query(parameters) = parameters.map_err(|rejection| {
        ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::PreviewInvalid,
            rejection.body_text(),
        )
    })?;
    let listing = Listing::from(&parameters)?;
    let path = crate::roots::absolute_query_path(&parameters.path)?;
    if !path.is_dir() {
        return Err(ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::PathInvalid,
            format!("{} is not a directory", path.display()),
        )
        .with_detail("path", path.to_string_lossy()));
    }

    let walking = Walking::claim(&state.previewing).ok_or_else(|| {
        ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::PreviewBusy,
            "a Capture Preview is already walking; one runs at a time",
        )
    })?;
    let prepared = tokio::task::spawn_blocking({
        let state = state.clone();
        move || prepared(&state, &path)
    })
    .await
    .map_err(|error| {
        ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::PreviewFailed,
            format!("the preview could not be prepared: {error}"),
        )
    })??;

    let (lines, response) = crate::ndjson::stream();
    tokio::task::spawn_blocking(move || {
        // Released when the walk ends, however it ends.
        let _walking = walking;
        walk(&prepared, &listing, &lines, crate::ndjson::STALL);
    });

    Ok(response)
}

/// The one preview walking, held for as long as it walks.
struct Walking(Arc<AtomicBool>);

impl Walking {
    /// The slot, unless a preview already holds it.
    fn claim(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(Arc::clone(flag)))
    }
}

impl Drop for Walking {
    /// Free the slot for the next preview.
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Which paths get a record of their own, and what the summary keeps.
#[derive(Debug, Clone, Copy)]
struct Listing {
    /// Every path, as `depth=infinity` asks.
    every: bool,
    /// Every path of one class, whatever `depth` says.
    only: Option<Only>,
    /// Whether excluded directories are entered to be measured.
    measure_excluded: bool,
    /// How many of the largest entries the summary names on each side.
    top: usize,
}

/// A class of path, as `only` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Only {
    /// Content to capture.
    Captured,
    /// Left out by a rule or the secret denylist.
    Excluded,
    /// Recorded without its content.
    Withheld,
    /// Not looked at.
    Unreadable,
}

impl Listing {
    /// What `parameters` ask for, or the problem with them.
    fn from(parameters: &PreviewParameters) -> Result<Self, ProblemResponse> {
        let invalid = |message: String| {
            ProblemResponse::new(StatusCode::BAD_REQUEST, ErrorCode::PreviewInvalid, message)
        };
        let every = match parameters.depth.as_deref() {
            None | Some("1") => false,
            Some("infinity") => true,
            Some(other) => {
                return Err(ProblemResponse::new(
                    StatusCode::BAD_REQUEST,
                    ErrorCode::DepthInvalid,
                    format!("`depth` is `1` or `infinity`, not `{other}`"),
                ));
            }
        };
        let only = match parameters.only.as_deref() {
            None => None,
            Some("captured") => Some(Only::Captured),
            Some("excluded") => Some(Only::Excluded),
            Some("withheld") => Some(Only::Withheld),
            Some("unreadable") => Some(Only::Unreadable),
            Some(other) => {
                return Err(invalid(format!(
                    "`only` is `captured`, `excluded`, `withheld` or `unreadable`, not `{other}`"
                )));
            }
        };

        Ok(Self {
            every,
            only,
            measure_excluded: every || parameters.measure_excluded.unwrap_or(false),
            top: parameters.top.unwrap_or(TOP).min(MOST),
        })
    }

    /// Whether `entry` gets a record of its own.
    fn lists(&self, entry: &PreviewEntry) -> bool {
        match self.only {
            Some(only) => only == class_of(entry.class),
            None => self.every,
        }
    }
}

/// The class `only` names a path of `class` by.
fn class_of(class: PreviewClass) -> Only {
    match class {
        PreviewClass::Captured => Only::Captured,
        PreviewClass::Excluded => Only::Excluded,
        PreviewClass::Withheld(_) => Only::Withheld,
        PreviewClass::Unreadable => Only::Unreadable,
    }
}

/// What a preview walks, and under which rules.
struct Prepared {
    /// The first record.
    header: PreviewRecord,
    /// The rules a capture would classify by.
    policy: CapturePolicy,
    /// The directory paths are relative to: the Root's, when the directory is or lies inside one.
    root: PathBuf,
    /// Where the walk starts beneath `root`.
    start: RootPath,
}

/// What previewing `path` walks: as part of the Root it is or lies inside, by that Root's configuration,
/// or else as registering it would configure it — its own `.keepster.toml` included.
///
/// Resolved while the Registry is held, as everywhere: resolving may write into a Root's store directory,
/// which an erasing deregistration must not be removing at the same moment.
fn prepared(state: &DaemonState, path: &Path) -> Result<Prepared, ProblemResponse> {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let registry = state.registry();
    // Without the Roots, neither an overlap nor another Root's fence could be known: a preview
    // without them would show what registering could never do.
    let unlisted = |error: keepster_store::RegistryError| {
        ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::RegistryFailed,
            error.to_string(),
        )
    };
    let records = registry.readable().map_err(unlisted)?;
    let holding = records
        .iter()
        .find_map(|record| Some((record, crate::places::inside(&absolute, &record.path)?)));

    let (effective, root, start, overlaps) = match holding {
        Some((record, start)) => {
            let mut effective = state.effective(record.id, &record.path)?;
            // The ignore files above where the walk starts, which it does not pass through; its own it
            // reads as it enters.
            if !start.is_root() {
                effective.policy =
                    keepster_app::config::policy_for_path(&effective.policy, &record.path, &start);
            }
            (effective, Some(record), start, None)
        }
        None => {
            // A directory watched once is registered again under the identity it had.
            let unwatched = registry
                .unwatched()
                .map_err(unlisted)?
                .into_iter()
                .rev()
                .find(|record| record.path == absolute)
                .map(|record| record.id);
            let effective = state
                .config()
                .candidate(&absolute, unwatched)
                .map_err(|errors| crate::problem::config_problem(&errors))?;
            let paths: Vec<(RootId, PathBuf)> = records
                .iter()
                .map(|other| (other.id, other.path.clone()))
                .collect();
            let overlaps = keepster_core::overlapping(&absolute, &paths);
            (effective, None, RootPath::root(), overlaps)
        }
    };
    drop(registry);

    // As a capture is fenced: nothing of another Root is ever entered.
    let fences = records
        .iter()
        .filter(|other| Some(other.id) != root.map(|root| root.id))
        .map(|other| {
            let real = std::fs::canonicalize(&other.path).unwrap_or_else(|_| other.path.clone());
            (real, WithheldReason::OtherRoot)
        })
        .collect();
    let policy = effective.policy.clone().with_real_fences(fences);
    let walked = root.map_or_else(|| absolute.clone(), |root| root.path.clone());

    let file = walked.join(keepster_app::config::ROOT_CONFIG);
    let config = file.is_file().then(|| PreviewConfig {
        file: file.to_string_lossy().into_owned(),
        sets: keepster_app::config::set_by_root_file(&effective)
            .into_iter()
            .map(|(key, values)| PreviewSetting {
                key: key.to_owned(),
                values: values.into_iter().map(crate::configuration::wire).collect(),
            })
            .collect(),
        warnings: effective
            .warnings
            .iter()
            .cloned()
            .map(crate::explain::warning)
            .collect(),
    });
    let header = PreviewRecord::Header {
        path: absolute.to_string_lossy().into_owned(),
        root: root.map(|root| root.id.to_string()),
        within: (!start.is_root()).then(|| crate::wire::text(&start)),
        within_bytes: crate::wire::companion(&start),
        overlaps: overlaps.map(|id| id.to_string()),
        config,
    };

    Ok(Prepared {
        header,
        policy,
        root: walked,
        start,
    })
}

/// Walk what `prepared` names, sending its header, the entries `listing` asks for, progress now and then
/// and the summary down `lines` — and stopping the moment no one is reading them.
///
/// A reader that stops reading is given up on after `stall`, [`crate::ndjson::STALL`] but in a test.
fn walk(prepared: &Prepared, listing: &Listing, lines: &mpsc::Sender<Bytes>, stall: Duration) {
    let send = |record: &PreviewRecord| crate::ndjson::send(lines, record, stall);
    if !send(&prepared.header) {
        return;
    }

    let mut tally = Tally::new(listing.top);
    let mut walked = 0_u64;
    let outcome = keepster_store::preview(
        &prepared.root,
        &prepared.start,
        &prepared.policy,
        &keepster_store::PreviewOptions {
            measure_excluded: listing.measure_excluded,
        },
        &mut |entry| {
            tally.add(&entry);
            walked += 1;
            let listed = !listing.lists(&entry) || send(&entry_record(&entry));
            let alive =
                !walked.is_multiple_of(PROGRESS_EVERY) || send(&PreviewRecord::Progress { walked });
            if listed && alive && !lines.is_closed() {
                ControlFlow::Continue(())
            } else {
                ControlFlow::Break(())
            }
        },
    );
    if lines.is_closed() {
        return;
    }
    match outcome {
        Ok(()) => {
            send(&summary_record(&tally.finish()));
        }
        Err(error) => {
            tracing::warn!(path = %prepared.root.display(), %error, "a Capture Preview could not walk the directory");
            send(&PreviewRecord::Error {
                detail: error.to_string(),
            });
        }
    }
}

/// The record of one classified path.
fn entry_record(entry: &PreviewEntry) -> PreviewRecord {
    PreviewRecord::Entry {
        path: crate::wire::text(&entry.path),
        path_bytes: crate::wire::companion(&entry.path),
        kind: kind_name(entry.kind).to_owned(),
        class: class_name(entry.class).to_owned(),
        reason: reason_of(entry.class),
        bytes: entry.bytes,
        rule: entry.rule.as_ref().map(crate::explain::reference),
        pruned: entry.pruned,
    }
}

/// The record of what the whole walk found.
fn summary_record(summary: &PreviewSummary) -> PreviewRecord {
    let sizes = |side: &[(keepster_core::RootPath, u64)]| {
        side.iter()
            .map(|(path, bytes)| PreviewSize {
                path: crate::wire::text(path),
                path_bytes: crate::wire::companion(path),
                bytes: *bytes,
            })
            .collect()
    };

    PreviewRecord::Summary {
        captured: totals(summary.captured),
        excluded: totals(summary.excluded),
        withheld: summary
            .withheld
            .iter()
            .map(|(reason, counted)| (reason.as_str().to_owned(), totals(*counted)))
            .collect(),
        unreadable: summary.unreadable,
        groups: summary
            .groups
            .iter()
            .map(|group| PreviewGroup {
                class: class_name(group.class).to_owned(),
                reason: reason_of(group.class),
                rule: group.rule.as_ref().map(crate::explain::reference),
                files: group.files,
                directories: group.directories,
                bytes: group.bytes,
                pruned: crate::wire::texts(&group.pruned),
                pruned_bytes: crate::wire::companions(&group.pruned),
            })
            .collect(),
        largest_captured: sizes(&summary.largest_captured),
        largest_left_out: sizes(&summary.largest_left_out),
        past_ceiling: ROOT_CEILING
            .past(summary)
            .into_iter()
            .map(ToOwned::to_owned)
            .collect(),
    }
}

/// Totals as the wire carries them.
fn totals(counted: Totals) -> PreviewTotals {
    PreviewTotals {
        files: counted.files,
        directories: counted.directories,
        bytes: counted.bytes,
        pruned: counted.pruned,
    }
}

/// What a path is, as an entry names it.
fn kind_name(kind: PreviewKind) -> &'static str {
    match kind {
        PreviewKind::File => "file",
        PreviewKind::Directory => "directory",
        PreviewKind::Link => "link",
        PreviewKind::Special => "special",
    }
}

/// How a path was classified, as an entry names it.
fn class_name(class: PreviewClass) -> &'static str {
    match class {
        PreviewClass::Captured => "captured",
        PreviewClass::Excluded => "excluded",
        PreviewClass::Withheld(_) => "withheld",
        PreviewClass::Unreadable => "unreadable",
    }
}

/// Why a Withheld path is Withheld, and nothing for any other.
fn reason_of(class: PreviewClass) -> Option<String> {
    match class {
        PreviewClass::Withheld(reason) => Some(reason.as_str().to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! The two guards a preview owes the daemon, held to directly rather than through sockets whose
    //! buffers no test can size: one walk at a time, and no walk outliving its reader.

    use std::time::{Duration, Instant};

    use keepster_api::PreviewRecord;
    use keepster_core::{CapturePolicy, RootPath};
    use tokio::sync::mpsc;

    use super::{Listing, Only, Prepared, Walking, walk};

    /// A directory of `files` files, and what previewing it walks.
    fn prepared(files: usize) -> (tempfile::TempDir, Prepared) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        for at in 0..files {
            std::fs::write(directory.path().join(format!("{at:04}.txt")), b"x").expect("a file");
        }
        let prepared = Prepared {
            header: PreviewRecord::Header {
                path: directory.path().to_string_lossy().into_owned(),
                root: None,
                within: None,
                within_bytes: None,
                overlaps: None,
                config: None,
            },
            policy: CapturePolicy::default(),
            root: directory.path().to_path_buf(),
            start: RootPath::root(),
        };

        (directory, prepared)
    }

    /// Every path listed, as `depth=infinity` asks.
    const EVERY: Listing = Listing {
        every: true,
        only: None::<Only>,
        measure_excluded: true,
        top: 10,
    };

    #[test]
    fn one_walk_holds_the_slot_until_it_ends_however_it_ends() {
        let slot = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        let first = Walking::claim(&slot).expect("the slot is free");
        assert!(Walking::claim(&slot).is_none(), "a second walk is refused");
        drop(first);
        assert!(
            Walking::claim(&slot).is_some(),
            "released when the walk ends"
        );

        let slot = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let held = Walking::claim(&slot).expect("the slot is free");
        let ended = std::thread::spawn(move || {
            let _held = held;
            panic!("a walk that fails");
        })
        .join();
        assert!(ended.is_err());
        assert!(
            Walking::claim(&slot).is_some(),
            "released by a walk that panicked, too"
        );
    }

    #[test]
    fn a_walk_stops_at_once_when_its_reader_goes_away() {
        let (_directory, prepared) = prepared(200);
        let (lines, mut reader) = mpsc::channel(1);

        let started = Instant::now();
        let walking = std::thread::spawn(move || {
            walk(&prepared, &EVERY, &lines, Duration::from_secs(60));
        });
        // The header, then the reader goes away with the walk still under way.
        let header = reader.blocking_recv().expect("the header");
        assert!(String::from_utf8_lossy(&header).contains("\"header\""));
        drop(reader);
        walking.join().expect("the walk ends");

        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no line waits a minute for a reader that is gone"
        );
    }

    #[test]
    fn a_walk_gives_up_on_a_reader_that_stopped_reading() {
        let (_directory, prepared) = prepared(200);
        // Room for the header alone, and a reader that never reads.
        let (lines, mut reader) = mpsc::channel(1);

        let started = Instant::now();
        walk(&prepared, &EVERY, &lines, Duration::from_millis(100));

        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the walk ends, so its slot is free for the next"
        );
        let header = reader.try_recv().expect("the header was sent");
        assert!(String::from_utf8_lossy(&header).contains("\"header\""));
        assert!(
            reader.try_recv().is_err(),
            "nothing after the line nobody took"
        );
    }
}
