//! Restore: a Snapshot's versions back over the Working Tree, planned whole before anything is written,
//! with one Safety Snapshot per request so the request has one undo (ticket 47).

use std::collections::BTreeMap;
use std::time::SystemTime;

use keepster_core::{
    CapturePolicy, Confirmed, InvalidPath, Plan, Resolution, RootPath, SnapshotId, Trigger, Verify,
    plan, target_of,
};
use keepster_store::{
    CaptureRequest, CapturedSnapshot, RestoreScope, RootRecord, StoreError, Written,
    destination_naming,
};

use crate::snapshots::{self, CaptureError, StoreLocation};

/// One Restore request.
#[derive(Debug, Clone)]
pub struct RestoreRequest {
    /// The Snapshot whose versions are written.
    pub from: SnapshotId,
    /// The paths — a directory meaning everything beneath it — or the whole Snapshot when empty.
    pub paths: Vec<RootPath>,
    /// The structural changes agreed to.
    pub confirmed: Confirmed,
    /// The answers to the questions no confirmation settles, by Snapshot path.
    pub resolutions: BTreeMap<RootPath, Resolution>,
    /// Pass over what is still in the way rather than stop the request.
    pub skip_unresolvable: bool,
    /// Only plan: write nothing and take no Safety Snapshot.
    pub dry_run: bool,
}

/// What a Restore did — or, for a dry run, would do.
#[derive(Debug, Clone)]
pub struct Restored {
    /// The Snapshot the Working Tree can be restored to, to undo this one. `None` for a dry run.
    pub safety_snapshot: Option<SnapshotId>,
    /// The Safety Snapshot when this request wrote one, rather than finding the Working Tree identical
    /// to the latest Snapshot.
    pub captured: Option<CapturedSnapshot>,
    /// What was decided for every path.
    pub plan: Plan,
    /// What happened to every path, in the plan's order; `None` for a dry run.
    pub written: Option<Vec<Written>>,
}

/// Why a Restore did not happen.
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    /// Some path needs a confirmation or a resolution, or cannot be restored: nothing was written.
    #[error("the restore needs a confirmation or a resolution first")]
    Blocked(Plan),
    /// A rename resolution does not name a bare name.
    #[error("`{path}` cannot be renamed that way: {reason}")]
    RenameInvalid {
        /// The path the resolution was for.
        path: RootPath,
        /// Why.
        reason: InvalidPath,
    },
    /// The Safety Snapshot could not be taken, so nothing was written.
    #[error(transparent)]
    Capture(#[from] CaptureError),
    /// The history could not be read.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Restore `request` over the Root `root`.
///
/// The whole request is planned before anything is written; if any path is blocked nothing is, and the
/// plan says why for every path. Otherwise the Safety Snapshot comes first — a Snapshot of the current
/// state, or the latest one when the Working Tree is identical to it — and then each file is written,
/// re-checked against the plan as it goes. Restore overwrites and creates; it never deletes.
pub fn restore(
    root: &RootRecord,
    location: &StoreLocation,
    policy: &CapturePolicy,
    request: &RestoreRequest,
    now: SystemTime,
) -> Result<Restored, RestoreError> {
    let Some(store) = snapshots::open_for_reading(location)? else {
        return Err(StoreError::SnapshotNotFound(request.from).into());
    };

    let mut renamed = BTreeMap::new();
    for (path, resolution) in &request.resolutions {
        if let Resolution::RenameTo(_) = resolution {
            let target = target_of(path, Some(resolution)).map_err(|reason| {
                RestoreError::RenameInvalid {
                    path: path.clone(),
                    reason,
                }
            })?;
            renamed.insert(path.clone(), target);
        }
    }
    let scope = RestoreScope {
        from: request.from,
        paths: &request.paths,
        root_path: &root.path,
        policy,
        naming: destination_naming(&root.path),
        renamed: &renamed,
    };

    let preflight = store.restore_preflight(&scope)?;
    let planned = plan(
        preflight.found.clone(),
        scope.naming,
        &request.confirmed,
        &request.resolutions,
        request.skip_unresolvable,
    );
    if request.dry_run {
        return Ok(Restored {
            safety_snapshot: None,
            captured: None,
            plan: planned,
            written: None,
        });
    }
    if !planned.proceeds() {
        return Err(RestoreError::Blocked(planned));
    }

    let safety = snapshots::pass(
        root,
        location,
        policy,
        &CaptureRequest {
            trigger: Trigger::Restore,
            now,
            verify: Verify::Stat,
            record_unchanged: false,
        },
    )?;
    let safety_snapshot = safety
        .snapshot
        .as_ref()
        .map(|captured| captured.id)
        .or(safety.unchanged_since);

    let written = store.restore_write(&scope, &preflight, &planned);

    Ok(Restored {
        safety_snapshot,
        captured: safety.snapshot,
        plan: planned,
        written: Some(written),
    })
}
