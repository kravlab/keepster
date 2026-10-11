//! A Root's housekeeping (tickets 51 and 52, ticket 06): Retention once for each Segment rollover, and a
//! Maintenance Pass at most daily — each at the first quiet moment after it is owed, and never at the
//! same time as a capture of that Root.
//!
//! Retention has work only once a Segment has wholly crossed a Tier boundary, which happens at most once
//! a Segment Period, so it is owed once per rollover. Maintenance rewrites packs, so it is owed once a
//! day, counted from the pass the store records — so a restarted daemon does not maintain every store at
//! once — and only while there is a `git` to run. Neither is urgent, so each waits until the Root has been
//! quiet for a while — and, so that a Root that never goes quiet is still kept within its ladder, not
//! longer than its own ceiling. Both take the store's lock as a capture does, without waiting for it: a
//! capture in flight wins, and the pass is tried again at the next look.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, TryLockError};
use std::time::{Duration, Instant, SystemTime};

use keepster_api::Event;
use keepster_app::snapshots::{self, StoreLocation};
use keepster_core::{RootId, SegmentName};

use crate::DaemonState;

/// How housekeeping is paced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HousekeepingTiming {
    /// How often every Root is looked at.
    pub every: Duration,
    /// How long a Root must have taken no Snapshot for a pass to run.
    pub quiet: Duration,
    /// How long a pass waits for that before it runs anyway.
    pub ceiling: Duration,
    /// How long after one Maintenance Pass the next is owed.
    pub maintain_every: Duration,
}

impl Default for HousekeepingTiming {
    /// A look a minute, five minutes' quiet, six hours at most, maintenance daily (ticket 06).
    fn default() -> Self {
        Self {
            every: Duration::from_secs(60),
            quiet: Duration::from_secs(5 * 60),
            ceiling: Duration::from_secs(6 * 3600),
            maintain_every: Duration::from_secs(24 * 3600),
        }
    }
}

/// Which Roots owe which pass.
#[derive(Debug)]
pub(crate) struct Housekeeping {
    timing: HousekeepingTiming,
    /// Taken last of all the daemon's locks, after a store's lock and the Registry.
    roots: Mutex<HashMap<RootId, Due>>,
    /// Set once the daemon is stopping, after which no pass starts.
    stopping: AtomicBool,
    /// Wakes the loop to stop.
    stop: tokio::sync::Notify,
}

/// Where one Root stands with its passes.
#[derive(Debug, Clone, Default)]
struct Due {
    /// The Segment Retention last ran in; it is owed again once the current one is another.
    retained_in: Option<SegmentName>,
    /// When the Retention now owed was first seen to be owed, which the ceiling counts from.
    retention_since: Option<Instant>,
    /// When the last Maintenance Pass finished, by the wall clock the store's record of it is kept in;
    /// one is owed again a day later.
    maintained: Option<SystemTime>,
    /// When the Maintenance Pass now owed was first seen to be owed.
    maintenance_since: Option<Instant>,
}

/// The passes one Root owes now, each with when it became owed.
#[derive(Debug, Clone, Copy, Default)]
struct Owed {
    retention: Option<Instant>,
    maintenance: Option<Instant>,
}

impl Owed {
    /// Whether nothing is owed.
    fn is_none(self) -> bool {
        self.retention.is_none() && self.maintenance.is_none()
    }

    /// Only the passes that may run now: each once the Root is `quiet`, or once it has waited `ceiling`.
    fn runnable(self, quiet: bool, ceiling: Duration) -> Self {
        let may_run =
            |since: Option<Instant>| since.filter(|since| quiet || since.elapsed() >= ceiling);

        Self {
            retention: may_run(self.retention),
            maintenance: may_run(self.maintenance),
        }
    }
}

impl Housekeeping {
    pub(crate) fn new(timing: HousekeepingTiming) -> Self {
        Self {
            timing,
            roots: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
            stop: tokio::sync::Notify::new(),
        }
    }

    /// Whether the Root `id` has been looked at before.
    fn knows(&self, id: RootId) -> bool {
        self.roots().contains_key(&id)
    }

    /// Start keeping the Root `id`, whose store records its last Maintenance Pass at `maintained`.
    fn seed(&self, id: RootId, maintained: Option<SystemTime>) {
        self.roots().entry(id).or_insert_with(|| Due {
            maintained,
            ..Due::default()
        });
    }

    /// What the Root `id` owes in `segment` at `now` — maintenance only if it `can_maintain`. Each wait
    /// starts at the first look that finds it owed — for a Root seen for the first time too, so the
    /// daemon's start schedules its passes rather than running them while it reconciles.
    fn owed(&self, id: RootId, segment: &SegmentName, now: SystemTime, can_maintain: bool) -> Owed {
        let every = self.timing.maintain_every;
        let mut roots = self.roots();
        let due = roots.entry(id).or_default();
        let looked = Instant::now();

        let retention = (due.retained_in.as_ref() != Some(segment))
            .then(|| *due.retention_since.get_or_insert(looked));
        // A record from a clock that has since been set back by more than a day is as stale as one a day
        // old; within that, it is taken at its word.
        let maintenance_owed = can_maintain
            && due.maintained.is_none_or(|last| {
                now.duration_since(last)
                    .unwrap_or_else(|ahead| ahead.duration())
                    >= every
            });
        if !maintenance_owed {
            // Without a `git` nothing is owed, and the ceiling counts from when one is.
            due.maintenance_since = None;
        }
        let maintenance = maintenance_owed.then(|| *due.maintenance_since.get_or_insert(looked));

        Owed {
            retention,
            maintenance,
        }
    }

    /// The Root `id` had its Retention in `segment`; the next is owed from the next rollover.
    fn retained(&self, id: RootId, segment: SegmentName) {
        let mut roots = self.roots();
        let due = roots.entry(id).or_default();
        due.retained_in = Some(segment);
        due.retention_since = None;
    }

    /// The Root `id` had its Maintenance Pass at `now` — or had nothing to maintain, or a pass failed
    /// for its store alone; the next is owed a day on.
    fn maintained(&self, id: RootId, now: SystemTime) {
        let mut roots = self.roots();
        let due = roots.entry(id).or_default();
        due.maintained = Some(now);
        due.maintenance_since = None;
    }

    /// Forget every Root not among `registered`.
    fn forget_except(&self, registered: &[RootId]) {
        self.roots().retain(|id, _| registered.contains(id));
    }

    /// Start no further pass, and end the loop once the pass in flight, if any, is done.
    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.stop.notify_one();
    }

    fn roots(&self) -> std::sync::MutexGuard<'_, HashMap<RootId, Due>> {
        self.roots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Look at every Root on the pace `state` was started with, until the daemon stops.
pub(crate) async fn run_continually(state: DaemonState) {
    let mut ticks = tokio::time::interval(state.housekeeping.timing.every);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick is immediate; the start's own Reconciliations come first.
    ticks.tick().await;

    loop {
        tokio::select! {
            _ = ticks.tick() => {}
            () = state.housekeeping.stop.notified() => break,
        }
        if state.housekeeping.stopping.load(Ordering::SeqCst) {
            break;
        }
        let looked = {
            let state = state.clone();
            tokio::task::spawn_blocking(move || pass_over_every_root(&state)).await
        };
        if let Err(error) = looked {
            tracing::error!(%error, "a look for housekeeping passes failed");
        }
    }
}

/// One look at every registered Root, running the passes each one owes and may run now.
fn pass_over_every_root(state: &DaemonState) {
    let records = match state.registry().readable() {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(%error, "the Registry could not be read for housekeeping");
            return;
        }
    };
    state
        .housekeeping
        .forget_except(&records.iter().map(|record| record.id).collect::<Vec<_>>());
    // A `git` installed or upgraded since the last look is found here, never under another lock.
    crate::git::refresh(state);

    for record in records {
        // A stopping daemon's last captures must not find a store taken by a pass just starting.
        if state.housekeeping.stopping.load(Ordering::SeqCst) {
            return;
        }
        consider(state, &record);
    }
}

/// Run the passes the Root `record` owes, if it owes any and may run them now.
fn consider(state: &DaemonState, record: &keepster_store::RootRecord) {
    let id = record.id;
    let timing = state.housekeeping.timing;
    let now = crate::roots::now();
    let Ok(segment) = SegmentName::hourly(now) else {
        return;
    };
    let location = snapshots::location(&state.registry().root_dir(id));
    if !state.housekeeping.knows(id) {
        state
            .housekeeping
            .seed(id, snapshots::maintained_at(&location));
    }
    // Without a `git` there is no Maintenance Pass to owe: status says why.
    let git = crate::git::current(state);
    let owed = state.housekeeping.owed(id, &segment, now, git.is_some());
    if owed.is_none() {
        return;
    }
    // A Root without a configuration in force is not captured, and its history is not touched either:
    // the ladder it would be thinned by is the one that cannot be read. Its warnings are the capture's
    // to log, not each look's.
    let Ok(loaded) = state.effective(id, &record.path) else {
        return;
    };

    let quiet = match snapshots::newest(&location) {
        Ok(newest) => newest.is_none_or(|newest| {
            now.duration_since(newest.moment())
                .is_ok_and(|age| age >= timing.quiet)
        }),
        Err(error) => {
            tracing::warn!(root = %id, %error, "housekeeping could not read the history");
            return;
        }
    };
    let owed = owed.runnable(quiet, timing.ceiling);
    if owed.is_none() {
        return;
    }

    let store = state.store_lock(id);
    let _held = match store.try_lock() {
        Ok(held) => held,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        // A capture is in flight: it wins, and the passes are tried at the next look.
        Err(TryLockError::WouldBlock) => return,
    };
    if !state.registry().contains(id) {
        return;
    }

    if owed.retention.is_some() {
        retain(state, id, &location, &loaded.retention, now, segment);
    }
    if let (Some(_), Some((generation, git))) = (owed.maintenance, git) {
        maintain(state, id, &location, generation, git, now);
    }
}

/// Retention over the Root `id`, under its store's lock.
fn retain(
    state: &DaemonState,
    id: RootId,
    location: &StoreLocation,
    ladder: &keepster_core::Ladder,
    now: std::time::SystemTime,
    segment: SegmentName,
) {
    match snapshots::retain(location, ladder, now) {
        Ok(retained) => {
            state.housekeeping.retained(id, segment);
            if let Some(reason) = &retained.index_lag {
                tracing::warn!(root = %id, %reason, "the history index lags");
            }
            if retained.removed_snapshots == 0 {
                return;
            }
            tracing::info!(
                root = %id,
                deleted = retained.deleted_segments,
                thinned = retained.thinned_segments,
                removed = retained.removed_snapshots,
                "Retention removed what the ladder no longer keeps"
            );
            state.publish(Event::RetentionCompleted {
                root: id.to_string(),
                deleted_segments: retained.deleted_segments,
                thinned_segments: retained.thinned_segments,
                removed_snapshots: retained.removed_snapshots,
            });
        }
        // Owed still: the next look tries again.
        Err(error) => tracing::warn!(root = %id, %error, "a Retention pass failed"),
    }
}

/// A Maintenance Pass over the Root `id` at `now` with the `git` of `generation`, under the store's lock.
/// Until a pass with that `git` has succeeded, a failure is the `git`'s and makes maintenance unavailable
/// for every Root, with what it said in `status` (ticket 11); after one has, it is this store's, and is
/// tried again the next day.
fn maintain(
    state: &DaemonState,
    id: RootId,
    location: &StoreLocation,
    generation: u64,
    git: keepster_store::Git,
    now: SystemTime,
) {
    let store = match snapshots::open_for_reading(location) {
        Ok(Some(store)) => store,
        // Nothing captured yet, so nothing to maintain.
        Ok(None) => {
            state.housekeeping.maintained(id, now);
            return;
        }
        Err(error) => {
            tracing::warn!(root = %id, %error, "the history could not be opened for maintenance");
            return;
        }
    };

    let Some(git) = crate::git::proven(state, generation, git) else {
        return;
    };
    match store.maintain(&git) {
        Ok(()) => {
            crate::git::succeeded(state, generation);
            state.housekeeping.maintained(id, now);
            tracing::info!(root = %id, git = %git.path().display(), "maintained");
            state.publish(Event::MaintenanceCompleted {
                root: id.to_string(),
            });
        }
        Err(unavailable) => {
            tracing::warn!(root = %id, %unavailable, "a Maintenance Pass failed");
            if !crate::git::failed(state, generation, unavailable) {
                state.housekeeping.maintained(id, now);
            }
        }
    }
}
