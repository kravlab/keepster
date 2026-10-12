//! Export: a Snapshot's versions written into a directory the request names, planned whole before
//! anything is written (ticket 58, ticket 19).
//!
//! Restore's preflight, less its three Restore-only rules, and with two of its own: overwriting is
//! confirmed, and so is writing into a Watched Root — where overwriting takes that Root's Safety Snapshot
//! first, as a Restore would, so an Export is never a second, less safe way to destroy a working file.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

use keepster_core::{
    Ancestry, CapturePolicy, Confirmed, Existing, InvalidPath, Plan, Resolution, RootPath,
    SnapshotId, Step, Trigger, Verify, export_plan, fence_other_roots, target_of,
};
use keepster_store::{
    CaptureRequest, CapturedSnapshot, ExportScope, HistoryStore, Preflight, RootRecord, StoreError,
    Within, Written, destination_naming,
};

use crate::snapshots::{self, CaptureError, StoreLocation};

/// One Export request.
#[derive(Debug, Clone)]
pub struct ExportRequest {
    /// The Snapshot whose versions are written.
    pub from: SnapshotId,
    /// The paths — a directory meaning everything beneath it — or the whole Snapshot when empty.
    pub paths: Vec<RootPath>,
    /// The directory written into.
    pub dest: PathBuf,
    /// The changes agreed to.
    pub confirmed: Confirmed,
    /// The answers to the questions no confirmation settles, by Snapshot path.
    pub resolutions: BTreeMap<RootPath, Resolution>,
    /// Pass over what is still in the way rather than stop the request.
    pub skip_unresolvable: bool,
    /// Every other Watched Root beneath the destination, as a path in it: what lands there is refused,
    /// since that Root's Safety Snapshot is not this Export's to take.
    pub beneath: Vec<RootPath>,
}

/// The Watched Root an Export's destination lies inside.
#[derive(Debug, Clone)]
pub struct DestinationRoot {
    /// Its record.
    pub record: RootRecord,
    /// Where its history lives: where its Safety Snapshot goes.
    pub location: StoreLocation,
    /// What it captures.
    pub policy: CapturePolicy,
    /// Where the destination is in it.
    pub at: RootPath,
}

/// What an Export would do.
#[derive(Debug, Clone)]
pub struct Planned {
    /// What was decided for every path.
    pub plan: Plan,
    /// How many files the selection holds, Withheld paths aside.
    pub files: u64,
    /// Their sizes, summed.
    pub bytes: u64,
}

/// What an Export did.
#[derive(Debug, Clone)]
pub struct Exported {
    /// What was decided for every path.
    pub plan: Plan,
    /// What happened to every path, in the plan's order.
    pub written: Vec<Written>,
    /// The destination Root's Snapshot taken before anything in it was overwritten — or its latest,
    /// when nothing had changed since.
    pub safety_snapshot: Option<SnapshotId>,
    /// The Safety Snapshot when this Export wrote one.
    pub captured: Option<CapturedSnapshot>,
}

/// Why an Export did not happen.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// Some path needs a confirmation or a resolution, or cannot be exported: nothing was written.
    #[error("the export needs a confirmation or a resolution first")]
    Blocked(Plan),
    /// A rename resolution does not name a bare name.
    #[error("`{path}` cannot be renamed that way: {reason}")]
    RenameInvalid {
        /// The path the resolution was for.
        path: RootPath,
        /// Why.
        reason: InvalidPath,
    },
    /// The destination Root's Safety Snapshot could not be taken, so nothing was written.
    #[error(transparent)]
    Capture(#[from] CaptureError),
    /// The history could not be read.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Plan `request` from the history at `location`, into a destination inside `into` when it is in a Root.
pub fn plan(
    location: &StoreLocation,
    request: &ExportRequest,
    into: Option<&DestinationRoot>,
) -> Result<Planned, ExportError> {
    let store = open(location, request.from)?;
    let renamed = renamed(request)?;
    let (_, preflight, plan) = planned(&store, request, into, &renamed)?;
    let (files, bytes) = preflight.held();

    Ok(Planned { plan, files, bytes })
}

/// Export `request` from the history at `location`: planned again, refused whole if any path is blocked,
/// the destination Root's Safety Snapshot taken first where something in it is overwritten, then every
/// file written, re-checked as it goes, `done` told after each how many of `total` are.
pub fn export(
    location: &StoreLocation,
    request: &ExportRequest,
    into: Option<&DestinationRoot>,
    now: SystemTime,
    done: &mut dyn FnMut(u64, u64),
) -> Result<Exported, ExportError> {
    let store = open(location, request.from)?;
    let renamed = renamed(request)?;
    let (scope, preflight, plan) = planned(&store, request, into, &renamed)?;
    if !plan.proceeds() {
        return Err(ExportError::Blocked(plan));
    }

    let mut safety = None;
    if let Some(into) = into.filter(|_| overwrites(&preflight, &plan)) {
        safety = Some(snapshots::pass(
            &into.record,
            &into.location,
            &into.policy,
            &CaptureRequest {
                trigger: Trigger::Export,
                now,
                verify: Verify::Stat,
                record_unchanged: false,
                hold_emptied: false,
            },
        )?);
    }
    let safety_snapshot = safety.as_ref().and_then(|pass| {
        pass.snapshot
            .as_ref()
            .map(|captured| captured.id)
            .or(pass.unchanged_since)
    });

    let total = plan.paths.len() as u64;
    let written = store.export_write(&scope, &preflight, &plan, &mut |count| done(count, total));

    Ok(Exported {
        plan,
        written,
        safety_snapshot,
        captured: safety.and_then(|pass| pass.snapshot),
    })
}

/// The history to export from, which a Root never captured does not have.
fn open(location: &StoreLocation, from: SnapshotId) -> Result<HistoryStore, ExportError> {
    snapshots::open_for_reading(location)?
        .ok_or(ExportError::Store(StoreError::SnapshotNotFound(from)))
}

/// Where each renamed path is written instead.
fn renamed(request: &ExportRequest) -> Result<BTreeMap<RootPath, RootPath>, ExportError> {
    let mut renamed = BTreeMap::new();
    for (path, resolution) in &request.resolutions {
        if let Resolution::RenameTo(_) = resolution {
            let target =
                target_of(path, Some(resolution)).map_err(|reason| ExportError::RenameInvalid {
                    path: path.clone(),
                    reason,
                })?;
            renamed.insert(path.clone(), target);
        }
    }

    Ok(renamed)
}

/// The scope, the facts and the plan of `request`.
fn planned<'a>(
    store: &HistoryStore,
    request: &'a ExportRequest,
    into: Option<&'a DestinationRoot>,
    renamed: &'a BTreeMap<RootPath, RootPath>,
) -> Result<(ExportScope<'a>, Preflight, Plan), ExportError> {
    let scope = ExportScope {
        from: request.from,
        paths: &request.paths,
        dest: &request.dest,
        naming: destination_naming(naming_probe(&request.dest)),
        renamed,
        within: into.map(|into| Within {
            root_path: &into.record.path,
            policy: &into.policy,
            at: &into.at,
        }),
    };
    let preflight = store.export_preflight(&scope)?;
    let mut plan = export_plan(
        preflight.found.clone(),
        scope.naming,
        &request.confirmed,
        &request.resolutions,
        request.skip_unresolvable,
        into.is_some(),
    );
    fence_other_roots(
        &mut plan,
        |target| {
            request
                .beneath
                .iter()
                .any(|root| target == root || target.ancestors().any(|above| above == *root))
        },
        request.skip_unresolvable,
    );

    Ok((scope, preflight, plan))
}

/// The nearest directory there is at or above `dest`, whose filesystem says what names it folds: a
/// destination not made yet will be made on it.
fn naming_probe(dest: &std::path::Path) -> &std::path::Path {
    dest.ancestors()
        .find(|directory| directory.is_dir())
        .unwrap_or(dest)
}

/// Whether writing the plan replaces anything already there, which inside a Root the Safety Snapshot is
/// to hold first.
fn overwrites(preflight: &Preflight, plan: &Plan) -> bool {
    plan.paths
        .iter()
        .zip(&preflight.found)
        .any(|(planned, found)| {
            matches!(planned.step, Step::Create | Step::Restore)
                && (found.existing != Existing::Absent
                    || matches!(found.ancestry, Ancestry::File { .. }))
        })
}
