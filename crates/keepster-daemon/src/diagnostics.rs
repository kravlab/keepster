//! What the daemon hands a diagnostics bundle: `GET /v1/diagnostics` (ticket 63, ADR 0013).
//!
//! **The bundle discloses by inventory, and never redacts.** Paths are here as they are — the Registry,
//! the configuration and the log are full of them, and the defects the bundle exists for cannot be
//! diagnosed without them — while the inventory the CLI prints says what each file reveals. A reader
//! who finds no redaction code here should not add it: a redaction pass promises what it cannot keep
//! (ADR 0013).
//!
//! **No byte of any file from history is ever read here**, under any flag: every section comes from the
//! daemon's state, the configuration files, the Registry, refs, commit messages and the index — none
//! from a blob — so the guarantee needs no pattern matching.
//!
//! **Nothing is sent anywhere.** The route is served on the control channel alone, never on the loopback
//! listener, and the CLI writes what it receives into a local file. The product has no remote, and this
//! is the one command where egress would have looked natural.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use keepster_api::{
    DiagnosticConfiguration, Diagnostics, Environment, ErrorCode, GlobalConfiguration, LogFileTail,
    LogTail, MaintenancePass, PinTopology, Problem, RegistryRecord, RootConfigurationReport,
    RootFilesystem, RootTopology, RootWatermarks, SegmentTopology, SnapshotPoint, Status,
    StoreTopology, WatchCounters, WatermarkReport,
};
use keepster_app::config::GlobalLevel;
use keepster_core::{RootId, SnapshotId};
use keepster_store::{HeldEntry, Registry, Standing, Topology};

use crate::DaemonState;
use crate::bundle::{DiskEnvironment, DiskWatches, OnDisk};
use crate::housekeeping::{LastPass, PassOutcome};
use crate::problem::{ProblemResponse, invalid_file};
use crate::volumes::Volumes;

/// How much of the log a bundle carries, at most: the newest lines, across the current file and the
/// rotated ones. A megabyte holds hours of informational lines, and a bundle stays small enough to
/// attach to an issue.
pub const LOG_TAIL_BUDGET: u64 = 1024 * 1024;

/// Everything the daemon knows that a diagnostics bundle carries.
#[utoipa::path(
    get,
    path = "/v1/diagnostics",
    operation_id = "get_diagnostics",
    tag = "status",
    responses(
        (status = 200, description = "Every section of a diagnostics bundle, each carrying its own failure. Never a byte of a file from history. Served on the control channel alone.", body = Diagnostics),
        (status = 500, description = "The Registry could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn diagnostics(
    State(state): State<DaemonState>,
) -> Result<Json<Diagnostics>, ProblemResponse> {
    let status = crate::status_of(&state).await?;
    // Reading every store is blocking work.
    tokio::task::spawn_blocking(move || collect(&state, status))
        .await
        .map_err(|joined| {
            ProblemResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::HistoryUnreadable,
                joined.to_string(),
            )
        })?
        .map(Json)
}

/// Every section beside `status`, read while the Registry is held — as every handler holds it while it
/// reports on a Root, so an erasing deregistration cannot remove a store mid-read. The order is always
/// the Registry, then the configuration.
fn collect(state: &DaemonState, status: Status) -> Result<Diagnostics, ProblemResponse> {
    let registry = state.registry();
    let held = registry.held().map_err(crate::roots::registry_problem)?;
    let watched = watched(&held);

    let (in_use, ceiling) = state.runtimes.watches();
    let passes = state.housekeeping.last_passes();
    let environment = Environment {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        filesystems: filesystems(state.volumes.as_ref(), &watched),
        watches: WatchCounters {
            in_use: count(in_use),
            ceiling: ceiling.map(count),
        },
        git: crate::git::report(state).1,
        maintenance: held
            .iter()
            .filter_map(|entry| {
                passes
                    .get(&entry.id)
                    .map(|pass| maintenance(entry.id, pass))
            })
            .collect(),
    };

    // Taken and released before any Root's configuration is resolved, which takes the same lock.
    let global_level = state.config().global();
    let configuration = DiagnosticConfiguration {
        global: global_report(global_level),
        roots: watched
            .iter()
            .map(|(id, path)| {
                root_report(
                    *id,
                    crate::configuration::root_configuration(*id, state.configuration(*id, path)),
                )
            })
            .collect(),
    };

    let mut topology = Vec::new();
    let mut watermarks = Vec::new();
    for entry in &held {
        let location = keepster_app::snapshots::location(&registry.root_dir(entry.id));
        let (shape, marks) = history_of(
            entry.id,
            keepster_app::snapshots::open_for_reading(&location),
        );
        topology.push(shape);
        watermarks.push(marks);
    }

    let log_format = state.config().daemon().log_format;
    let log = match state.log.file() {
        Some(file) => log_tail(&file, log_format),
        None => LogTail {
            format: log_format,
            files: Vec::new(),
            error: None,
        },
    };

    Ok(Diagnostics {
        status,
        environment,
        configuration,
        registry: held.iter().map(registry_record).collect(),
        topology,
        watermarks,
        log,
    })
}

/// Where a bundle collected without the daemon reads from: the places the daemon would use, as this
/// platform resolves them for this user.
#[derive(Debug, Clone, Default)]
pub struct Places {
    /// The base every Root's directory is under: the Registry and the stores.
    pub store_base: Option<PathBuf>,
    /// The global configuration file.
    pub config_path: Option<PathBuf>,
    /// The user's home, which the configuration's patterns expand `~` in.
    pub home: Option<PathBuf>,
    /// The directory the daemon writes its log into.
    pub log_dir: Option<PathBuf>,
}

/// What the disk alone gives a bundle when the daemon cannot be asked (ticket 63).
///
/// Read without writing anything — no Registry directory created, no known-good copy kept, no index
/// opened — so a report run beside a daemon that will not start leaves the disk as it found it. What
/// only the daemon's memory holds, and each store's topology and watermarks (the CLI collects no
/// history), are left for the bundle to name missing.
pub fn on_disk(places: &Places, volumes: &dyn Volumes) -> OnDisk {
    let held = places
        .store_base
        .as_deref()
        .ok_or_else(|| "this platform has no per-user data directory".to_owned())
        .and_then(|base| Registry::at(base).held().map_err(|error| error.to_string()));
    let watched = held.as_deref().map(watched).unwrap_or_default();

    let global_file =
        keepster_app::config::read_global(places.config_path.as_deref(), places.home.as_deref());
    let level = GlobalLevel {
        file: places.config_path.clone(),
        effective: global_file
            .as_ref()
            .ok()
            .map(keepster_app::config::global_effective),
        error: global_file.as_ref().err().cloned(),
    };
    // The format a daemon starting now would write in, for a log with no line to read it from.
    let log_format = keepster_app::config::starting_settings(global_file.as_ref().ok()).log_format;
    let (roots, unread_roots) = match (&global_file, &places.store_base) {
        (Ok(global), Some(base)) => (
            watched
                .iter()
                .map(|(id, path)| {
                    root_report(
                        *id,
                        crate::configuration::root_configuration(
                            *id,
                            keepster_app::config::read_root(global, *id, path, base),
                        ),
                    )
                })
                .collect(),
            None,
        ),
        // With no global version to merge with, no Root has a configuration to show.
        (Err(error), _) if !watched.is_empty() => (
            Vec::new(),
            Some(format!(
                "the global configuration file could not be read: {error}"
            )),
        ),
        // No Root is named, or no Registry was read to name any — which the bundle names itself.
        _ => (Vec::new(), None),
    };

    OnDisk {
        environment: DiskEnvironment {
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            filesystems: filesystems(volumes, &watched),
            watches: DiskWatches {
                ceiling: keepster_watch::default_ceiling().map(count),
            },
        },
        configuration: DiagnosticConfiguration {
            global: global_report(level),
            roots,
        },
        unread_roots,
        registry: held
            .as_ref()
            .map(|held| held.iter().map(registry_record).collect())
            .map_err(Clone::clone),
        log: match &places.log_dir {
            Some(directory) => log_tail(&directory.join(crate::LOG_FILE), log_format),
            None => LogTail {
                format: log_format,
                files: Vec::new(),
                error: None,
            },
        },
    }
}

/// Every watched Root whose record reads, with its path: the Roots whose filesystem and configuration
/// a bundle reports.
fn watched(held: &[HeldEntry]) -> Vec<(RootId, &Path)> {
    held.iter()
        .filter(|entry| entry.standing == Standing::Watched)
        .filter_map(|entry| Some((entry.id, entry.record.as_ref().ok()?.path.as_path())))
        .collect()
}

/// The filesystem under each of the `watched` Roots, as `volumes` names it.
fn filesystems(volumes: &dyn Volumes, watched: &[(RootId, &Path)]) -> Vec<RootFilesystem> {
    watched
        .iter()
        .map(|(id, path)| filesystem(volumes, *id, path))
        .collect()
}

/// The filesystem under the Root `id` at `path`, as `volumes` names it.
fn filesystem(volumes: &dyn Volumes, id: RootId, path: &Path) -> RootFilesystem {
    let mut found = RootFilesystem {
        root: id.to_string(),
        path: path.display().to_string(),
        filesystem: None,
        remote: None,
        mount_point: None,
        error: None,
    };
    match volumes.volume_of(path) {
        Ok(volume) => {
            found.filesystem = Some(volume.filesystem).filter(|name| !name.is_empty());
            found.remote = Some(volume.remote);
            found.mount_point = Some(volume.mount_point.display().to_string());
        }
        Err(error) => found.error = Some(error.to_string()),
    }

    found
}

/// One Root the Registry holds, with its record as written.
fn registry_record(entry: &HeldEntry) -> RegistryRecord {
    let mut record = RegistryRecord {
        id: entry.id.to_string(),
        standing: match entry.standing {
            Standing::Watched => "watched",
            Standing::Unwatched => "unwatched",
        }
        .to_owned(),
        path: None,
        volume: None,
        within_volume: None,
        error: None,
    };
    match &entry.record {
        Ok(found) => {
            record.path = Some(found.path.display().to_string());
            record.volume = found.volume.as_ref().map(|place| place.identity.clone());
            record.within_volume = found.volume.as_ref().map(|place| place.within.clone());
        }
        Err(error) => record.error = Some(error.to_string()),
    }

    record
}

/// The global level of the configuration on the wire.
fn global_report(level: GlobalLevel) -> GlobalConfiguration {
    GlobalConfiguration {
        file: level.file.map(|file| file.display().to_string()),
        keys: level
            .effective
            .as_ref()
            .map(crate::configuration::keys)
            .unwrap_or_default(),
        errors: level
            .error
            .iter()
            .map(|error| invalid_file(error).into_problem())
            .collect(),
    }
}

/// One Root's configuration, or why it has none, as a bundle reports it.
fn root_report(
    id: RootId,
    configuration: Result<keepster_api::RootConfiguration, ProblemResponse>,
) -> RootConfigurationReport {
    match configuration {
        Ok(configuration) => RootConfigurationReport {
            root: id.to_string(),
            configuration: Some(configuration),
            problem: None,
        },
        Err(problem) => RootConfigurationReport {
            root: id.to_string(),
            configuration: None,
            problem: Some(problem.into_problem()),
        },
    }
}

/// The end of the log whose current file is `file`, within [`LOG_TAIL_BUDGET`], its lines in
/// `format`; or why it could not be read, so a bundle never says there is no log when there is one.
fn log_tail(file: &Path, format: String) -> LogTail {
    let read = match (
        file.parent(),
        file.file_name().and_then(|name| name.to_str()),
    ) {
        (Some(directory), Some(name)) => {
            keepster_service::rotating::tail(directory, name, LOG_TAIL_BUDGET)
                .map_err(|error| format!("{}: {error}", file.display()))
        }
        _ => Err(format!("{} names no file in a directory", file.display())),
    };
    let (files, error) = match read {
        Ok(tails) => (tails, None),
        Err(error) => (Vec::new(), Some(error)),
    };

    LogTail {
        format,
        files: files
            .into_iter()
            .map(|tail| LogFileTail {
                name: tail.name,
                size: tail.size,
                text: tail.text,
                truncated: tail.truncated,
            })
            .collect(),
        error,
    }
}

/// How the Root `id`'s last Maintenance Pass ended, on the wire.
fn maintenance(id: RootId, pass: &LastPass) -> MaintenancePass {
    let outcome = match &pass.outcome {
        PassOutcome::Succeeded(_) => "succeeded",
        PassOutcome::Failed(_) => "failed",
        PassOutcome::Unavailable(_) => "git_unavailable",
        PassOutcome::Unreadable(_) => "history_unreadable",
    };
    let mut wire = MaintenancePass {
        root: id.to_string(),
        finished_at: moment(pass.finished),
        outcome: outcome.to_owned(),
        reclaimed_bytes: None,
        stderr: None,
        error: None,
    };
    match &pass.outcome {
        PassOutcome::Succeeded(maintained) => {
            wire.reclaimed_bytes = Some(maintained.reclaimed_bytes);
        }
        PassOutcome::Failed(git) | PassOutcome::Unavailable(git) => {
            wire.stderr = git.detail().map(ToOwned::to_owned);
            wire.error = Some(git.to_string());
        }
        PassOutcome::Unreadable(error) => wire.error = Some(error.clone()),
    }

    wire
}

/// The Root `id`'s History Store as `opened` found it: its shape, and how far its index got.
fn history_of(
    id: RootId,
    opened: Result<Option<keepster_store::HistoryStore>, keepster_store::StoreError>,
) -> (RootTopology, RootWatermarks) {
    let mut shape = RootTopology {
        root: id.to_string(),
        store: None,
        error: None,
    };
    let mut marks = RootWatermarks {
        root: id.to_string(),
        sources: Vec::new(),
        error: None,
    };
    match opened.and_then(|store| store.map(|store| store.topology()).transpose()) {
        Ok(Some(topology)) => {
            let Topology {
                segments,
                pins,
                watermarks,
                history_bytes,
                index_bytes,
            } = topology;
            shape.store = Some(StoreTopology {
                segments: segments
                    .into_iter()
                    .map(|segment| SegmentTopology {
                        name: segment.name,
                        tip: segment.tip,
                        format: segment.format,
                        snapshots: count(segment.snapshots),
                        foreign: count(segment.foreign),
                        oldest: segment.oldest.map(point),
                        newest: segment.newest.map(point),
                    })
                    .collect(),
                pins: pins
                    .into_iter()
                    .map(|pin| PinTopology {
                        snapshot: pin.snapshot,
                        tip: pin.tip,
                        format: pin.format,
                    })
                    .collect(),
                history_bytes,
                index_bytes,
            });
            match watermarks {
                Ok(watermarks) => {
                    marks.sources = watermarks
                        .into_iter()
                        .map(|mark| WatermarkReport {
                            source: mark.source,
                            tip: mark.tip,
                            indexed: mark.indexed,
                        })
                        .collect();
                }
                Err(error) => marks.error = Some(error.to_string()),
            }
        }
        // Nothing was ever captured: no store, and nothing wrong.
        Ok(None) => {}
        Err(error) => {
            shape.error = Some(error.to_string());
            marks.error = Some(error.to_string());
        }
    }

    (shape, marks)
}

/// A Snapshot by its identity and when it was taken, as the timeline spells it.
fn point(id: SnapshotId) -> SnapshotPoint {
    SnapshotPoint {
        id: id.to_string(),
        taken_at: crate::snapshots::taken_at(id),
    }
}

/// `moment` as RFC 3339 in UTC.
fn moment(moment: std::time::SystemTime) -> String {
    jiff::Timestamp::try_from(moment)
        .map(|moment| moment.to_string())
        .unwrap_or_default()
}

/// `count` on the wire, which counts in `u64`.
fn count(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}
