//! When Retention runs (ticket 51, ticket 06): once for each Segment rollover of a Root, at the first
//! quiet moment after it, and never at the same time as a capture of that Root.
//!
//! A pass has work only once a Segment has wholly crossed a Tier boundary, which happens at most once a
//! Segment Period, so it is checked once per rollover. It is never urgent, so it waits until the Root has
//! been quiet for a while — and, so that a Root that never goes quiet is still kept within its ladder,
//! not longer than a ceiling. It takes the store's lock as a capture does, without waiting for it: a
//! capture in flight wins, and the pass is tried again at the next check.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, TryLockError};
use std::time::{Duration, Instant};

use keepster_api::Event;
use keepster_app::snapshots;
use keepster_core::{RootId, SegmentName};

use crate::DaemonState;

/// How Retention is paced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionTiming {
    /// How often every Root is looked at.
    pub every: Duration,
    /// How long a Root must have taken no Snapshot for a pass to run.
    pub quiet: Duration,
    /// How long a pass waits for that before it runs anyway.
    pub ceiling: Duration,
}

impl Default for RetentionTiming {
    /// A look a minute, five minutes' quiet, six hours at most (ticket 06).
    fn default() -> Self {
        Self {
            every: Duration::from_secs(60),
            quiet: Duration::from_secs(5 * 60),
            ceiling: Duration::from_secs(6 * 3600),
        }
    }
}

/// Which Roots owe a pass.
#[derive(Debug)]
pub(crate) struct Retention {
    timing: RetentionTiming,
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
    /// The Segment a pass last ran in; a pass is owed again once the current one is another.
    ran_in: Option<SegmentName>,
    /// When the pass now owed was first seen to be owed, which the ceiling counts from.
    since: Option<Instant>,
}

impl Retention {
    pub(crate) fn new(timing: RetentionTiming) -> Self {
        Self {
            timing,
            roots: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
            stop: tokio::sync::Notify::new(),
        }
    }

    /// When the pass the Root `id` owes in `segment` became owed, or `None` when it owes none. The wait
    /// starts at the first look that finds it owed — for a Root seen for the first time too, so the
    /// daemon's start schedules a pass rather than running one while it reconciles.
    fn owed_since(&self, id: RootId, segment: &SegmentName) -> Option<Instant> {
        let mut roots = self.roots();
        let due = roots.entry(id).or_default();
        if due.ran_in.as_ref() == Some(segment) {
            return None;
        }

        Some(*due.since.get_or_insert_with(Instant::now))
    }

    /// The Root `id` had its pass in `segment`; the next is owed from the next rollover.
    fn ran(&self, id: RootId, segment: SegmentName) {
        self.roots().insert(
            id,
            Due {
                ran_in: Some(segment),
                since: None,
            },
        );
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
    let mut ticks = tokio::time::interval(state.retention.timing.every);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick is immediate; the start's own Reconciliations come first.
    ticks.tick().await;

    loop {
        tokio::select! {
            _ = ticks.tick() => {}
            () = state.retention.stop.notified() => break,
        }
        if state.retention.stopping.load(Ordering::SeqCst) {
            break;
        }
        let looked = {
            let state = state.clone();
            tokio::task::spawn_blocking(move || pass_over_every_root(&state)).await
        };
        if let Err(error) = looked {
            tracing::error!(%error, "a look for Retention passes failed");
        }
    }
}

/// One look at every registered Root, running the pass each one owes and may run now.
fn pass_over_every_root(state: &DaemonState) {
    let records = match state.registry().readable() {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(%error, "the Registry could not be read for Retention");
            return;
        }
    };
    state
        .retention
        .forget_except(&records.iter().map(|record| record.id).collect::<Vec<_>>());

    for record in records {
        // A stopping daemon's last captures must not find a store taken by a pass just starting.
        if state.retention.stopping.load(Ordering::SeqCst) {
            return;
        }
        consider(state, &record);
    }
}

/// Run the pass the Root `record` owes, if it owes one and may run it now.
fn consider(state: &DaemonState, record: &keepster_store::RootRecord) {
    let id = record.id;
    let timing = state.retention.timing;
    let now = crate::roots::now();
    let Ok(segment) = SegmentName::hourly(now) else {
        return;
    };
    let Some(since) = state.retention.owed_since(id, &segment) else {
        return;
    };
    // A Root without a configuration in force is not captured, and its history is not touched either:
    // the ladder it would be thinned by is the one that cannot be read. Its warnings are the capture's
    // to log, not each look's.
    let Ok(loaded) = state.effective(id, &record.path) else {
        return;
    };
    let location = snapshots::location(&state.registry().root_dir(id));

    let quiet = match snapshots::newest(&location) {
        Ok(newest) => newest.is_none_or(|newest| {
            now.duration_since(newest.moment())
                .is_ok_and(|age| age >= timing.quiet)
        }),
        Err(error) => {
            tracing::warn!(root = %id, %error, "Retention could not read the history");
            return;
        }
    };
    if !quiet && since.elapsed() < timing.ceiling {
        return;
    }

    let store = state.store_lock(id);
    let _held = match store.try_lock() {
        Ok(held) => held,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        // A capture is in flight: it wins, and the pass is tried at the next look.
        Err(TryLockError::WouldBlock) => return,
    };
    if !state.registry().contains(id) {
        return;
    }

    match snapshots::retain(&location, &loaded.retention, now) {
        Ok(retained) => {
            state.retention.ran(id, segment);
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
