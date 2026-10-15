//! Export at the API (ticket 58, ticket 19): a Snapshot's versions written by the daemon into a directory
//! the client names — planned at once, written as a job — and the two ways of handing bytes to a browser
//! instead: a selection's size before anything is done, and its archive.
//!
//! The daemon and its clients share one machine (ticket 05), which is what makes a destination named by
//! a web page meaningful at all, and the endpoint no more privileged than any other.

use std::io;
use std::path::PathBuf;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use keepster_api::{
    ErrorCode, ExportPlan, ExportRequest, JobAccepted, JobResult, Problem, SelectionSize, Withheld,
};
use keepster_app::export::{self, DestinationRoot, ExportError};
use keepster_app::snapshots::{self, StoreLocation};
use keepster_core::{
    ArchiveEntry, ArchiveWriter, Crc32, RootId, RootPath, SnapshotId, VolumeVerdict, archive_size,
};
use keepster_store::{Selected, Selection, StoreError};
use tokio::sync::{mpsc, oneshot};

use crate::DaemonState;
use crate::path_parameter::PathParameter;
use crate::problem::ProblemResponse;
use crate::restore::{confirmed_by, mode_choice, resolutions_of, wire};
use crate::roots::RootAddress;
use crate::snapshots::{
    SnapshotAddress, located, parse_snapshot, prepare, store_problem, withheld_reason,
};

/// What an Export job is called.
const EXPORT: &str = "export";

/// How many chunks of an archive may wait for a slow client.
const IN_FLIGHT: usize = 4;

/// Write a Snapshot's versions into a directory the request names, leaving the Working Tree untouched.
#[utoipa::path(
    post,
    path = "/v1/roots/{id}/export",
    operation_id = "export_snapshot",
    tag = "jobs",
    params(RootAddress),
    request_body = ExportRequest,
    responses(
        (status = 200, description = "A dry run: what would happen to every file, and the selection's size; nothing written.", body = ExportPlan),
        (status = 202, description = "Planned, and queued as a job that plans again under the stores' locks and then writes each file to a temporary name beside it, renamed into place, with the Snapshot's permission bits — on Windows, none. Missing directories are made. A Withheld path is absent from the destination and its outcome says `skipped`, with its reason. Inside a Root, anything overwritten is first held by that Root's Safety Snapshot, named in the job's result.", body = JobAccepted,
            headers(("Location" = String, description = "The job, at `/v1/jobs/{id}`."))),
        (status = 400, description = "`confirmation_required`: some file needs a confirmation (`overwrite`, `replace_type`, `dest_in_root`, `overwrite_unrecoverable`, or `\"all\"`) or a resolution (`write`, `rename_to`, `name_case`, `modes`), or cannot be written (`type_conflict`, `link_check_failed`); `paths` says what each needs and nothing was written. `dest_in_store`: the destination is inside a History Store. Otherwise a malformed path, destination or resolution.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root, Snapshot or path in the Snapshot.", body = Problem, content_type = "application/problem+json"),
        (status = 409, description = "`dest_unwritable`: the destination, or the nearest directory above it that is there, is not a directory. Otherwise the Root the destination lies inside is unavailable, or its configuration has never been valid.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The history could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn export(
    State(state): State<DaemonState>,
    Path(RootAddress { id }): Path<RootAddress>,
    body: Result<Json<ExportRequest>, JsonRejection>,
) -> Result<Response, ProblemResponse> {
    let Json(request) = body.map_err(|rejection| {
        ProblemResponse::new(
            rejection.status(),
            ErrorCode::BodyInvalid,
            rejection.body_text(),
        )
    })?;
    let (id, location) = located(&state, &id)?;
    let fingerprint = serde_json::to_string(&request).expect("a request serialises");
    let dry_run = request.dry_run;
    let mut parsed = parsed(request)?;
    let dest = parsed.dest.clone();

    let (planned, into, parsed) = tokio::task::spawn_blocking({
        let (state, location) = (state.clone(), location.clone());
        move || {
            let into = destination(&state, &parsed.dest)?;
            parsed.beneath = into.beneath.clone();
            let planned =
                export::plan(&location, &parsed, into.root.as_ref()).map_err(export_problem)?;
            Ok::<_, ProblemResponse>((planned, into, parsed))
        }
    })
    .await
    .map_err(joined)??;

    if dry_run {
        return Ok(Json(ExportPlan {
            dest: dest.to_string_lossy().into_owned(),
            files: planned.files,
            bytes: planned.bytes,
            paths: wire(&planned.plan, None),
        })
        .into_response());
    }
    if !planned.plan.proceeds() {
        return Err(export_problem(ExportError::Blocked(planned.plan)));
    }

    let also = into.root.as_ref().map(|into| into.record.id);
    let job = crate::jobs::start(&state, EXPORT, fingerprint, id, also, move |state, job| {
        exporting(state, job, &location, &parsed, also)
    });

    Ok(crate::jobs::accepted(job).into_response())
}

/// The request in the domain's terms, or the problem with it.
fn parsed(request: ExportRequest) -> Result<export::ExportRequest, ProblemResponse> {
    let from = parse_snapshot(&request.from)?;
    let paths = crate::wire::requested(&request.paths, &request.paths_bytes)?;
    // Rid of `.` and `..` by name before anything compares it: a destination spelt to climb back into a
    // History Store or a Root is checked as where it leads.
    let dest = crate::places::lexical(&PathBuf::from(&request.dest));
    if !dest.is_absolute() {
        return Err(ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::PathInvalid,
            "`dest` must be absolute: a relative one would be resolved against the daemon's own \
             working directory",
        )
        .with_detail("path", request.dest.as_str()));
    }
    let mut confirmed = confirmed_by(request.confirm);
    if let Some(choice) = request.modes {
        confirmed = confirmed.with_modes(mode_choice(choice));
    }

    Ok(export::ExportRequest {
        from,
        paths,
        dest,
        confirmed,
        resolutions: resolutions_of(&request.resolutions, &request.resolutions_bytes)?,
        skip_unresolvable: request.skip_unresolvable,
        beneath: Vec::new(),
    })
}

/// Where an Export writes, as the Roots see it.
#[derive(Debug, Clone)]
struct Destination {
    /// The Watched Root it lies inside, if it does.
    root: Option<DestinationRoot>,
    /// Every other Watched Root beneath it, as a path in it.
    beneath: Vec<RootPath>,
}

/// Where `dest` is among the Watched Roots — or the problem with writing there at all: inside a History
/// Store, which nothing may write into, or where no directory can be.
fn destination(
    state: &DaemonState,
    dest: &std::path::Path,
) -> Result<Destination, ProblemResponse> {
    let real = crate::places::real(dest).unwrap_or_else(|| dest.to_path_buf());
    let stores = crate::places::real(&state.store_base).unwrap_or_else(|| state.store_base.clone());
    if real.starts_with(&stores) {
        return Err(ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::DestInStore,
            format!(
                "{} is inside a History Store, which nothing may write into",
                dest.display()
            ),
        )
        .with_detail("path", dest.to_string_lossy()));
    }
    // The destination is made where it is missing; what is there already must be a directory.
    if let Some(there) = dest.ancestors().find(|above| above.exists()) {
        if !there.is_dir() {
            return Err(ProblemResponse::new(
                StatusCode::CONFLICT,
                ErrorCode::DestUnwritable,
                format!("{} is not a directory to write into", there.display()),
            )
            .with_detail("path", dest.to_string_lossy()));
        }
    }

    let records = state.registry().readable().map_err(|error| {
        ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::RegistryFailed,
            error.to_string(),
        )
    })?;
    let beneath = records
        .iter()
        .filter_map(|record| crate::places::inside(&record.path, dest))
        .filter(|path| !path.is_root())
        .collect();
    let Some((record, at)) = records
        .iter()
        .find_map(|record| Some((record, crate::places::inside(dest, &record.path)?)))
    else {
        return Ok(Destination {
            root: None,
            beneath,
        });
    };
    // The Root's own rules, with every other Root fenced, as its Safety Snapshot will capture by them.
    let (record, location, loaded) = prepare(state, record.id)?;

    Ok(Destination {
        root: Some(DestinationRoot {
            record,
            location,
            policy: loaded.policy,
            at,
        }),
        beneath,
    })
}

/// The Export itself, under the stores' locks: the destination looked at again, since a Root may have
/// been registered around it while the job waited, then the plan, the Safety Snapshot where one is owed,
/// and the files, announced as they are written.
fn exporting(
    state: &DaemonState,
    job: keepster_core::JobId,
    location: &StoreLocation,
    request: &export::ExportRequest,
    locked: Option<RootId>,
) -> Result<JobResult, ProblemResponse> {
    let found = destination(state, &request.dest)?;
    let request = &export::ExportRequest {
        beneath: found.beneath,
        ..request.clone()
    };
    let into = found.root;
    if let Some(into) = &into {
        // A Root the request did not know about would be written into without its store held.
        if Some(into.record.id) != locked {
            return Err(ProblemResponse::new(
                StatusCode::CONFLICT,
                ErrorCode::RootOverlaps,
                format!(
                    "{} became part of a Root while the export waited; ask again",
                    request.dest.display()
                ),
            ));
        }
        if !into.record.path.is_dir()
            || crate::volumes::verdict(&*state.volumes, &into.record) == VolumeVerdict::Elsewhere
        {
            return Err(crate::volumes::elsewhere(&into.record.path));
        }
    }

    let exported = export::export(
        location,
        request,
        into.as_ref(),
        crate::roots::now(),
        &mut |done, total| crate::jobs::announce_progress(state, job, done, total),
    )
    .map_err(export_problem)?;
    if let (Some(into), Some(captured)) = (&into, &exported.captured) {
        crate::snapshots::announce(state, into.record.id, captured);
    }

    Ok(JobResult {
        dest: Some(request.dest.to_string_lossy().into_owned()),
        safety_snapshot: exported.safety_snapshot.map(|id| id.to_string()),
        paths: wire(&exported.plan, Some(&exported.written)),
        ..JobResult::default()
    })
}

/// Why an Export did not happen, as the one response shape every endpoint uses.
fn export_problem(error: ExportError) -> ProblemResponse {
    match error {
        ExportError::Blocked(plan) => ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::ConfirmationRequired,
            "some files need a confirmation or a resolution, or cannot be exported; nothing was written",
        )
        .with_paths(wire(&plan, None)),
        ExportError::RenameInvalid { ref path, reason } => crate::restore::resolution_invalid(
            path,
            &format!("`rename_to` must be a bare name: {reason}"),
        ),
        ExportError::Capture(error) => crate::snapshots::capture_problem(error),
        ExportError::Store(error) => store_problem(error),
    }
}

/// A blocking task that did not finish, as a failure to read the history.
fn joined(error: tokio::task::JoinError) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::HistoryUnreadable,
        error.to_string(),
    )
}

/// The paths a selection's `path=` asks for: none for the whole Snapshot.
fn selected(path: Option<RootPath>) -> Vec<RootPath> {
    path.filter(|path| !path.is_root()).into_iter().collect()
}

/// How much a selection of a Snapshot holds, so its size is shown before an Export or a download.
#[utoipa::path(
    get,
    path = "/v1/roots/{id}/snapshots/{snapshot}/stat",
    operation_id = "stat_selection",
    tag = "snapshots",
    params(
        SnapshotAddress,
        ("path" = Option<String>, Query, description = "A file or a directory, Root-relative, its bytes percent-encoded; the whole Snapshot when absent."),
    ),
    responses(
        (status = 200, description = "Its files and bytes, the exact size of its archive, and what it withholds.", body = SelectionSize),
        (status = 400, description = "The path is malformed.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root, Snapshot or path.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The history could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn stat(
    State(state): State<DaemonState>,
    Path(SnapshotAddress { id, snapshot }): Path<SnapshotAddress>,
    PathParameter(path): PathParameter,
) -> Result<Json<SelectionSize>, ProblemResponse> {
    let (_, location) = located(&state, &id)?;
    let snapshot = parse_snapshot(&snapshot)?;
    let paths = selected(path);
    let selection =
        crate::snapshots::blocking(move || selection(&location, snapshot, &paths)).await?;

    Ok(Json(SelectionSize {
        files: selection.files.len() as u64,
        bytes: selection
            .files
            .iter()
            .fold(0_u64, |bytes, file| bytes.saturating_add(file.size)),
        archive_bytes: archive_size(&entries(&selection.files)),
        withheld: selection
            .withheld
            .iter()
            .map(|(path, reason)| Withheld {
                path: crate::wire::text(path),
                path_bytes: crate::wire::companion(path),
                why: withheld_reason(*reason),
            })
            .collect(),
    }))
}

/// The selection `paths` of the Snapshot `snapshot`.
fn selection(
    location: &StoreLocation,
    snapshot: SnapshotId,
    paths: &[RootPath],
) -> Result<Selection, StoreError> {
    snapshots::open_for_reading(location)?
        .ok_or(StoreError::SnapshotNotFound(snapshot))?
        .selection(snapshot, paths)
}

/// Every file as the archive holds it.
fn entries(files: &[Selected]) -> Vec<ArchiveEntry> {
    files.iter().map(entry).collect()
}

/// One file as the archive holds it.
fn entry(file: &Selected) -> ArchiveEntry {
    ArchiveEntry {
        name: file.path.as_bytes().to_vec(),
        size: file.size,
        mode: file.mode,
        executable: file.executable,
        link: file.link,
    }
}

/// A selection of a Snapshot as one zip archive, for a client that cannot be given a destination — the
/// browser's download.
#[utoipa::path(
    get,
    path = "/v1/roots/{id}/snapshots/{snapshot}/archive",
    operation_id = "download_archive",
    tag = "snapshots",
    params(
        SnapshotAddress,
        ("path" = Option<String>, Query, description = "A file or a directory, Root-relative, its bytes percent-encoded; the whole Snapshot when absent."),
    ),
    responses(
        (status = 200, description = "A zip64 archive, streamed, every file stored uncompressed with its permission bits, so its size is known first: `Content-Length` is what `stat` said. A Withheld path is absent, as from any export; `stat` names them. Generated as it is sent, so it cannot be resumed — an interrupted download starts again.", content_type = "application/zip", headers(
            ("Content-Length" = u64, description = "The archive's exact size."),
            ("Content-Disposition" = String, description = "`attachment`, with a file name."),
        )),
        (status = 400, description = "The path is malformed.", body = Problem, content_type = "application/problem+json"),
        (status = 404, description = "No such Root, Snapshot or path.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The history could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn archive(
    State(state): State<DaemonState>,
    Path(SnapshotAddress { id, snapshot }): Path<SnapshotAddress>,
    PathParameter(path): PathParameter,
) -> Result<Response, ProblemResponse> {
    let (_, location) = located(&state, &id)?;
    let snapshot = parse_snapshot(&snapshot)?;
    let paths = selected(path);
    let name = match paths.first() {
        Some(path) => format!("{}-{snapshot}.zip", String::from_utf8_lossy(path.name())),
        None => format!("{snapshot}.zip"),
    };

    let (head_sender, head) = oneshot::channel::<Result<u64, StoreError>>();
    let (chunks, received) = mpsc::channel::<Result<Bytes, io::Error>>(IN_FLIGHT);
    // The store is neither `Send` nor `Sync`: it lives on this one blocking thread, which says how big
    // the archive is and then streams it.
    tokio::task::spawn_blocking(move || {
        let found = snapshots::open_for_reading(&location).and_then(|store| {
            let store = store.ok_or(StoreError::SnapshotNotFound(snapshot))?;
            let selection = store.selection(snapshot, &paths)?;
            Ok((store, selection))
        });
        let (store, selection) = match found {
            Ok(found) => found,
            Err(error) => {
                let _ = head_sender.send(Err(error));
                return;
            }
        };
        if head_sender
            .send(Ok(archive_size(&entries(&selection.files))))
            .is_err()
        {
            return;
        }
        if let Err(error) = stream(&store, snapshot, &selection.files, &chunks) {
            // A client gone away is no failure; anything else ends the body short, which the client
            // sees as a broken transfer rather than a smaller archive it believes.
            if error.kind() != io::ErrorKind::BrokenPipe {
                let _ = chunks.blocking_send(Err(error));
            }
        }
    });

    let size = head.await.map_err(|_| {
        ProblemResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::HistoryUnreadable,
            "the archive could not be read",
        )
    })?;
    let size = size.map_err(store_problem)?;
    let body = futures_util::stream::unfold(received, |mut received| async move {
        received.recv().await.map(|chunk| (chunk, received))
    });

    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/zip"),
            ),
            (
                header::CONTENT_LENGTH,
                HeaderValue::from_str(&size.to_string()).expect("digits are a header value"),
            ),
            (
                header::CONTENT_DISPOSITION,
                crate::content::attachment(&name),
            ),
        ],
        Body::from_stream(body),
    )
        .into_response())
}

/// Every file of a selection into `chunks`, framed as a zip archive dated at the Snapshot's moment.
fn stream(
    store: &keepster_store::HistoryStore,
    snapshot: SnapshotId,
    files: &[Selected],
    chunks: &mpsc::Sender<Result<Bytes, io::Error>>,
) -> io::Result<()> {
    let send = |bytes: Vec<u8>| {
        chunks
            .blocking_send(Ok(Bytes::from(bytes)))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    };
    let mut writer = ArchiveWriter::new(snapshot.moment());
    for file in files {
        let entry = entry(file);
        send(writer.begin(&entry))?;
        let mut crc = Crc32::new();
        store
            .read_selected(file, &mut |chunk| {
                crc.update(chunk);
                send(chunk.to_vec())
            })
            .map_err(|error| match error {
                StoreError::Delivery(error) => error,
                other => io::Error::other(other.to_string()),
            })?;
        send(writer.end(&entry, crc.finish()))?;
    }

    send(writer.finish())
}
