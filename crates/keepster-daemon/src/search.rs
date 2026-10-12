//! Search across Snapshots over `/v1` (ticket 59, ticket 34): two operations, because every property a
//! client can observe differs between them. A path search is a lookup in an index that already exists —
//! JSON, inside the chronology budget, across every Root by default. A content search is a walk over every
//! distinct version stored — NDJSON, with no time promised, one Root unless all are asked for, stopped when
//! its reader goes away.

use std::ops::ControlFlow;

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use keepster_api::{
    ContentRecord, ErrorCode, PathHit, PathSearch, Problem, SearchCost, Unsearched,
};
use keepster_app::snapshots::{self, StoreLocation};
use keepster_core::{ContentQuery, PathQuery, RootId, RootPath};
use keepster_store::{ContentFound, ContentPlan, HistoryStore, SearchCounts};
use tokio::sync::mpsc;

use crate::DaemonState;
use crate::problem::ProblemResponse;
use crate::snapshots::{located, parse_path};

/// How many paths a path search answers with unless asked for another number.
const PATHS: usize = 100;

/// The most paths a path search answers with, however many are asked for.
const MOST_PATHS: usize = 1000;

/// How many versions a content search looks at between two `progress` records.
const PROGRESS_EVERY: u64 = 100;

/// What a path search asks.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct PathParameters {
    /// A fragment of the path, matched anywhere in it — case-insensitively unless it has a capital.
    q: Option<String>,
    /// `true` to read `q` as an Include Rules pattern, as the configuration writes them: `**/*.env`.
    glob: Option<bool>,
    /// The Roots to search, by identity, comma-separated; every Root when absent — those no longer
    /// watched whose history is kept included.
    roots: Option<String>,
    /// How many paths to answer with: 100 unless given, and at most 1000.
    limit: Option<String>,
}

/// Find a path by its name in the history of every Root, a file long deleted included.
#[utoipa::path(
    get,
    path = "/v1/search/paths",
    operation_id = "search_paths",
    tag = "snapshots",
    params(PathParameters),
    responses(
        (status = 200, description = "Every path any Snapshot changed that the query finds, by Root and then by path: how many Snapshots changed it, the first and the last, and whether the newest Snapshot still holds it. Answered from the index, inside the chronology budget.", body = PathSearch),
        (status = 400, description = "`q` is missing or empty, or not a pattern, or the query string cannot be read (`query_invalid`); `limit` is not a positive whole number (`limit_invalid`).", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "A Root asked for is not known.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The Registry could not be read, so which Roots there are is unknown (`registry_failed`).", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn paths(
    State(state): State<DaemonState>,
    parameters: Result<Query<PathParameters>, QueryRejection>,
) -> Result<Json<PathSearch>, ProblemResponse> {
    let Query(parameters) = parameters.map_err(invalid_query)?;
    let query = PathQuery::new(
        parameters.q.as_deref().unwrap_or_default(),
        parameters.glob.unwrap_or(false),
    )
    .map_err(|error| query_invalid(&error.to_string()))?;
    let limit = match parameters.limit.as_deref() {
        None => PATHS,
        Some(text) => match text.parse::<usize>() {
            Ok(0) | Err(_) => {
                return Err(ProblemResponse::new(
                    StatusCode::BAD_REQUEST,
                    ErrorCode::LimitInvalid,
                    "`limit` must be a positive whole number",
                )
                .with_detail("limit", text));
            }
            // Capped rather than refused, as a page of the chronology is.
            Ok(limit) => limit.min(MOST_PATHS),
        },
    };
    let roots = searchable(&state, parameters.roots.as_deref())?;

    crate::snapshots::blocking(move || Ok(find(&roots, &query, limit)))
        .await
        .map(Json)
}

/// The paths `query` finds in each of `roots`, at most `limit` in all.
fn find(roots: &[(RootId, StoreLocation)], query: &PathQuery, limit: usize) -> PathSearch {
    let mut search = PathSearch {
        hits: Vec::new(),
        truncated: false,
        unsearched: Vec::new(),
    };
    for (root, location) in roots {
        let found = snapshots::open_for_reading(location).and_then(|store| {
            store
                .map(|store| store.find_paths(query, limit))
                .transpose()
        });
        match found {
            Ok(Some((hits, more))) => {
                search.truncated |= more;
                search.hits.extend(hits.into_iter().map(|hit| PathHit {
                    root: root.to_string(),
                    path: hit.path,
                    snapshots: hit.snapshots,
                    first: hit.first.to_string(),
                    last: hit.last.to_string(),
                    exists: hit.exists,
                }));
            }
            // Never captured: nothing to find, and nothing wrong.
            Ok(None) => {}
            Err(error) => search.unsearched.push(Unsearched {
                root: root.to_string(),
                detail: error.to_string(),
            }),
        }
    }
    if search.hits.len() > limit {
        search.hits.truncate(limit);
        search.truncated = true;
    }

    search
}

/// What a content search asks.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ContentParameters {
    /// What to find in the content: a literal, matched case-insensitively unless it has a capital.
    q: Option<String>,
    /// `true` to read `q` as a regular expression, matched in linear time.
    regex: Option<bool>,
    /// The Roots to search, by identity, comma-separated. One is the usual: a content search reads every
    /// version stored, so asking for every Root is said with `all`.
    roots: Option<String>,
    /// `true` to search every Root — those no longer watched whose history is kept included.
    all: Option<bool>,
    /// A file or directory, Root-relative: only what is at or beneath it is read.
    path: Option<String>,
}

/// Find text in every version a Root's history holds, streamed as it is found.
#[utoipa::path(
    get,
    path = "/v1/search/content",
    operation_id = "search_content",
    tag = "snapshots",
    params(ContentParameters),
    responses(
        (status = 200, description = "An `application/x-ndjson` stream: a `plan` record first, saying how many distinct versions and bytes each Root will have read; then `match` records — one per content version, a path and the range of Snapshots its content was current for, never one per Snapshot — and `withheld` records for paths with no content to search, with `progress` now and then; and a `summary` last, counting versions scanned, binaries skipped and Withheld paths met. A version is read once however many Snapshots held it; a binary one is skipped and counted; the metadata sidecar is never searched. Outside the read budget: no time is promised, and closing the connection stops the walk.", body = ContentRecord, content_type = "application/x-ndjson"),
        (status = 400, description = "`q` is missing or empty, or not a regular expression that compiles, or the query string cannot be read (`query_invalid`); no Root named and `all` not asked (`search_invalid`); or `path` is malformed.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "A Root asked for is not known.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The Registry could not be read, so which Roots there are is unknown (`registry_failed`).", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn content(
    State(state): State<DaemonState>,
    parameters: Result<Query<ContentParameters>, QueryRejection>,
) -> Result<Response, ProblemResponse> {
    let Query(parameters) = parameters.map_err(invalid_query)?;
    let query = ContentQuery::new(
        parameters.q.as_deref().unwrap_or_default(),
        parameters.regex.unwrap_or(false),
    )
    .map_err(|error| query_invalid(&error.to_string()))?;
    let roots = match (parameters.all, parameters.roots.as_deref()) {
        (Some(true), _) => searchable(&state, None)?,
        (_, Some(roots)) if !roots.trim().is_empty() => searchable(&state, Some(roots))?,
        _ => {
            return Err(ProblemResponse::new(
                StatusCode::BAD_REQUEST,
                ErrorCode::SearchInvalid,
                "a content search reads every version a Root holds: name the Root with `roots`, or \
                 ask for every one with `all=true`",
            ));
        }
    };
    let under = parse_path(parameters.path.as_deref().unwrap_or_default())?;

    let (lines, response) = crate::ndjson::stream();
    tokio::task::spawn_blocking(move || {
        search(&roots, &query, &under, &lines, crate::ndjson::STALL);
    });

    Ok(response)
}

/// Search `roots` for `query` at or beneath `under`, sending what is found down `lines` — and stopping the
/// moment no one is reading them.
fn search(
    roots: &[(RootId, StoreLocation)],
    query: &ContentQuery,
    under: &RootPath,
    lines: &mpsc::Sender<Bytes>,
    stall: std::time::Duration,
) {
    let send = |record: &ContentRecord| crate::ndjson::send(lines, record, stall);

    // Every plan first, so that the cost is said before any of it is paid.
    let mut planned: Vec<(RootId, HistoryStore, ContentPlan)> = Vec::new();
    let mut failed = Vec::new();
    for (root, location) in roots {
        if lines.is_closed() {
            return;
        }
        let opened = snapshots::open_for_reading(location).and_then(|store| {
            store
                .map(|store| store.content_plan(under).map(|plan| (store, plan)))
                .transpose()
        });
        match opened {
            Ok(Some((store, plan))) => planned.push((*root, store, plan)),
            Ok(None) => {}
            Err(error) => failed.push((*root, error.to_string())),
        }
    }
    let costs = planned
        .iter()
        .map(|(root, _, plan)| SearchCost {
            root: root.to_string(),
            versions: plan.versions(),
            bytes: plan.bytes,
        })
        .collect();
    if !send(&ContentRecord::Plan { roots: costs }) {
        return;
    }
    for (root, detail) in failed {
        if !send(&ContentRecord::Error {
            root: root.to_string(),
            detail,
        }) {
            return;
        }
    }

    let mut total = SearchCounts::default();
    let mut withheld = 0;
    for (root, store, plan) in &planned {
        let root = root.to_string();
        for met in &plan.withheld {
            withheld += 1;
            let record = ContentRecord::Withheld {
                root: root.clone(),
                path: met.path.clone(),
                reason: met.reason.as_str().to_owned(),
                kind: met.reason.kind().map(|kind| kind.as_str().to_owned()),
                snapshot: met.snapshot.to_string(),
            };
            if !send(&record) {
                return;
            }
        }

        let versions = plan.versions();
        let mut done = 0_u64;
        let mut reading = true;
        let counts = store.search_content(plan, query, &mut |found| {
            let going = match found {
                ContentFound::Match {
                    version,
                    blob,
                    line,
                } => send(&ContentRecord::Match {
                    root: root.clone(),
                    path: version.path.clone(),
                    content: blob,
                    first: version.first.to_string(),
                    last: version.last.to_string(),
                    line: line.line,
                    snippet: line.snippet.clone(),
                }),
                ContentFound::Scanned => {
                    done += 1;
                    !done.is_multiple_of(PROGRESS_EVERY)
                        || send(&ContentRecord::Progress {
                            root: root.clone(),
                            done,
                            total: versions,
                        })
                }
            };
            reading = going && !lines.is_closed();
            if reading {
                ControlFlow::Continue(())
            } else {
                ControlFlow::Break(())
            }
        });
        if !reading
            || !send(&ContentRecord::Progress {
                root: root.clone(),
                done,
                total: versions,
            })
        {
            return;
        }
        total.scanned += counts.scanned;
        total.binary += counts.binary;
        total.unreadable += counts.unreadable;
    }

    send(&ContentRecord::Summary {
        versions_scanned: total.scanned,
        binaries_skipped: total.binary,
        withheld,
        unreadable: total.unreadable,
    });
}

/// The Roots `asked` names, comma-separated — or, with none named, every Root: the watched ones and those
/// no longer watched whose history is kept, each with where its history lives.
fn searchable(
    state: &DaemonState,
    asked: Option<&str>,
) -> Result<Vec<(RootId, StoreLocation)>, ProblemResponse> {
    if let Some(asked) = asked.filter(|asked| !asked.trim().is_empty()) {
        let mut roots = Vec::new();
        for id in asked.split(',').map(str::trim).filter(|id| !id.is_empty()) {
            let found = located(state, id)?;
            if !roots.iter().any(|(root, _)| *root == found.0) {
                roots.push(found);
            }
        }
        return Ok(roots);
    }

    // A Registry that cannot be read fails the search: an empty answer would say nothing was found where
    // nothing was looked at.
    let registry = state.registry();
    let mut ids: Vec<RootId> = registry
        .readable()
        .map_err(crate::roots::registry_problem)?
        .into_iter()
        .chain(
            registry
                .unwatched()
                .map_err(crate::roots::registry_problem)?,
        )
        .map(|record| record.id)
        .collect();
    ids.sort();
    ids.dedup();

    Ok(ids
        .into_iter()
        .map(|id| (id, snapshots::location(&registry.root_dir(id))))
        .collect())
}

/// A query string that cannot be read at all.
fn invalid_query(rejection: QueryRejection) -> ProblemResponse {
    query_invalid(&rejection.body_text())
}

/// The problem with a query.
fn query_invalid(detail: &str) -> ProblemResponse {
    ProblemResponse::new(StatusCode::BAD_REQUEST, ErrorCode::QueryInvalid, detail)
}

#[cfg(test)]
mod tests {
    //! The guard a content search owes the daemon, held to directly rather than through sockets whose
    //! buffers no test can size: no walk outliving its reader.

    use std::time::{Duration, Instant, SystemTime};

    use keepster_app::snapshots::StoreLocation;
    use keepster_core::{CapturePolicy, ContentQuery, RootId, RootPath, Trigger};
    use keepster_store::HistoryStore;
    use tokio::sync::mpsc;

    use super::search;

    /// A Root whose one Snapshot holds a file of 500 matching lines, and where its history lives.
    fn history() -> (tempfile::TempDir, RootId, StoreLocation) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path().join("project");
        std::fs::create_dir_all(&root).expect("the Root");
        std::fs::write(root.join("notes.txt"), "a needle\n".repeat(500)).expect("a file");
        let location = StoreLocation {
            store: directory.path().join("history.git"),
            index: directory.path().join("index.sqlite"),
        };
        let id = RootId::generate(SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_779_025));
        let store = HistoryStore::open(&location.store, &location.index).expect("a store");
        store
            .capture(
                id,
                &root,
                &CapturePolicy::default(),
                Trigger::Manual,
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_779_085),
            )
            .expect("a Snapshot");

        (directory, id, location)
    }

    #[test]
    fn a_search_stops_at_once_when_its_reader_goes_away() {
        let (_directory, id, location) = history();
        let query = ContentQuery::new("needle", false).expect("a query");
        let (lines, mut reader) = mpsc::channel(1);

        let started = Instant::now();
        let searching = std::thread::spawn(move || {
            search(
                &[(id, location)],
                &query,
                &RootPath::root(),
                &lines,
                Duration::from_secs(60),
            );
        });
        // The plan, then the reader goes away with every match still to come.
        let plan = reader.blocking_recv().expect("the plan");
        assert!(String::from_utf8_lossy(&plan).contains("\"plan\""));
        drop(reader);
        searching.join().expect("the search ends");

        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no line waits a minute for a reader that is gone"
        );
    }

    #[test]
    fn a_search_gives_up_on_a_reader_that_stopped_reading() {
        let (_directory, id, location) = history();
        let query = ContentQuery::new("needle", false).expect("a query");
        // Room for the plan alone, and a reader that never reads.
        let (lines, mut reader) = mpsc::channel(1);

        let started = Instant::now();
        search(
            &[(id, location)],
            &query,
            &RootPath::root(),
            &lines,
            Duration::from_millis(100),
        );

        assert!(
            started.elapsed() < Duration::from_secs(30),
            "500 matches do not wait a tenth of a second each"
        );
        let plan = reader.try_recv().expect("the plan was sent");
        assert!(String::from_utf8_lossy(&plan).contains("\"plan\""));
        assert!(
            reader.try_recv().is_err(),
            "nothing after the line nobody took"
        );
    }
}
