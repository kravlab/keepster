//! Automatic capture (ticket 43): one runtime per Root, folding the watcher's events and its own timers
//! through the policy kernel and doing what it decides.
//!
//! The policy is `keepster_core::decide`, a pure function; this module owns its state and is the only
//! place that turns an [`Action`] into a capture or a timer. Everything that can happen to a Root — a
//! change, a timer, a capture finishing, the store's lock being busy or released, new settings, the stop
//! — reaches the policy as an event on one channel, so the order the policy sees is the order things
//! were handled in, and shutdown is one more event rather than a second code path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use keepster_app::config::Effective;
use keepster_app::snapshots;
use keepster_core::{Action, CapturePolicy, PolicyEvent, PolicyState, RootId, Trigger, decide};
use keepster_store::RootRecord;
use keepster_watch::{WatchEvent, Watcher};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::DaemonState;

/// Every Root's runtime.
#[derive(Debug, Default)]
pub(crate) struct Triggers {
    runtimes: Mutex<HashMap<RootId, Runtime>>,
    /// Set once the daemon is stopping, after which no runtime is started.
    closed: AtomicBool,
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
    /// How many paths wait to be captured.
    pub(crate) pending_paths: u64,
    /// When the oldest of them changed.
    pub(crate) pending_since: Option<Instant>,
}

#[derive(Debug)]
enum Message {
    /// Something for the policy.
    Event(PolicyEvent),
    /// The Root's configuration as it stands now.
    Refresh(Box<Effective>),
}

impl Triggers {
    /// Bring the runtimes in line with the Registry: start one for every Root that can be captured
    /// automatically, hand every running one its current configuration, and stop the rest.
    ///
    /// `roots` is every registered Root with its configuration in force, `None` when it has none — which
    /// leaves the Root uncaptured rather than captured by the defaults (ticket 04).
    pub(crate) fn sync(&self, state: &DaemonState, roots: Vec<(RootRecord, Option<Effective>)>) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let mut runtimes = self.runtimes();

        let keep: Vec<RootId> = roots.iter().map(|(record, _)| record.id).collect();
        runtimes.retain(|id, runtime| {
            let kept = keep.contains(id);
            if !kept {
                runtime.task.abort();
            }
            kept
        });

        for (record, effective) in roots {
            let capturable = record.path.is_dir() && effective.is_some();
            match (runtimes.get(&record.id), effective) {
                (Some(runtime), Some(effective)) if capturable && runtime.path == record.path => {
                    let _ = runtime.inbox.send(Message::Refresh(Box::new(effective)));
                }
                (_, Some(effective)) if capturable => {
                    if let Some(old) = runtimes.remove(&record.id) {
                        old.task.abort();
                    }
                    runtimes.insert(record.id, start(state, record, effective));
                }
                _ => {
                    // A Root that cannot be captured — its directory is gone, or its configuration never
                    // had a valid version — has nothing for a runtime to do until that changes.
                    if let Some(old) = runtimes.remove(&record.id) {
                        old.task.abort();
                    }
                }
            }
        }
    }

    /// What the runtime of the Root `id` reports, if one runs.
    pub(crate) fn activity(&self, id: RootId) -> Option<Activity> {
        self.runtimes()
            .get(&id)
            .map(|runtime| *lock(&runtime.activity))
    }

    /// Stop every runtime as a stopping daemon must: no further Trigger is accepted, the capture in
    /// flight finishes, and every debounce window still counting down is captured now (ticket 05).
    pub(crate) async fn stop(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let runtimes: Vec<Runtime> = self
            .runtimes()
            .drain()
            .map(|(_, runtime)| runtime)
            .collect();

        for runtime in &runtimes {
            let _ = runtime.inbox.send(Message::Event(PolicyEvent::Shutdown));
        }
        for runtime in runtimes {
            let _ = runtime.task.await;
        }
    }

    fn runtimes(&self) -> MutexGuard<'_, HashMap<RootId, Runtime>> {
        // A runtime is either in the map or not; nothing is left half-done by a panic.
        self.runtimes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn start(state: &DaemonState, record: RootRecord, effective: Effective) -> Runtime {
    let (inbox, messages) = mpsc::unbounded_channel();
    let activity = Arc::new(Mutex::new(Activity {
        automatic: automatic(&effective),
        ..Activity::default()
    }));
    let path = record.path.clone();
    let task = tokio::spawn(run(
        state.clone(),
        record,
        effective,
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

/// Whether any Trigger captures the Root without being asked.
fn automatic(effective: &Effective) -> bool {
    effective.triggers.watcher || effective.triggers.periodic.is_some()
}

/// One Root's runtime: until it is stopped, take the next event — a message, or the timer the policy
/// armed — through the policy, and do what it decides.
async fn run(
    state: DaemonState,
    record: RootRecord,
    effective: Effective,
    outbox: mpsc::UnboundedSender<Message>,
    mut inbox: mpsc::UnboundedReceiver<Message>,
    activity: Arc<Mutex<Activity>>,
) {
    let origin = state.started;
    let mut rules = effective.policy.clone();
    let mut settings = effective.triggers;
    let mut watcher = watch(&record.path, &rules, settings.watcher, &outbox).await;
    let mut policy = PolicyState::new(settings, origin.elapsed());
    let mut deadline: Option<Instant> = None;
    // Asked once at the start, which is how a periodic floor arms its first timer.
    let mut next = Some(PolicyEvent::Timer);

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
                        None => break,
                        Some(Message::Event(event)) => event,
                        Some(Message::Refresh(effective)) => {
                            // A change to what classifies paths, or to whether the watcher is on, means
                            // different watches; the policy only needs the intervals.
                            if effective.policy != rules || effective.triggers.watcher != settings.watcher {
                                drop(watcher.take());
                                rules = effective.policy.clone();
                                watcher = watch(&record.path, &rules, effective.triggers.watcher, &outbox).await;
                            }
                            lock(&activity).automatic = automatic(&effective);
                            if effective.triggers == settings {
                                continue;
                            }
                            settings = effective.triggers;
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

        let (actions, decided) = decide(policy, event, origin.elapsed());
        policy = decided;
        for action in actions {
            match action {
                Action::Capture { trigger, .. } => {
                    capture(state.clone(), record.id, trigger, outbox.clone());
                }
                Action::ArmTimer { at } => deadline = Some(origin + at),
            }
        }

        {
            let mut activity = lock(&activity);
            activity.pending_paths = u64::try_from(policy.pending_paths()).unwrap_or(u64::MAX);
            activity.pending_since = policy.pending_since().map(|since| origin + since);
        }
        if policy.stopped() {
            break;
        }
    }

    if let Some(watcher) = watcher {
        // Dropping a watcher joins its thread.
        let _ = tokio::task::spawn_blocking(move || drop(watcher)).await;
    }
}

/// Start watching the Root at `path` under `rules`, its events going to `outbox`, when the watcher
/// Trigger is on.
///
/// A Root that cannot be watched is still captured by its other Triggers and by Reconciliation: the
/// watcher is a latency optimisation, so failing to arm it is said, not fatal.
async fn watch(
    path: &Path,
    rules: &CapturePolicy,
    on: bool,
    outbox: &mpsc::UnboundedSender<Message>,
) -> Option<Watcher> {
    if !on {
        return None;
    }
    let (path, rules, outbox) = (path.to_path_buf(), rules.clone(), outbox.clone());

    // Arming walks the tree, which blocks.
    let started = tokio::task::spawn_blocking(move || {
        Watcher::start(&path, &rules, move |event| {
            let WatchEvent::Changed(paths) = event;
            let _ = outbox.send(Message::Event(PolicyEvent::Changed(paths)));
        })
    })
    .await;

    match started {
        Ok(Ok(watcher)) => Some(watcher),
        Ok(Err(error)) => {
            tracing::warn!(%error, "the Root could not be watched; its other Triggers still capture it");
            None
        }
        Err(error) => {
            tracing::warn!(%error, "arming the watcher failed");
            None
        }
    }
}

/// Capture the Root `id` as the policy asked, reporting the outcome back to its runtime.
fn capture(
    state: DaemonState,
    id: RootId,
    trigger: Trigger,
    outbox: mpsc::UnboundedSender<Message>,
) {
    tokio::task::spawn_blocking(move || {
        let outcome = capture_now(&state, id, trigger);
        let busy = outcome == PolicyEvent::LockBusy;
        let _ = outbox.send(Message::Event(outcome));

        if busy {
            // Neither blocked on nor dropped: the paths stay with the policy, and one capture covering
            // all of them follows the release (ticket 12).
            let store = state.store_lock(id);
            drop(
                store
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
            let _ = outbox.send(Message::Event(PolicyEvent::LockReleased));
        }
    });
}

/// One automatic capture, as the event that reports how it went.
fn capture_now(state: &DaemonState, id: RootId, trigger: Trigger) -> PolicyEvent {
    let now = crate::roots::now();
    let (root, location, loaded) = match crate::snapshots::prepare(state, id) {
        Ok(prepared) => prepared,
        Err(problem) => {
            tracing::warn!(root = %id, %trigger, problem = ?problem, "an automatic capture could not start");
            return PolicyEvent::CaptureFailed;
        }
    };

    let store = state.store_lock(id);
    let _held = match store.try_lock() {
        Ok(held) => held,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => return PolicyEvent::LockBusy,
    };

    match snapshots::capture_if_changed(&root, &location, &loaded.policy, trigger, now) {
        Ok(Some(captured)) => {
            tracing::info!(root = %id, snapshot = %captured.id, %trigger, "captured");
            if let Some(reason) = &captured.index_lag {
                tracing::warn!(root = %id, snapshot = %captured.id, %reason, "the history index lags");
            }
            PolicyEvent::CaptureFinished
        }
        Ok(None) => PolicyEvent::CaptureFinished,
        Err(error) => {
            tracing::warn!(root = %id, %trigger, %error, "an automatic capture failed");
            PolicyEvent::CaptureFailed
        }
    }
}

fn lock(activity: &Mutex<Activity>) -> MutexGuard<'_, Activity> {
    activity
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
