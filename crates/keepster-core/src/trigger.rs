//! The trigger policy: when a Root's changes become a Snapshot (ticket 43).
//!
//! A pure function over values, `decide(state, event, now) -> (actions, state)`, with the state owned by
//! the daemon. The shape is forced by what filesystem events are: lost, duplicated and reordered, so the
//! test that matters is "given these events at these times, exactly these Snapshots happen" — a fold over
//! `(event, time)` pairs with no watcher, no clock and no waiting (ticket 08, ADR 0003). Shutdown is an
//! event through the same function, not a second code path.
//!
//! Time is a [`Duration`] since an origin the caller chooses — the daemon's start — because only
//! intervals matter here and a monotonic clock is the right one for them.

use std::collections::BTreeSet;
use std::time::Duration;

use crate::path::RootPath;
use crate::snapshot::Trigger;

/// Which Triggers are active, and the intervals they keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Triggers {
    /// Whether a change the watcher reports leads to a Snapshot.
    pub watcher: bool,
    /// The periodic floor: a Snapshot at least this often, whatever the watcher saw. Off when `None`.
    pub periodic: Option<Duration>,
    /// How long a Root must stay quiet after its last change before the change is captured, so that a
    /// burst of saves is one Snapshot.
    pub debounce: Duration,
    /// The shortest time between two Snapshots of one Root, always in force, so a noisy build is not a
    /// Snapshot per second.
    pub rate_limit: Duration,
    /// How often a Root that follows links passes over their targets, which no watch covers (ticket 49).
    /// `None` for a Root that follows none. Whatever Triggers are active, as Reconciliation is.
    pub follow_every: Option<Duration>,
}

impl Default for Triggers {
    /// The documented defaults: the watcher with a 3 s debounce, one Snapshot a minute at most, no
    /// periodic floor (ticket 04).
    fn default() -> Self {
        Self {
            watcher: true,
            periodic: None,
            debounce: Duration::from_secs(3),
            rate_limit: Duration::from_secs(60),
            follow_every: None,
        }
    }
}

/// Why a duration was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{0}` is not a duration: write a whole number with a unit, such as `500ms`, `3s`, `10m`, `1h` or `1d`"
)]
pub struct InvalidDuration(pub String);

/// Read a duration such as `500ms`, `3s`, `10m`, `1h` or `1d`. The unit is required — a bare `3` could
/// mean seconds or minutes — and only whole numbers are accepted, as for sizes.
pub fn parse_duration(text: &str) -> Result<Duration, InvalidDuration> {
    let invalid = || InvalidDuration(text.to_owned());

    let trimmed = text.trim();
    let digits = trimmed
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(digits);
    let number: u64 = number.parse().map_err(|_| invalid())?;

    let milliseconds: u64 = match unit.trim() {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(invalid()),
    };

    number
        .checked_mul(milliseconds)
        .map(Duration::from_millis)
        .ok_or_else(invalid)
}

/// A duration in the largest unit [`parse_duration`] reads that writes it as a whole number, so what is
/// shown is what a person would have written.
pub fn format_duration(duration: Duration) -> String {
    let milliseconds = duration.as_millis();
    let units = [
        ("d", 86_400_000),
        ("h", 3_600_000),
        ("m", 60_000),
        ("s", 1_000),
    ];

    units
        .iter()
        .find(|(_, size)| milliseconds != 0 && milliseconds.is_multiple_of(*size))
        .map_or_else(
            || format!("{milliseconds}ms"),
            |(unit, size)| format!("{}{unit}", milliseconds / size),
        )
}

/// What happened, as the policy is told it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyEvent {
    /// The watcher saw these paths change. The list may be empty when the paths could not be named:
    /// a change is a change.
    Changed(Vec<RootPath>),
    /// A timer the policy armed has fired — or the daemon asks what is due, as it does once at the start.
    Timer,
    /// A Reconciliation is owed: the daemon started, the platform lost events, or the rules changed. It
    /// is owed whatever Triggers are active, because it is the guarantee rather than a Trigger the user
    /// chooses, and it takes whatever changes are pending with it.
    Reconcile,
    /// The capture last asked for has been written.
    CaptureFinished,
    /// The capture last asked for found nothing to write. No Snapshot was taken, so the rate limit does
    /// not count from it.
    CaptureUnchanged,
    /// The capture last asked for failed; what it covered is still owed — unless the daemon is stopping,
    /// when the one flush it gets has been spent.
    CaptureFailed,
    /// The capture last asked for could not start because the store's lock is held — by a manual capture
    /// or a Maintenance Pass. It is neither blocked on nor dropped: it waits for [`Self::LockReleased`].
    LockBusy,
    /// The lock a deferred capture was waiting for is free.
    LockReleased,
    /// The configuration changed which Triggers are active, or their intervals.
    Settings(Triggers),
    /// The daemon is stopping: no further change is accepted, and what is pending is captured now.
    Shutdown,
    /// A pass over the Root finished — whoever asked for it — and either read everything it meant to or
    /// did not. One that did not is retried, with a backoff that doubles while passes stay incomplete
    /// and starts again after one that is complete (ADR 0004). Reported before the capture's own outcome.
    PassOutcome {
        /// Whether something it meant to read could not be read.
        incomplete: bool,
    },
}

/// How long after an incomplete pass the Root is passed over again, at first.
pub const RETRY_AFTER: Duration = Duration::from_secs(30);

/// The longest the backoff between retries grows to.
pub const RETRY_CEILING: Duration = Duration::from_secs(60 * 60);

/// What the daemon should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Capture the Root now, under its store's lock, and report back with [`PolicyEvent::CaptureFinished`],
    /// [`PolicyEvent::CaptureFailed`] or [`PolicyEvent::LockBusy`].
    Capture {
        /// The Trigger the Snapshot is recorded as.
        trigger: Trigger,
        /// The paths seen changing since the last Snapshot. A capture walks the whole Root regardless —
        /// the watcher is never the correctness mechanism — so this is what prompted it, not its scope.
        paths: BTreeSet<RootPath>,
    },
    /// Deliver [`PolicyEvent::Timer`] at this time, replacing any timer armed before.
    ArmTimer {
        /// When.
        at: Duration,
    },
}

/// Everything the policy remembers about one Root between events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyState {
    settings: Triggers,
    /// What changed since the last Snapshot, if anything did.
    pending: Option<BTreeSet<RootPath>>,
    /// When the last change arrived: the debounce window counts from here.
    last_change: Option<Duration>,
    /// When the oldest change still pending arrived.
    pending_since: Option<Duration>,
    /// The capture asked for and not yet reported back.
    in_flight: Option<InFlight>,
    /// When the last Snapshot was asked for: the rate limit counts from here.
    last_capture: Option<Duration>,
    /// When the periodic floor is next due.
    next_periodic: Option<Duration>,
    /// When the followed targets are next due to be passed over.
    next_follow: Option<Duration>,
    /// Whether a Reconciliation is owed.
    reconcile: bool,
    /// Whether a capture is waiting for the store's lock.
    waiting_for_lock: bool,
    /// Whether the daemon is stopping.
    stopping: bool,
    /// The first wait before an incomplete pass is retried.
    retry_after: Duration,
    /// The retry owed after an incomplete pass, if one is.
    retry: Option<Retry>,
}

/// A pass owed because the last one was incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Retry {
    /// When it is due.
    at: Duration,
    /// How long it waited, which the next one doubles.
    backoff: Duration,
}

impl PolicyState {
    /// A Root's policy, starting at `now` with nothing pending.
    pub fn new(settings: Triggers, now: Duration) -> Self {
        Self {
            settings,
            pending: None,
            last_change: None,
            pending_since: None,
            in_flight: None,
            last_capture: None,
            next_periodic: settings.periodic.map(|every| now + every),
            next_follow: settings.follow_every.map(|every| now + every),
            reconcile: false,
            waiting_for_lock: false,
            stopping: false,
            retry_after: RETRY_AFTER,
            retry: None,
        }
    }

    /// This policy, retrying an incomplete pass first after `first` rather than [`RETRY_AFTER`] — which
    /// only a test has reason to want.
    pub fn retrying_after(mut self, first: Duration) -> Self {
        self.retry_after = first;
        self
    }

    /// Whether a stopping Root has nothing left to capture, so the daemon may finish stopping it.
    pub fn stopped(&self) -> bool {
        self.stopping
            && self.pending.is_none()
            && self.in_flight.is_none()
            && !self.waiting_for_lock
    }

    /// How many paths are waiting to be captured, the one being captured excluded.
    pub fn pending_paths(&self) -> usize {
        self.pending.as_ref().map_or(0, BTreeSet::len)
    }

    /// When the oldest change still waiting to be captured arrived, if one is.
    pub fn pending_since(&self) -> Option<Duration> {
        self.pending.as_ref().and(self.pending_since)
    }
}

/// A capture asked for: what it covers, when it was asked for, and since when that was pending — all
/// owed again if it does not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InFlight {
    trigger: Trigger,
    paths: BTreeSet<RootPath>,
    asked: Duration,
    pending_since: Option<Duration>,
}

/// The next state, and what to do, after `event` at `now`.
pub fn decide(
    mut state: PolicyState,
    event: PolicyEvent,
    now: Duration,
) -> (Vec<Action>, PolicyState) {
    match event {
        PolicyEvent::Changed(paths) => {
            if state.stopping || !state.settings.watcher {
                return (Vec::new(), state);
            }
            state.pending.get_or_insert_default().extend(paths);
            state.pending_since.get_or_insert(now);
            state.last_change = Some(now);
        }
        PolicyEvent::Timer => {}
        PolicyEvent::Reconcile => {
            if !state.stopping {
                state.reconcile = true;
            }
        }
        PolicyEvent::CaptureFinished => {
            if let Some(in_flight) = state.in_flight.take() {
                state.last_capture = Some(in_flight.asked);
            }
        }
        PolicyEvent::CaptureUnchanged => {
            state.in_flight = None;
        }
        PolicyEvent::CaptureFailed => {
            if let Some(in_flight) = state.in_flight.take() {
                state.last_capture = Some(in_flight.asked);
                // A capture failing while the daemon stops would fail again at once, and the stop would
                // never finish; Reconciliation at the next start covers what it missed.
                if !state.stopping {
                    owe(&mut state, in_flight);
                }
            }
        }
        PolicyEvent::LockBusy => {
            // Not a Snapshot, so it does not count against the rate limit.
            if let Some(in_flight) = state.in_flight.take() {
                owe(&mut state, in_flight);
            }
            state.waiting_for_lock = true;
        }
        PolicyEvent::LockReleased => state.waiting_for_lock = false,
        PolicyEvent::Settings(settings) => {
            if settings.periodic != state.settings.periodic {
                state.next_periodic = settings.periodic.map(|every| now + every);
            }
            if settings.follow_every != state.settings.follow_every {
                state.next_follow = settings.follow_every.map(|every| now + every);
            }
            state.settings = settings;
        }
        PolicyEvent::Shutdown => state.stopping = true,
        PolicyEvent::PassOutcome { incomplete } => {
            state.retry = (incomplete && !state.stopping).then(|| {
                let backoff = state.retry.map_or(state.retry_after, |retry| {
                    // Never below the first wait, which a test may set past the ceiling.
                    (retry.backoff * 2)
                        .min(RETRY_CEILING)
                        .max(state.retry_after)
                });
                Retry {
                    at: now + backoff,
                    backoff,
                }
            });
        }
    }

    let actions = due(&mut state, now);
    (actions, state)
}

/// Owe again, at once, what a capture that did not happen was for: its changes, the Reconciliation it
/// was, or the periodic floor — which stays a periodic capture, so a Trigger the user switched off never
/// fires in its place.
fn owe(state: &mut PolicyState, in_flight: InFlight) {
    match in_flight.trigger {
        Trigger::Reconcile => state.reconcile = true,
        Trigger::Periodic if in_flight.paths.is_empty() => {
            state.next_periodic = Some(
                state
                    .next_periodic
                    .map_or(in_flight.asked, |next| next.min(in_flight.asked)),
            );
        }
        _ => {}
    }
    if in_flight.paths.is_empty() && in_flight.trigger != Trigger::Watcher {
        return;
    }

    state
        .pending
        .get_or_insert_default()
        .extend(in_flight.paths);
    state.pending_since = match (state.pending_since, in_flight.pending_since) {
        (Some(later), Some(earlier)) => Some(later.min(earlier)),
        (later, earlier) => later.or(earlier).or(Some(in_flight.asked)),
    };
    // Its debounce window had already closed.
    state.last_change.get_or_insert(Duration::ZERO);
}

/// Capture if a capture is due, otherwise arm the timer for when one will be.
fn due(state: &mut PolicyState, now: Duration) -> Vec<Action> {
    if state.in_flight.is_some() || state.waiting_for_lock {
        return Vec::new();
    }

    if state.stopping {
        // The likeliest moment for uncaptured work to exist: the user's most recent edits are flushed now,
        // bypassing the debounce and the rate limit (ticket 05). Nothing is armed after.
        return match state.pending.take() {
            Some(paths) => vec![capture(state, Trigger::Watcher, paths, now)],
            None => Vec::new(),
        };
    }

    let allowed = state
        .last_capture
        .map_or(Duration::ZERO, |last| last + state.settings.rate_limit);
    // The window closes once the Root has been quiet for the debounce — or, for a file that is never
    // quiet, such as a log or a watch-mode build, once its oldest change has waited as long as the rate
    // limit would make it wait anyway. The rate limit is what caps how often; the debounce only gathers.
    let longest = state.settings.debounce.max(state.settings.rate_limit);
    let quiet = state.pending.as_ref().and(state.last_change).map(|last| {
        let gathered = last + state.settings.debounce;
        state
            .pending_since
            .map_or(gathered, |since| gathered.min(since + longest))
    });
    let candidates = [
        state.reconcile.then_some((allowed, Trigger::Reconcile)),
        quiet.map(|at| (at.max(allowed), Trigger::Watcher)),
        state
            .next_periodic
            .map(|at| (at.max(allowed), Trigger::Periodic)),
        state
            .retry
            .map(|retry| (retry.at.max(allowed), Trigger::Retry)),
        // The guarantee for what no watch covers, so recorded as the Reconciliation it is.
        state
            .next_follow
            .map(|at| (at.max(allowed), Trigger::Reconcile)),
    ];

    match candidates.into_iter().flatten().min_by_key(|(at, _)| *at) {
        Some((at, trigger)) if at <= now => {
            // Every capture is the same walk, so the label is the best account of why: a Reconciliation
            // owed first, then a change seen, then the periodic floor.
            let trigger = if state.reconcile {
                Trigger::Reconcile
            } else if quiet.is_some_and(|quiet| quiet.max(allowed) <= now) {
                Trigger::Watcher
            } else {
                trigger
            };
            let paths = state.pending.take().unwrap_or_default();
            vec![capture(state, trigger, paths, now)]
        }
        Some((at, _)) => vec![Action::ArmTimer { at }],
        None => Vec::new(),
    }
}

fn capture(
    state: &mut PolicyState,
    trigger: Trigger,
    paths: BTreeSet<RootPath>,
    now: Duration,
) -> Action {
    state.last_change = None;
    state.reconcile = false;
    state.in_flight = Some(InFlight {
        trigger,
        paths: paths.clone(),
        asked: now,
        pending_since: state.pending_since.take(),
    });
    // Any Snapshot is the floor's Snapshot: it walks the whole Root, so the floor counts again from here.
    state.next_periodic = state.settings.periodic.map(|every| now + every);
    state.next_follow = state.settings.follow_every.map(|every| now + every);

    Action::Capture { trigger, paths }
}
