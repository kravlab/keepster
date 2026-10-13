//! The alarms (ticket 67): what stopped the insurance, decided here and only here. A tray delivers what
//! this module decides and originates nothing (ADR 0008).

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use keepster_api::{
    ALARM_JOB_FAILED, ALARM_ROOT_DEGRADED, ALARM_ROOT_UNAVAILABLE, Alarm, ErrorCode, Event,
    MuteRequest, MuteStatus, Problem, RootState,
};

use crate::DaemonState;
use crate::problem::ProblemResponse;

/// How long changes are gathered before they are announced together: a Root that flaps on a loose cable
/// is one notice, and one that is gone and back inside the window is none.
pub(crate) const WINDOW: Duration = Duration::from_secs(5);

/// Which alarm of a Root an alarm is: the one for its state, or the one for each kind of job. A new alarm
/// in the same slot replaces the one before, which is how recovery replaces rather than adds.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Slot {
    /// The alarm about the Root's state: `unavailable` or `degraded`.
    State,
    /// The alarm about a job of this kind on the Root.
    Job(String),
}

/// An alarm's place: its Root, then its slot.
type Key = (String, Slot);

/// The alarms in force, and the ones announced last.
///
/// Every change lands in `current` at once; `flush` announces the difference from `announced` once the
/// window that opened with the first unannounced change has passed. Pure: the caller hands in the time.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    /// The alarms the last announcement said are in force.
    announced: BTreeMap<Key, Alarm>,
    /// The alarms in force now, announced or not.
    current: BTreeMap<Key, Alarm>,
    /// When the oldest change not yet announced was made; `None` while there is none.
    opened: Option<Instant>,
}

/// What one announcement says changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Change {
    /// The alarms announced for the first time, each replacing any in its slot.
    pub(crate) raised: Vec<Alarm>,
    /// The alarms that ended with nothing in their slot to replace them.
    pub(crate) cleared: Vec<Alarm>,
}

impl Ledger {
    /// Every registered Root's state as it stands, as `(id, state, state_code)`.
    ///
    /// The ledger starts empty, which is every Root watching: a Root first seen in another state — the
    /// one on an unplugged disk after a restart — raises its alarm like any change would. A Root not
    /// among `roots` is no longer registered, and its alarms go with it.
    pub(crate) fn observe_roots<'a, I>(&mut self, roots: I, at: Instant)
    where
        I: IntoIterator<Item = (&'a str, RootState, Option<&'a str>)>,
    {
        let mut seen = std::collections::BTreeSet::new();
        for (root, state, code) in roots {
            seen.insert(root.to_owned());
            let key = (root.to_owned(), Slot::State);
            let kind = match state {
                RootState::Watching => {
                    self.current.remove(&key);
                    continue;
                }
                RootState::Degraded => ALARM_ROOT_DEGRADED,
                RootState::Unavailable => ALARM_ROOT_UNAVAILABLE,
            };
            let alarm = Alarm {
                kind: kind.to_owned(),
                root: root.to_owned(),
                reason: code.map(str::to_owned),
                job: None,
                job_kind: None,
            };
            self.current.insert(key, alarm);
        }
        self.current.retain(|(root, _), _| seen.contains(root));
        self.touched(at);
    }

    /// The job `job`, of `kind`, on `root`, ended: failed, or succeeded.
    pub(crate) fn job_ended(
        &mut self,
        kind: &str,
        root: &str,
        job: &str,
        failed: bool,
        at: Instant,
    ) {
        let key = (root.to_owned(), Slot::Job(kind.to_owned()));
        if failed {
            let alarm = Alarm {
                kind: ALARM_JOB_FAILED.to_owned(),
                root: root.to_owned(),
                reason: None,
                job: Some(job.to_owned()),
                job_kind: Some(kind.to_owned()),
            };
            self.current.insert(key, alarm);
        } else {
            self.current.remove(&key);
        }
        self.touched(at);
    }

    /// What changed since the last announcement, once the window has passed; `None` before then, or
    /// when whatever changed has changed back.
    pub(crate) fn flush(&mut self, at: Instant) -> Option<Change> {
        let opened = self.opened?;
        if at.saturating_duration_since(opened) < WINDOW {
            return None;
        }
        self.opened = None;
        let raised: Vec<Alarm> = self
            .current
            .iter()
            .filter(|(key, alarm)| self.announced.get(*key) != Some(*alarm))
            .map(|(_, alarm)| alarm.clone())
            .collect();
        let cleared: Vec<Alarm> = self
            .announced
            .iter()
            .filter(|(key, alarm)| self.current.get(*key) != Some(*alarm))
            .map(|(_, alarm)| alarm.clone())
            .collect();
        self.announced = self.current.clone();

        (!raised.is_empty() || !cleared.is_empty()).then_some(Change { raised, cleared })
    }

    /// Open the window at `at` if a change is now unannounced and none was before.
    fn touched(&mut self, at: Instant) {
        if self.opened.is_none() && self.current != self.announced {
            self.opened = Some(at);
        }
    }

    /// The alarms in force as last announced: what `status` shows, so that it agrees with the stream.
    pub(crate) fn active(&self) -> Vec<Alarm> {
        self.announced.values().cloned().collect()
    }
}

/// The alarms and the mute, as the daemon holds them: a leaf in the lock order, taken with nothing that
/// waits on it and holding nothing while it is held but the event buffer.
#[derive(Debug, Default)]
pub(crate) struct Alarms {
    /// The ledger and the mute, changed together under one lock.
    inner: Mutex<Inner>,
}

/// What [`Alarms`] guards.
#[derive(Debug, Default)]
struct Inner {
    /// The alarms in force and the ones announced last.
    ledger: Ledger,
    /// The wall-clock moment the mute ends. A monotonic clock stops while a laptop sleeps, and a mute
    /// is a promise about the moment it ends, as a raised log level is.
    muted_until: Option<SystemTime>,
}

impl Alarms {
    /// Every registered Root as `status` reports it: see [`Ledger::observe_roots`].
    pub(crate) fn observe_roots(&self, roots: &[keepster_api::Root], at: Instant) {
        self.inner().ledger.observe_roots(
            roots
                .iter()
                .map(|root| (root.id.as_str(), root.state, root.state_code.as_deref())),
            at,
        );
    }

    /// See [`Ledger::job_ended`].
    pub(crate) fn job_ended(&self, kind: &str, root: &str, job: &str, failed: bool, at: Instant) {
        self.inner().ledger.job_ended(kind, root, job, failed, at);
    }

    /// The `alarms.changed` owed at `at`: the alarms' window has passed with something changed, or the
    /// mute has ended at `now`.
    pub(crate) fn flush(&self, at: Instant, now: SystemTime) -> Option<Event> {
        let mut inner = self.inner();
        let change = inner.ledger.flush(at);
        let unmuted = inner.muted_until.is_some_and(|until| now >= until);
        if unmuted {
            inner.muted_until = None;
        }
        if change.is_none() && !unmuted {
            return None;
        }

        Some(inner.announcement(change.unwrap_or_default()))
    }

    /// Mute notifications until `until`, replacing a mute in force; the `alarms.changed` that says so.
    pub(crate) fn mute(&self, until: SystemTime) -> Event {
        let mut inner = self.inner();
        inner.muted_until = Some(until);
        inner.announcement(Change::default())
    }

    /// Lift the mute; the `alarms.changed` that says so.
    pub(crate) fn unmute(&self) -> Event {
        let mut inner = self.inner();
        inner.muted_until = None;
        inner.announcement(Change::default())
    }

    /// The alarms in force, as last announced.
    pub(crate) fn active(&self) -> Vec<Alarm> {
        self.inner().ledger.active()
    }

    /// Until when notifications are muted at `now`, while they are.
    pub(crate) fn muted_until(&self, now: SystemTime) -> Option<SystemTime> {
        self.inner().muted_until.filter(|until| now < *until)
    }

    /// The ledger and the mute, locked; a poisoned lock is recovered rather than passed on as a panic.
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Inner {
    /// `change` as the event that announces it, with every alarm in force and the mute.
    fn announcement(&self, change: Change) -> Event {
        Event::AlarmsChanged {
            alarms: self.ledger.active(),
            raised: change.raised,
            cleared: change.cleared,
            muted_until: self.muted_until.and_then(crate::roots::rfc3339),
        }
    }
}

/// Announce whatever the alarms owe now: called on every reload pass, once a second.
pub(crate) fn announce_due(state: &DaemonState) {
    if let Some(event) = state.alarms.flush(Instant::now(), crate::roots::now()) {
        state.publish(event);
    }
}

/// Mute the notifications for a while.
#[utoipa::path(
    put,
    path = "/v1/alarms/mute",
    operation_id = "mute_alarms",
    tag = "status",
    request_body = MuteRequest,
    responses(
        (status = 200, description = "Notifications are muted until `until`, then not by themselves; `status` shows it for as long as it lasts, and `alarms.changed` announces it. Alarms are still raised and listed while it lasts. A later mute replaces this one and its end.", body = MuteStatus),
        (status = 400, description = "`minutes` outside 1 to 1440 (`mute_invalid`), or a body that is not this JSON (`body_invalid`).", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn mute(
    State(state): State<DaemonState>,
    body: Result<Json<MuteRequest>, JsonRejection>,
) -> Result<Json<MuteStatus>, ProblemResponse> {
    let Json(request) = body.map_err(|rejection| {
        ProblemResponse::new(
            rejection.status(),
            ErrorCode::BodyInvalid,
            rejection.body_text(),
        )
    })?;
    let duration = mute_duration(request.minutes).map_err(|detail| {
        ProblemResponse::new(StatusCode::BAD_REQUEST, ErrorCode::MuteInvalid, detail)
    })?;
    let until = crate::roots::now() + duration;

    state.publish(state.alarms.mute(until));

    Ok(Json(MuteStatus {
        until: crate::roots::rfc3339(until),
    }))
}

/// Lift the mute now.
#[utoipa::path(
    delete,
    path = "/v1/alarms/mute",
    operation_id = "unmute_alarms",
    tag = "status",
    responses(
        (status = 200, description = "Notifications are no longer muted, and `alarms.changed` announces it. Lifting no mute is no error.", body = MuteStatus),
    ),
)]
pub(crate) async fn unmute(State(state): State<DaemonState>) -> Json<MuteStatus> {
    state.publish(state.alarms.unmute());

    Json(MuteStatus { until: None })
}

/// How long a mute asked for `minutes` lasts: an hour when not said, never more than a day, so a mute
/// forgotten is not a state anyone can reach. The refusal says why.
pub(crate) fn mute_duration(minutes: Option<i64>) -> Result<Duration, String> {
    const LONGEST: i64 = 24 * 60;

    match minutes.unwrap_or(DEFAULT_MUTE_MINUTES) {
        minutes @ 1..=LONGEST => Ok(Duration::from_secs(minutes.unsigned_abs() * 60)),
        minutes => Err(format!(
            "`minutes` is {minutes}; a mute lasts 1 to 1440 minutes"
        )),
    }
}

/// How long a mute lasts when nobody says.
const DEFAULT_MUTE_MINUTES: i64 = 60;

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant, SystemTime};

    use keepster_api::{Alarm, Event, RootState};

    use super::{Alarms, Change, Ledger, WINDOW, mute_duration};

    fn state_alarm(kind: &str, root: &str, reason: &str) -> Alarm {
        Alarm {
            kind: kind.to_owned(),
            root: root.to_owned(),
            reason: Some(reason.to_owned()),
            job: None,
            job_kind: None,
        }
    }

    fn unavailable(root: &str) -> Alarm {
        state_alarm("root_unavailable", root, "directory_missing")
    }

    fn degraded(root: &str) -> Alarm {
        state_alarm("root_degraded", root, "config_invalid")
    }

    fn job_failed(root: &str, job: &str, kind: &str) -> Alarm {
        Alarm {
            kind: "job_failed".to_owned(),
            root: root.to_owned(),
            reason: None,
            job: Some(job.to_owned()),
            job_kind: Some(kind.to_owned()),
        }
    }

    const MISSING: (RootState, Option<&str>) = (RootState::Unavailable, Some("directory_missing"));
    const INVALID: (RootState, Option<&str>) = (RootState::Degraded, Some("config_invalid"));
    const FINE: (RootState, Option<&str>) = (RootState::Watching, None);

    /// One pass over the Roots `roots`, each with its state.
    fn pass(ledger: &mut Ledger, roots: &[(&str, (RootState, Option<&str>))], at: Instant) {
        ledger.observe_roots(
            roots
                .iter()
                .map(|(root, (state, code))| (*root, *state, *code)),
            at,
        );
    }

    /// A ledger whose alarms for `roots` are already announced.
    fn announced(roots: &[(&str, (RootState, Option<&str>))], at: Instant) -> Ledger {
        let mut ledger = Ledger::default();
        pass(&mut ledger, roots, at);
        ledger.flush(at + WINDOW).expect("announced");
        ledger
    }

    #[test]
    fn a_root_first_seen_unavailable_raises_its_alarm_once_the_window_has_passed() {
        let start = Instant::now();
        let mut ledger = Ledger::default();

        pass(&mut ledger, &[("a", MISSING)], start);

        assert_eq!(
            ledger.flush(start + WINDOW - Duration::from_millis(1)),
            None
        );
        assert_eq!(
            ledger.active(),
            [],
            "nothing is in force before it is announced"
        );
        assert_eq!(
            ledger.flush(start + WINDOW),
            Some(Change {
                raised: vec![unavailable("a")],
                cleared: vec![],
            })
        );
        assert_eq!(ledger.active(), [unavailable("a")]);
    }

    #[test]
    fn a_root_that_is_watching_raises_nothing() {
        let start = Instant::now();
        let mut ledger = Ledger::default();

        pass(&mut ledger, &[("a", FINE)], start);

        assert_eq!(ledger.flush(start + WINDOW), None);
    }

    #[test]
    fn a_state_that_stays_as_announced_is_not_announced_again() {
        let start = Instant::now();
        let mut ledger = announced(&[("a", MISSING)], start);

        pass(&mut ledger, &[("a", MISSING)], start + WINDOW);

        assert_eq!(ledger.flush(start + WINDOW * 3), None);
        assert_eq!(ledger.active(), [unavailable("a")]);
    }

    #[test]
    fn a_return_to_watching_clears_the_alarm() {
        let start = Instant::now();
        let mut ledger = announced(&[("a", MISSING)], start);
        let later = start + WINDOW * 2;

        pass(&mut ledger, &[("a", FINE)], later);

        assert_eq!(
            ledger.flush(later + WINDOW),
            Some(Change {
                raised: vec![],
                cleared: vec![unavailable("a")],
            })
        );
        assert_eq!(ledger.active(), []);
    }

    #[test]
    fn degraded_then_unavailable_replaces_the_alarm_rather_than_adding_one() {
        let start = Instant::now();
        let mut ledger = announced(&[("a", INVALID)], start);
        let later = start + WINDOW * 2;

        pass(&mut ledger, &[("a", MISSING)], later);

        assert_eq!(
            ledger.flush(later + WINDOW),
            Some(Change {
                raised: vec![unavailable("a")],
                cleared: vec![degraded("a")],
            })
        );
        assert_eq!(ledger.active(), [unavailable("a")]);
    }

    #[test]
    fn an_alarm_raised_and_cleared_inside_one_window_is_never_announced() {
        let start = Instant::now();
        let mut ledger = Ledger::default();

        pass(&mut ledger, &[("a", MISSING)], start);
        pass(&mut ledger, &[("a", FINE)], start + Duration::from_secs(2));

        assert_eq!(ledger.flush(start + WINDOW), None);
        assert_eq!(ledger.active(), []);
    }

    #[test]
    fn changes_inside_one_window_are_announced_together() {
        let start = Instant::now();
        let mut ledger = Ledger::default();

        pass(&mut ledger, &[("a", MISSING), ("b", FINE)], start);
        pass(
            &mut ledger,
            &[("a", MISSING), ("b", INVALID)],
            start + Duration::from_secs(3),
        );

        assert_eq!(
            ledger.flush(start + WINDOW),
            Some(Change {
                raised: vec![unavailable("a"), degraded("b")],
                cleared: vec![],
            })
        );
    }

    #[test]
    fn a_root_no_longer_registered_takes_its_alarms_with_it() {
        let start = Instant::now();
        let mut ledger = announced(&[("a", MISSING), ("b", FINE)], start);
        let later = start + WINDOW * 2;
        ledger.job_ended("export", "a", "j1", true, later);

        pass(&mut ledger, &[("b", FINE)], later);

        assert_eq!(
            ledger.flush(later + WINDOW),
            Some(Change {
                raised: vec![],
                cleared: vec![unavailable("a")],
            })
        );
        assert_eq!(ledger.active(), []);
    }

    #[test]
    fn a_failed_job_raises_and_the_next_success_of_its_kind_on_its_root_clears_it() {
        let start = Instant::now();
        let mut ledger = Ledger::default();

        ledger.job_ended("maintenance", "a", "j1", true, start);
        assert_eq!(
            ledger.flush(start + WINDOW),
            Some(Change {
                raised: vec![job_failed("a", "j1", "maintenance")],
                cleared: vec![],
            })
        );

        let later = start + WINDOW * 2;
        ledger.job_ended("export", "a", "j2", false, later);
        ledger.job_ended("maintenance", "b", "j3", false, later);
        assert_eq!(
            ledger.flush(later + WINDOW),
            None,
            "a success of another kind, or on another Root, clears nothing"
        );

        let latest = later + WINDOW * 2;
        ledger.job_ended("maintenance", "a", "j4", false, latest);
        assert_eq!(
            ledger.flush(latest + WINDOW),
            Some(Change {
                raised: vec![],
                cleared: vec![job_failed("a", "j1", "maintenance")],
            })
        );
    }

    #[test]
    fn a_job_alarm_and_a_state_alarm_of_one_root_stand_side_by_side() {
        let start = Instant::now();
        let mut ledger = Ledger::default();

        pass(&mut ledger, &[("a", MISSING)], start);
        ledger.job_ended("forget", "a", "j1", true, start);
        ledger.flush(start + WINDOW).expect("announced");

        assert_eq!(
            ledger.active(),
            [unavailable("a"), job_failed("a", "j1", "forget")]
        );
    }

    #[test]
    fn a_mute_that_has_ended_is_announced_once_and_shown_no_more() {
        let alarms = Alarms::default();
        let at = Instant::now();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let until = now + Duration::from_secs(60);
        alarms.mute(until);

        assert_eq!(alarms.muted_until(now), Some(until));
        assert_eq!(
            alarms.flush(at, now),
            None,
            "nothing is owed while it lasts"
        );
        assert_eq!(alarms.muted_until(until), None);
        assert_eq!(
            alarms.flush(at, until),
            Some(Event::AlarmsChanged {
                alarms: vec![],
                raised: vec![],
                cleared: vec![],
                muted_until: None,
            })
        );
        assert_eq!(alarms.flush(at, until), None, "and said once");
    }

    #[test]
    fn a_mute_lasts_an_hour_unless_told_otherwise() {
        assert_eq!(mute_duration(None), Ok(Duration::from_secs(3_600)));
        assert_eq!(mute_duration(Some(1)), Ok(Duration::from_secs(60)));
        assert_eq!(mute_duration(Some(1_440)), Ok(Duration::from_secs(86_400)));
    }

    #[test]
    fn a_mute_shorter_than_a_minute_or_longer_than_a_day_is_refused() {
        for minutes in [0, -5, 1_441] {
            let refused = mute_duration(Some(minutes)).expect_err("refused");
            assert!(
                refused.contains("1 to 1440"),
                "the refusal says what is taken: {refused}"
            );
        }
    }
}
