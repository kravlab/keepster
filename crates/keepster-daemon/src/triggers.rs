//! Automatic capture and Reconciliation (tickets 43 and 44): one runtime per Root, folding the
//! watcher's events and its own timers through the policy kernel and doing what it decides.
//!
//! The policy is `keepster_core::decide`, a pure function; this module owns its state and is the only
//! place that turns an [`Action`] into a capture or a timer. Everything that can happen to a Root — a
//! change, a timer, a capture finishing, the store's lock being busy or released, new settings, the stop
//! — reaches the policy as an event on one channel, so the order the policy sees is the order things
//! were handled in, and shutdown is one more event rather than a second code path.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use keepster_app::config::Effective;
use keepster_app::snapshots;
use keepster_core::{
    Action, CapturePolicy, PolicyEvent, PolicyState, RootId, Trigger, Triggers, Verify, decide,
};
use keepster_store::{CaptureRequest, RootRecord};
use keepster_watch::{WatchEvent, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::DaemonState;

/// How often a capture waiting for its store's lock looks again.
const LOCK_POLL: Duration = Duration::from_millis(500);

/// How long a stopping daemon waits for its runtimes to finish the capture in flight and flush what is
/// pending, inside the service manager's stop timeout of about ten seconds (ticket 05). Past it the
/// remaining runtimes are abandoned, which is as survivable as being killed: at worst an unreferenced
/// object and a lagging index, both repaired at the next start.
const STOP_DRAIN: Duration = Duration::from_secs(8);

/// Every Root's runtime.
#[derive(Debug)]
pub(crate) struct Runtimes {
    runtimes: Mutex<HashMap<RootId, Runtime>>,
    /// Set once the daemon is stopping, after which no runtime is started.
    closed: AtomicBool,
    /// How many directory watches all Roots together may hold, where the platform has such a budget.
    ceiling: Option<usize>,
    /// Held while a Root arms its watcher and checks it against the ceiling, so that Roots arming at
    /// the same moment — every Root, at the daemon's start — count each other's watches.
    arming: tokio::sync::Mutex<()>,
    /// Whether the daemon's first look at the Registry is done. A runtime started by that look owes the
    /// start's Reconciliation only if the Root asks for one; any runtime started later owes one always,
    /// because a Root registered, moved or come back is exactly the change ADR 0009 queues it for.
    settled: AtomicBool,
}

#[derive(Debug)]
struct Runtime {
    path: PathBuf,
    inbox: mpsc::UnboundedSender<Message>,
    task: JoinHandle<()>,
    activity: Arc<Mutex<Activity>>,
}

/// What a Root's runtime tells the rest of the daemon about itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Activity {
    /// Whether some Trigger captures this Root without being asked.
    pub(crate) automatic: bool,
    /// Whether a Reconciliation runs when the daemon starts.
    pub(crate) at_start: bool,
    /// How a pass judges a file it does not read.
    pub(crate) verify: Verify,
    /// How the watcher stands.
    pub(crate) watch: Watch,
    /// Whether the last pass found every mtime on a whole second — a filter weaker than it looks.
    pub(crate) coarse_mtime: bool,
    /// Whether a Reconciliation has completed since the runtime started: until one has, nothing proves
    /// that what changed while nobody watched is in history.
    pub(crate) reconciled: bool,
    /// How many paths wait to be captured.
    pub(crate) pending_paths: u64,
    /// When the oldest of them changed.
    pub(crate) pending_since: Option<Instant>,
}

/// How a Root's watcher stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Watch {
    /// The watcher Trigger is off, or the watcher is still being armed.
    #[default]
    Off,
    /// Watching, with this many directory watches.
    Armed(usize),
    /// Not watched, because arming it would exceed the daemon-wide ceiling: reconciled periodically
    /// instead.
    Ceiling,
    /// Not watched, because the platform refused: reconciled periodically instead.
    Failed,
}

#[derive(Debug)]
enum Message {
    /// Something for the policy.
    Event(PolicyEvent),
    /// The Root's configuration as it stands now.
    Refresh(Box<Effective>),
    /// The Root is no longer captured automatically: deregistered, gone, or without a configuration
    /// to apply. Whatever is pending is dropped with it.
    Stop,
}

impl Runtimes {
    /// No runtimes yet, under a daemon-wide watch ceiling where the platform has one.
    pub(crate) fn new(ceiling: Option<usize>) -> Self {
        Self {
            runtimes: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            ceiling,
            arming: tokio::sync::Mutex::new(()),
            settled: AtomicBool::new(false),
        }
    }

    /// Bring the runtimes in line with the Registry: start one for every Root that can be captured
    /// automatically, hand every running one its current configuration, and stop the rest.
    ///
    /// `roots` is every registered Root with its configuration in force, `None` when it has none — which
    /// leaves the Root uncaptured rather than captured by the defaults (ticket 04).
    pub(crate) fn sync(&self, state: &DaemonState, roots: Vec<(RootRecord, Option<Effective>)>) {
        let mut runtimes = self.runtimes();
        // Checked under the lock `stop` sets it under, so no runtime starts after the stop drained them.
        if self.closed.load(Ordering::SeqCst) {
            return;
        }

        let settled = self.settled.swap(true, Ordering::SeqCst);
        let keep: Vec<RootId> = roots.iter().map(|(record, _)| record.id).collect();
        runtimes.retain(|id, runtime| {
            let kept = keep.contains(id);
            if !kept {
                let _ = runtime.inbox.send(Message::Stop);
            }
            kept
        });

        for (record, effective) in roots {
            let capturable = record.path.is_dir() && effective.is_some();
            match (runtimes.get(&record.id), effective) {
                (Some(runtime), Some(effective)) if capturable && runtime.path == record.path => {
                    let _ = runtime.inbox.send(Message::Refresh(Box::new(effective)));
                }
                // A Root registered, moved or back from being gone: a runtime of its own, whose first
                // act is the Reconciliation a Registry change owes (ADR 0009).
                (_, Some(effective)) if capturable => {
                    if let Some(old) = runtimes.remove(&record.id) {
                        let _ = old.inbox.send(Message::Stop);
                    }
                    let reconcile = settled || effective.reconcile.at_start;
                    runtimes.insert(record.id, start(state, record, effective, reconcile));
                }
                _ => {
                    // A Root that cannot be captured — its directory is gone, or its configuration never
                    // had a valid version — has nothing for a runtime to do until that changes.
                    if let Some(old) = runtimes.remove(&record.id) {
                        let _ = old.inbox.send(Message::Stop);
                    }
                }
            }
        }
    }

    /// Owe the Root `id` a Reconciliation, as a reload that changed what classifies its paths does.
    pub(crate) fn reconcile(&self, id: RootId) {
        if let Some(runtime) = self.runtimes().get(&id) {
            let _ = runtime.inbox.send(Message::Event(PolicyEvent::Reconcile));
        }
    }

    /// Record what a pass over the Root `id` saw, when the pass ran outside its runtime — one someone asked
    /// for.
    pub(crate) fn passed(&self, id: RootId, trigger: Trigger, coarse_mtime: bool) {
        if let Some(runtime) = self.runtimes().get(&id) {
            record_pass(&mut lock(&runtime.activity), trigger, coarse_mtime);
        }
    }

    /// What the runtime of the Root `id` reports, if one runs.
    pub(crate) fn activity(&self, id: RootId) -> Option<Activity> {
        self.runtimes()
            .get(&id)
            .map(|runtime| *lock(&runtime.activity))
    }

    /// Whether `watches` more directory watches for the Root `id` fit under the ceiling beside every
    /// other Root's.
    fn fits(&self, id: RootId, watches: usize) -> bool {
        let Some(ceiling) = self.ceiling else {
            return true;
        };
        let used: usize = self
            .runtimes()
            .iter()
            .filter(|(other, _)| **other != id)
            .map(|(_, runtime)| match lock(&runtime.activity).watch {
                Watch::Armed(count) => count,
                _ => 0,
            })
            .sum();

        used + watches <= ceiling
    }

    /// Stop every runtime as a stopping daemon must: no further Trigger is accepted, the capture in
    /// flight finishes, and every debounce window still counting down is captured now (ticket 05).
    pub(crate) async fn stop(&self) {
        let runtimes: Vec<Runtime> = {
            let mut runtimes = self.runtimes();
            self.closed.store(true, Ordering::SeqCst);
            runtimes.drain().map(|(_, runtime)| runtime).collect()
        };

        for runtime in &runtimes {
            let _ = runtime.inbox.send(Message::Event(PolicyEvent::Shutdown));
        }
        let aborts: Vec<_> = runtimes
            .iter()
            .map(|runtime| runtime.task.abort_handle())
            .collect();
        let drained =
            futures_util::future::join_all(runtimes.into_iter().map(|runtime| runtime.task));
        if tokio::time::timeout(STOP_DRAIN, drained).await.is_err() {
            tracing::warn!("stopping before every pending change was captured");
            for abort in aborts {
                abort.abort();
            }
        }
    }

    fn runtimes(&self) -> MutexGuard<'_, HashMap<RootId, Runtime>> {
        // A runtime is either in the map or not; nothing is left half-done by a panic.
        self.runtimes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Start the Root's runtime; with `reconcile`, its first act is a Reconciliation.
fn start(
    state: &DaemonState,
    record: RootRecord,
    effective: Effective,
    reconcile: bool,
) -> Runtime {
    let (inbox, messages) = mpsc::unbounded_channel();
    let activity = Arc::new(Mutex::new(Activity::default()));
    describe(&mut lock(&activity), &effective);
    let path = record.path.clone();
    let task = tokio::spawn(run(
        state.clone(),
        record,
        effective,
        reconcile,
        inbox.clone(),
        messages,
        Arc::clone(&activity),
    ));

    Runtime {
        path,
        inbox,
        task,
        activity,
    }
}

/// Record in `activity` what a pass saw: the mtimes' granularity, and — for a Reconciliation — that
/// coverage is now proven.
fn record_pass(activity: &mut Activity, trigger: Trigger, coarse_mtime: bool) {
    activity.coarse_mtime = coarse_mtime;
    if trigger == Trigger::Reconcile {
        activity.reconciled = true;
    }
}

/// Record in `activity` what the configuration says about how the Root is captured.
fn describe(activity: &mut Activity, effective: &Effective) {
    activity.automatic = effective.triggers.watcher || effective.triggers.periodic.is_some();
    activity.at_start = effective.reconcile.at_start;
    activity.verify = effective.reconcile.verify;
}

/// The policy's settings for a Root under `effective`, watched or not: a Root whose watcher Trigger is on
/// but which cannot be watched is reconciled periodically instead, so its coverage does not quietly stop
/// at the last event it saw (ticket 30).
fn settings_for(effective: &Effective, watch: Watch) -> Triggers {
    let mut settings = effective.triggers;
    if settings.watcher && matches!(watch, Watch::Ceiling | Watch::Failed) {
        settings.watcher = false;
        settings.periodic = Some(
            settings
                .periodic
                .unwrap_or(effective.reconcile.fallback_every),
        );
    }

    settings
}

/// One Root's runtime: until it is stopped, take the next event — a message, or the timer the policy
/// armed — through the policy, and do what it decides.
async fn run(
    state: DaemonState,
    record: RootRecord,
    effective: Effective,
    reconcile: bool,
    outbox: mpsc::UnboundedSender<Message>,
    mut inbox: mpsc::UnboundedReceiver<Message>,
    activity: Arc<Mutex<Activity>>,
) {
    let origin = state.started;
    let mut rules = effective.policy.clone();
    let mut watching = effective.triggers.watcher;
    let mut watcher = watch(&state, &record, &rules, watching, &outbox, &activity).await;
    let mut settings = settings_for(&effective, lock(&activity).watch);
    let mut policy = PolicyState::new(settings, origin.elapsed());
    let mut deadline: Option<Instant> = None;
    // A daemon that was down saw nothing of what happened meanwhile, and neither did one that just
    // learned of this Root, so the first act is a pass over the whole Root (ticket 02) — unless the user
    // switched the start's walk off, which the Root's state then says, or nothing captures the Root
    // automatically yet. Either way the policy is asked what is due, which is how a periodic floor arms
    // its first timer.
    let mut next = Some(if reconcile {
        PolicyEvent::Reconcile
    } else {
        PolicyEvent::Timer
    });

    loop {
        let event = match next.take() {
            Some(event) => event,
            None => {
                let sleep = tokio::time::sleep_until(
                    deadline
                        .unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400))
                        .into(),
                );
                tokio::select! {
                    message = inbox.recv() => match message {
                        None | Some(Message::Stop) => break,
                        Some(Message::Event(event)) => event,
                        Some(Message::Refresh(effective)) => {
                            // A change to what classifies paths, or to whether the watcher is on, means
                            // different watches; the policy only needs the intervals.
                            if effective.policy != rules || effective.triggers.watcher != watching {
                                release(watcher.take()).await;
                                rules = effective.policy.clone();
                                watching = effective.triggers.watcher;
                                watcher = watch(&state, &record, &rules, watching, &outbox, &activity).await;
                            }
                            let owed = {
                                let mut activity = lock(&activity);
                                let was_automatic = activity.automatic;
                                describe(&mut activity, &effective);
                                reconcile && !was_automatic && activity.automatic && !activity.reconciled
                            };
                            // The pass the runtime's start owed was held back while nothing captured the
                            // Root automatically; it is due once something does.
                            if owed {
                                next = Some(PolicyEvent::Reconcile);
                            }
                            let refreshed = settings_for(&effective, lock(&activity).watch);
                            if refreshed == settings {
                                continue;
                            }
                            settings = refreshed;
                            PolicyEvent::Settings(settings)
                        }
                    },
                    () = sleep, if deadline.is_some() => {
                        deadline = None;
                        PolicyEvent::Timer
                    }
                }
            }
        };

        // A Root nothing captures automatically is passed over only when someone asks
        // (`keepster reconcile`): an automatic Snapshot would contradict its `capture_unavailable`.
        if matches!(event, PolicyEvent::Reconcile) && !lock(&activity).automatic {
            continue;
        }
        let (actions, decided) = decide(policy, event, origin.elapsed());
        policy = decided;
        for action in actions {
            match action {
                Action::Capture { trigger, paths } => {
                    tracing::debug!(root = %record.id, %trigger, changed = paths.len(), "capturing");
                    capture(
                        state.clone(),
                        record.id,
                        trigger,
                        outbox.clone(),
                        Arc::clone(&activity),
                    );
                }
                Action::ArmTimer { at } => deadline = Some(origin + at),
            }
        }

        {
            let mut activity = lock(&activity);
            // A watcher arms the directories that appear as they appear, so its share of the ceiling
            // grows with the tree.
            if let (Some(watcher), Watch::Armed(_)) = (&watcher, activity.watch) {
                activity.watch = Watch::Armed(watcher.watched().len());
            }
            activity.pending_paths = u64::try_from(policy.pending_paths()).unwrap_or(u64::MAX);
            activity.pending_since = policy.pending_since().map(|since| origin + since);
        }
        if policy.stopped() {
            break;
        }
    }

    release(watcher).await;
}

/// Stop a watcher off the runtime's threads: dropping one joins its event thread.
async fn release(watcher: Option<Watcher>) {
    if let Some(watcher) = watcher {
        let _ = tokio::task::spawn_blocking(move || drop(watcher)).await;
    }
}

/// Start watching the Root under `rules`, its events going to `outbox`, when the watcher Trigger is on,
/// and record in `activity` how that went.
///
/// A Root that cannot be watched — the platform refused, or its watches would exceed the daemon-wide
/// ceiling — is reconciled periodically instead: the watcher is a latency optimisation, so failing to arm
/// it is said, never fatal.
async fn watch(
    state: &DaemonState,
    record: &RootRecord,
    rules: &CapturePolicy,
    on: bool,
    outbox: &mpsc::UnboundedSender<Message>,
    activity: &Mutex<Activity>,
) -> Option<Watcher> {
    lock(activity).watch = Watch::Off;
    if !on {
        return None;
    }
    let _arming = state.runtimes.arming.lock().await;
    let (path, rules, outbox) = (record.path.clone(), rules.clone(), outbox.clone());

    // Arming walks the tree, which blocks.
    let started = tokio::task::spawn_blocking(move || {
        Watcher::start(&path, &rules, move |event| {
            let event = match event {
                WatchEvent::Changed(paths) => PolicyEvent::Changed(paths),
                // The platform lost events, and nothing says which: the guarantee is a pass over the
                // whole Root (ticket 35).
                WatchEvent::Rescan => PolicyEvent::Reconcile,
            };
            let _ = outbox.send(Message::Event(event));
        })
    })
    .await;

    let watcher = match started {
        Ok(Ok(watcher)) => watcher,
        Ok(Err(error)) => {
            tracing::warn!(root = %record.id, %error, "the Root could not be watched; it is reconciled periodically instead");
            lock(activity).watch = Watch::Failed;
            return None;
        }
        Err(error) => {
            tracing::warn!(root = %record.id, %error, "arming the watcher failed; the Root is reconciled periodically instead");
            lock(activity).watch = Watch::Failed;
            return None;
        }
    };

    let watches = watcher.watched().len();
    if !state.runtimes.fits(record.id, watches) {
        tracing::warn!(root = %record.id, watches, "watching this Root would exceed the daemon-wide watch ceiling; it is reconciled periodically instead");
        release(Some(watcher)).await;
        lock(activity).watch = Watch::Ceiling;
        return None;
    }
    lock(activity).watch = Watch::Armed(watches);

    Some(watcher)
}

/// Capture the Root `id` as the policy asked, reporting the outcome back to its runtime.
fn capture(
    state: DaemonState,
    id: RootId,
    trigger: Trigger,
    outbox: mpsc::UnboundedSender<Message>,
    activity: Arc<Mutex<Activity>>,
) {
    let captured = tokio::task::spawn_blocking({
        let state = state.clone();
        move || capture_now(&state, id, trigger)
    });

    tokio::spawn(async move {
        let (outcome, coarse_mtime) = captured.await.unwrap_or((PolicyEvent::CaptureFailed, None));
        if let Some(coarse_mtime) = coarse_mtime {
            record_pass(&mut lock(&activity), trigger, coarse_mtime);
        }
        let busy = outcome == PolicyEvent::LockBusy;
        let _ = outbox.send(Message::Event(outcome));

        if busy {
            // Neither blocked on nor dropped: the paths stay with the policy, and one capture covering
            // all of them follows the release (ticket 12). Polled rather than waited on, so a Maintenance
            // Pass holding the lock for minutes pins no thread; a runtime stopped meanwhile ends this too.
            let store = state.store_lock(id);
            while !outbox.is_closed() {
                tokio::time::sleep(LOCK_POLL).await;
                if !matches!(store.try_lock(), Err(TryLockError::WouldBlock)) {
                    let _ = outbox.send(Message::Event(PolicyEvent::LockReleased));
                    break;
                }
            }
        }
    });
}

/// One automatic capture, as the event that reports how it went — with what the pass saw of the
/// filesystem's mtimes, when a pass ran.
fn capture_now(state: &DaemonState, id: RootId, trigger: Trigger) -> (PolicyEvent, Option<bool>) {
    let now = crate::roots::now();
    let (root, location, loaded) = match crate::snapshots::prepare(state, id) {
        Ok(prepared) => prepared,
        Err(problem) => {
            tracing::warn!(root = %id, %trigger, problem = ?problem, "an automatic capture could not start");
            return (PolicyEvent::CaptureFailed, None);
        }
    };

    let store = state.store_lock(id);
    let _held = match store.try_lock() {
        Ok(held) => held,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => return (PolicyEvent::LockBusy, None),
    };
    if !state.registry().contains(id) {
        // Deregistered — and perhaps erased — since it was prepared: opening the store now would bring an
        // erased one back as an orphan.
        return (PolicyEvent::CaptureFailed, None);
    }

    let request = CaptureRequest {
        trigger,
        now,
        verify: loaded.reconcile.verify,
        record_unchanged: false,
    };
    match snapshots::pass(&root, &location, &loaded.policy, &request) {
        Ok(pass) => {
            if let Some(reason) = &pass.index_lag {
                tracing::warn!(root = %id, %reason, "the history index lags");
            }
            let event = match &pass.snapshot {
                Some(captured) => {
                    tracing::info!(root = %id, snapshot = %captured.id, %trigger, hashed = pass.hashed, "captured");
                    crate::snapshots::announce(state, id, captured);
                    PolicyEvent::CaptureFinished
                }
                None => PolicyEvent::CaptureUnchanged,
            };
            (event, Some(pass.coarse_mtime))
        }
        Err(error) => {
            tracing::warn!(root = %id, %trigger, %error, "an automatic capture failed");
            (PolicyEvent::CaptureFailed, None)
        }
    }
}

fn lock(activity: &Mutex<Activity>) -> MutexGuard<'_, Activity> {
    activity
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
