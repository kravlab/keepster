//! What the tray shows and says, decided from the status payload and the event stream alone.
//!
//! Pure: every input is handed in, the time included, and the adapters only draw the [`View`] and show
//! the [`Notice`]s it returns.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use keepster_api::{
    ALARM_JOB_FAILED, ALARM_ROOT_DEGRADED, ALARM_ROOT_UNAVAILABLE, Alarm, Event, Status,
};

use crate::strings;

/// How long a stream that ended without `daemon.stopping` waits for the daemon to answer again before
/// the tray says it died: long enough for a service manager to restart it.
pub const DEATH_GRACE: Duration = Duration::from_secs(10);

/// The level the tray's log is written at before the daemon has ever said its own.
pub const DEFAULT_LOG_LEVEL: &str = "info";

/// How much of an identity a person is shown for a Root the tray has no path for.
const SHORT_ID_LENGTH: usize = 8;

/// The tray's state, fed from the daemon.
#[derive(Debug)]
pub struct Model {
    /// Whether the daemon answers, and if not, since when.
    daemon: Daemon,
    /// Whether the open stream said `daemon.stopping`, so that its end is a stop and not a death.
    stopping: bool,
    /// Every Root, by identity, with its path: what an alarm is named by.
    roots: BTreeMap<String, String>,
    /// The alarms in force, as the daemon last said.
    alarms: Vec<Alarm>,
    /// Whether notifications are muted, as the daemon last said. The daemon announces a mute's end, so
    /// the tray reads no clock to know it; a death ends it too, since the mute lived in the daemon.
    muted: bool,
    /// Whether the notice of a death was shown, which a return then replaces.
    death_shown: bool,
    /// The daemon's log level as it last said, which the tray's log follows.
    log_level: String,
}

/// The daemon, as far as the tray can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Daemon {
    /// Never reached since the tray started: its absence is no transition.
    Unknown,
    /// Answering, with a stream open.
    Connected,
    /// Stopped on purpose: it said so.
    Stopped,
    /// Gone without a word at `since`; `said` once the grace passed and the death was decided.
    Lost {
        /// When the stream ended.
        since: Instant,
        /// Whether the grace has passed and the death was decided.
        said: bool,
    },
}

impl Default for Model {
    fn default() -> Self {
        Self {
            daemon: Daemon::Unknown,
            stopping: false,
            roots: BTreeMap::new(),
            alarms: Vec::new(),
            muted: false,
            death_shown: false,
            log_level: DEFAULT_LOG_LEVEL.to_owned(),
        }
    }
}

/// What the icon shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Every Root covered as promised.
    Covered,
    /// An alarm is in force.
    Attention,
    /// The daemon does not answer.
    Stopped,
}

/// What the icon and its menu show now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// The icon.
    pub icon: Icon,
    /// One line saying where things stand: the menu's first item and the icon's tooltip.
    pub status: String,
    /// Whether notifications are muted, which decides between the menu's mute and unmute.
    pub muted: bool,
    /// Whether the daemon answers, which decides between the menu's restart and start.
    pub daemon_running: bool,
}

/// Which notice a notification is: a later one with the same key replaces it, which is how a recovery
/// replaces the alarm it ends rather than adding to it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoticeKey {
    /// A Root's state.
    Root(String),
    /// A kind of job on a Root: `(root, kind)`.
    Job(String, String),
    /// The daemon itself.
    Daemon,
}

impl NoticeKey {
    /// The key as one string, for a platform that replaces a notification by a string identifier
    /// (macOS's request identifier, a Windows toast's tag): distinct keys never share one.
    pub fn identifier(&self) -> String {
        // A Root id and a job kind hold no `/`, so the joins cannot collide.
        match self {
            Self::Root(root) => format!("root/{root}"),
            Self::Job(root, kind) => format!("job/{root}/{kind}"),
            Self::Daemon => "daemon".to_owned(),
        }
    }
}

/// A notification to show, replacing any shown under the same key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Which notice this is, deciding what it replaces.
    pub key: NoticeKey,
    /// The notification's title.
    pub title: String,
    /// The notification's text.
    pub body: String,
}

/// What an event asks of the tray besides notices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reaction {
    /// The notifications to show.
    pub notices: Vec<Notice>,
    /// Why what the tray shows may be stale, so the status payload is fetched again; `None` when it
    /// is not.
    pub refetch: Option<Refetch>,
}

/// Why the status payload is fetched again, which decides what the fetch may notify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refetch {
    /// The Roots changed, so the names alarms are shown by may be stale. The stream missed nothing:
    /// the alarms are its to say.
    Roots,
    /// The stream missed events (`resync`), alarms among them: the fetch says what they would have.
    Missed,
}

impl Model {
    /// The daemon answered with `status`: a stream is open from here on. Alarms already in force show
    /// on the icon and notify nothing, because a tray starting is no transition; a daemon back after
    /// its death was said replaces that notice.
    pub fn connected(&mut self, status: &Status) -> Vec<Notice> {
        let back = matches!(self.daemon, Daemon::Lost { said: true, .. }) && self.death_shown;
        self.daemon = Daemon::Connected;
        self.stopping = false;
        self.death_shown = false;
        self.take(status);

        if back {
            vec![notice(NoticeKey::Daemon, strings::daemon_back())]
        } else {
            Vec::new()
        }
    }

    /// The status payload fetched again over a stream still open, for `why`: what it lists replaces
    /// what the tray held, and the daemon's own state is the stream's to say. After a stream that
    /// missed events, the notices are what it would have said of the alarms: one per alarm the tray
    /// did not hold, and a recovery for each it held that is gone. Unless muted.
    pub fn refreshed(&mut self, status: &Status, why: Refetch) -> Vec<Notice> {
        let before = std::mem::take(&mut self.alarms);
        self.take(status);
        // A stream that missed nothing says each change itself, maybe still on its way: said here
        // too, it would be said twice.
        if why == Refetch::Roots || self.muted {
            return Vec::new();
        }
        let raised: Vec<Alarm> = self
            .alarms
            .iter()
            .filter(|alarm| !before.contains(alarm))
            .cloned()
            .collect();
        let cleared: Vec<Alarm> = before
            .into_iter()
            .filter(|old| self.alarms.iter().all(|alarm| key_of(alarm) != key_of(old)))
            .collect();

        self.notices(&raised, &cleared)
    }

    /// What `status` lists, replacing what the tray held.
    fn take(&mut self, status: &Status) {
        self.roots = status
            .roots
            .iter()
            .map(|root| (root.id.clone(), root.path.clone()))
            .collect();
        self.alarms = status.daemon.alarms.clone().unwrap_or_default();
        self.muted = status.daemon.muted_until.is_some();
        if let Some(log) = &status.daemon.log {
            self.log_level.clone_from(&log.level);
        }
    }

    /// One event from the stream.
    pub fn event(&mut self, event: &Event) -> Reaction {
        match event {
            Event::AlarmsChanged {
                alarms,
                raised,
                cleared,
                muted_until,
            } => {
                self.alarms.clone_from(alarms);
                self.muted = muted_until.is_some();
                let notices = if self.muted {
                    Vec::new()
                } else {
                    self.notices(raised, cleared)
                };
                Reaction {
                    notices,
                    refetch: None,
                }
            }
            Event::DaemonStopping => {
                self.stopping = true;
                Reaction::default()
            }
            Event::LogLevelChanged { level, .. } => {
                self.log_level.clone_from(level);
                Reaction::default()
            }
            Event::Resync => Reaction {
                notices: Vec::new(),
                refetch: Some(Refetch::Missed),
            },
            Event::RootRegistered { .. }
            | Event::RootUnregistered { .. }
            | Event::RootMoved { .. } => Reaction {
                notices: Vec::new(),
                refetch: Some(Refetch::Roots),
            },
            _ => Reaction::default(),
        }
    }

    /// The stream ended at `at`: a stop when the daemon said so, otherwise possibly a death.
    pub fn stream_ended(&mut self, at: Instant) {
        self.daemon = match self.daemon {
            Daemon::Connected if self.stopping => Daemon::Stopped,
            Daemon::Connected => Daemon::Lost {
                since: at,
                said: false,
            },
            unchanged => unchanged,
        };
        self.stopping = false;
    }

    /// Time passed: what is owed at `at`, which is a dead daemon's notice once the grace has passed.
    /// A mute is no reason to keep it unsaid: the daemon held the mute in its memory, so it died with
    /// the daemon, and no daemon is left to announce its end.
    pub fn tick(&mut self, at: Instant) -> Vec<Notice> {
        let Daemon::Lost { since, said: false } = self.daemon else {
            return Vec::new();
        };
        if at.saturating_duration_since(since) < DEATH_GRACE {
            return Vec::new();
        }
        self.daemon = Daemon::Lost { since, said: true };
        self.muted = false;
        self.death_shown = true;

        vec![notice(NoticeKey::Daemon, strings::daemon_died())]
    }

    /// What to draw now.
    pub fn view(&self) -> View {
        let running = self.daemon == Daemon::Connected;
        let (icon, status) = if !running {
            (Icon::Stopped, strings::DAEMON_NOT_RUNNING.to_owned())
        } else if self.alarms.is_empty() {
            (Icon::Covered, strings::covering(self.roots.len()))
        } else {
            (Icon::Attention, strings::alarms(self.alarms.len()))
        };
        let status = if running && self.muted {
            strings::muted(&status)
        } else {
            status
        };

        View {
            icon,
            status,
            muted: self.muted,
            daemon_running: running,
        }
    }

    /// The level the tray's own log is written at: the daemon's, as last known, and `info` before the
    /// daemon has ever said.
    pub fn log_level(&self) -> &str {
        &self.log_level
    }

    /// The notices `raised` and `cleared` owe: one per alarm raised, and a recovery in the place of each
    /// one cleared that nothing raised replaces.
    fn notices(&self, raised: &[Alarm], cleared: &[Alarm]) -> Vec<Notice> {
        let mut notices: Vec<Notice> = raised.iter().map(|alarm| self.raised(alarm)).collect();
        for alarm in cleared {
            let key = key_of(alarm);
            if notices.iter().all(|notice| notice.key != key) {
                notices.push(self.recovered(alarm));
            }
        }

        notices
    }

    /// The notice of `alarm` raised, naming its Root; a kind this build does not know is shown by its
    /// code.
    fn raised(&self, alarm: &Alarm) -> Notice {
        let name = self.name(&alarm.root);
        let reason = alarm.reason.as_deref();
        let text = match (alarm.kind.as_str(), &alarm.job_kind, &alarm.job) {
            (ALARM_ROOT_UNAVAILABLE, _, _) => strings::root_unavailable(&name, reason),
            (ALARM_ROOT_DEGRADED, _, _) => strings::root_degraded(&name, reason),
            (ALARM_JOB_FAILED, Some(kind), Some(job)) => strings::job_failed(&name, kind, job),
            (kind, _, _) => strings::unknown_alarm(&name, kind),
        };

        notice(key_of(alarm), text)
    }

    /// The notice that `alarm` ended, replacing the one that said it was raised.
    fn recovered(&self, alarm: &Alarm) -> Notice {
        let name = self.name(&alarm.root);
        let text = match &alarm.job_kind {
            Some(kind) => strings::job_recovered(&name, kind),
            None => strings::root_recovered(&name),
        };

        notice(key_of(alarm), text)
    }

    /// A Root as a person recognises it: its directory's name, or, for one the tray has no path for, a
    /// short prefix of its identity.
    fn name(&self, root: &str) -> String {
        self.roots
            .get(root)
            .and_then(|path| Path::new(path).file_name())
            .map_or_else(
                || root.chars().take(SHORT_ID_LENGTH).collect(),
                |name| name.to_string_lossy().into_owned(),
            )
    }
}

/// Which notice `alarm` is shown under: its Root's for its state, its job kind's on its Root for a job.
fn key_of(alarm: &Alarm) -> NoticeKey {
    match &alarm.job_kind {
        Some(kind) => NoticeKey::Job(alarm.root.clone(), kind.clone()),
        None => NoticeKey::Root(alarm.root.clone()),
    }
}

/// A notice under `key` saying `(title, body)`, as the [`strings`] functions return them.
fn notice(key: NoticeKey, (title, body): (String, String)) -> Notice {
    Notice { key, title, body }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use keepster_api::{
        Alarm, DaemonStatus, Event, GitStatus, LogStatus, Reported, Root, RootState, Status,
        VersionInfo,
    };

    use super::{DEATH_GRACE, Icon, Model, Notice, NoticeKey, Reaction, Refetch};

    const PROJECT: &str = "01M4TW3FMA92KER3GXBG2GNT97";
    const NOTES: &str = "01M4TW3GQZ0D3B1VQ8ZK3V1Y2N";

    fn root(id: &str, path: &str) -> Root {
        Root {
            id: id.to_owned(),
            path: path.to_owned(),
            state: RootState::Watching,
            state_code: None,
            pending_paths: 0,
            oldest_pending_seconds: None,
            follow_targets: None,
            unrecognized_records: None,
        }
    }

    /// A status with `roots` and `alarms`, the log at `level`.
    fn status(roots: Vec<Root>, alarms: Vec<Alarm>, level: &str) -> Status {
        Status {
            daemon: DaemonStatus {
                version: VersionInfo {
                    build_version: "0.1.0".to_owned(),
                    build_hash: "unknown".to_owned(),
                    api_version: "v1".to_owned(),
                    store_format: None,
                },
                pid: 1,
                uptime_seconds: 1,
                autostart: Reported::Available,
                autostart_reason: None,
                linger: Reported::Available,
                linger_reason: None,
                maintenance: Reported::Available,
                git: GitStatus {
                    path: None,
                    version: None,
                    minimum: "2.32.0".to_owned(),
                    reason: None,
                    detail: None,
                    install: None,
                },
                listener: None,
                log: Some(LogStatus {
                    level: level.to_owned(),
                    default: "info".to_owned(),
                    until: None,
                    file: None,
                }),
                alarms: Some(alarms),
                muted_until: None,
                trays: Some(vec![]),
            },
            roots,
        }
    }

    fn two_roots() -> Vec<Root> {
        vec![
            root(PROJECT, "/home/u/src/project"),
            root(NOTES, "/home/u/notes"),
        ]
    }

    fn unavailable(root: &str) -> Alarm {
        Alarm {
            kind: "root_unavailable".to_owned(),
            root: root.to_owned(),
            reason: Some("root_missing".to_owned()),
            job: None,
            job_kind: None,
        }
    }

    fn degraded(root: &str) -> Alarm {
        Alarm {
            kind: "root_degraded".to_owned(),
            reason: Some("config_invalid".to_owned()),
            ..unavailable(root)
        }
    }

    fn job_failed(root: &str) -> Alarm {
        Alarm {
            kind: "job_failed".to_owned(),
            root: root.to_owned(),
            reason: None,
            job: Some("01M4TW3HJOB0000000000000000".to_owned()),
            job_kind: Some("maintenance".to_owned()),
        }
    }

    fn changed(raised: Vec<Alarm>, cleared: Vec<Alarm>, alarms: Vec<Alarm>) -> Event {
        Event::AlarmsChanged {
            alarms,
            raised,
            cleared,
            muted_until: None,
        }
    }

    fn notice(key: NoticeKey, title: &str, body: &str) -> Notice {
        Notice {
            key,
            title: title.to_owned(),
            body: body.to_owned(),
        }
    }

    fn connected(roots: Vec<Root>) -> Model {
        let mut model = Model::default();
        model.connected(&status(roots, vec![], "info"));
        model
    }

    #[test]
    fn a_daemon_with_no_alarm_is_shown_covering_its_roots() {
        let model = connected(two_roots());

        let view = model.view();
        assert_eq!(view.icon, Icon::Covered);
        assert_eq!(view.status, "Keepster is covering 2 roots");
        assert!(view.daemon_running);
        assert!(!view.muted);
        assert_eq!(
            connected(vec![root(PROJECT, "/p")]).view().status,
            "Keepster is covering 1 root"
        );
        assert_eq!(
            connected(vec![]).view().status,
            "Keepster is watching no roots"
        );
    }

    #[test]
    fn alarms_already_in_force_show_on_the_icon_without_a_notification() {
        let mut model = Model::default();

        let notices = model.connected(&status(two_roots(), vec![unavailable(PROJECT)], "info"));

        assert_eq!(notices, [], "a tray starting is no transition");
        assert_eq!(model.view().icon, Icon::Attention);
        assert_eq!(model.view().status, "Keepster: 1 alarm");
    }

    #[test]
    fn a_raised_alarm_is_notified_by_the_name_of_its_root() {
        let mut model = connected(two_roots());

        let reaction = model.event(&changed(
            vec![unavailable(PROJECT), degraded(NOTES)],
            vec![],
            vec![unavailable(PROJECT), degraded(NOTES)],
        ));

        assert_eq!(
            reaction.notices,
            [
                notice(
                    NoticeKey::Root(PROJECT.to_owned()),
                    "project is not being captured",
                    "Keepster cannot capture it (root_missing); `keepster status` says why."
                ),
                notice(
                    NoticeKey::Root(NOTES.to_owned()),
                    "notes is not covered as promised",
                    "Keepster still watches it, but not as promised (config_invalid); `keepster status` says why."
                ),
            ]
        );
        assert_eq!(reaction.refetch, None);
        assert_eq!(model.view().icon, Icon::Attention);
        assert_eq!(model.view().status, "Keepster: 2 alarms");
    }

    #[test]
    fn an_alarm_cleared_is_replaced_by_its_recovery() {
        let mut model = Model::default();
        model.connected(&status(two_roots(), vec![unavailable(PROJECT)], "info"));

        let reaction = model.event(&changed(vec![], vec![unavailable(PROJECT)], vec![]));

        assert_eq!(
            reaction.notices,
            [notice(
                NoticeKey::Root(PROJECT.to_owned()),
                "project is covered again",
                "Keepster captures it as promised."
            )]
        );
        assert_eq!(model.view().icon, Icon::Covered);
    }

    #[test]
    fn an_alarm_that_replaces_another_is_one_notice_in_its_place() {
        let mut model = Model::default();
        model.connected(&status(two_roots(), vec![degraded(PROJECT)], "info"));

        let reaction = model.event(&changed(
            vec![unavailable(PROJECT)],
            vec![degraded(PROJECT)],
            vec![unavailable(PROJECT)],
        ));

        assert_eq!(reaction.notices.len(), 1, "{:?}", reaction.notices);
        assert_eq!(reaction.notices[0].key, NoticeKey::Root(PROJECT.to_owned()));
        assert_eq!(reaction.notices[0].title, "project is not being captured");
    }

    #[test]
    fn a_failed_job_names_the_job_to_ask_about_and_its_success_replaces_it() {
        let mut model = connected(two_roots());
        let key = NoticeKey::Job(PROJECT.to_owned(), "maintenance".to_owned());

        let failed = model.event(&changed(
            vec![job_failed(PROJECT)],
            vec![],
            vec![job_failed(PROJECT)],
        ));
        let cleared = model.event(&changed(vec![], vec![job_failed(PROJECT)], vec![]));

        assert_eq!(
            failed.notices,
            [notice(
                key.clone(),
                "A maintenance of project failed",
                "`keepster job 01M4TW3HJOB0000000000000000` says what went wrong."
            )]
        );
        assert_eq!(
            cleared.notices,
            [notice(
                key,
                "A maintenance of project succeeded",
                "The failure before it no longer stands."
            )]
        );
    }

    #[test]
    fn an_alarm_of_a_kind_or_a_root_the_tray_does_not_know_is_still_notified() {
        let mut model = connected(two_roots());
        let unknown = Alarm {
            kind: "from_the_future".to_owned(),
            root: "01M4TW3ZZZZZZZZZZZZZZZZZZZ".to_owned(),
            reason: None,
            job: None,
            job_kind: None,
        };

        let reaction = model.event(&changed(vec![unknown.clone()], vec![], vec![unknown]));

        assert_eq!(
            reaction.notices,
            [notice(
                NoticeKey::Root("01M4TW3ZZZZZZZZZZZZZZZZZZZ".to_owned()),
                "01M4TW3Z: from_the_future",
                "`keepster status` says more."
            )]
        );
    }

    #[test]
    fn while_muted_alarms_change_the_icon_and_notify_nothing() {
        let mut model = connected(two_roots());

        let muted = model.event(&Event::AlarmsChanged {
            alarms: vec![],
            raised: vec![],
            cleared: vec![],
            muted_until: Some("2026-10-13T12:00:00Z".to_owned()),
        });
        let raised = model.event(&Event::AlarmsChanged {
            alarms: vec![unavailable(PROJECT)],
            raised: vec![unavailable(PROJECT)],
            cleared: vec![],
            muted_until: Some("2026-10-13T12:00:00Z".to_owned()),
        });

        assert_eq!(muted, Reaction::default());
        assert_eq!(raised.notices, []);
        let view = model.view();
        assert_eq!(view.icon, Icon::Attention);
        assert!(view.muted);
        assert_eq!(view.status, "Keepster: 1 alarm; notifications muted");
    }

    #[test]
    fn a_daemon_that_said_it_was_stopping_is_not_mourned() {
        let start = Instant::now();
        let mut model = connected(two_roots());

        model.event(&Event::DaemonStopping);
        model.stream_ended(start);

        assert_eq!(model.tick(start + DEATH_GRACE * 2), []);
        let view = model.view();
        assert_eq!(view.icon, Icon::Stopped);
        assert_eq!(view.status, "Keepster's daemon is not running");
        assert!(!view.daemon_running);
    }

    #[test]
    fn a_stream_that_ends_without_a_word_is_a_death_once_the_grace_has_passed_and_said_once() {
        let start = Instant::now();
        let mut model = connected(two_roots());

        model.stream_ended(start);

        assert_eq!(
            model.tick(start + DEATH_GRACE - Duration::from_millis(1)),
            []
        );
        assert_eq!(
            model.tick(start + DEATH_GRACE),
            [notice(
                NoticeKey::Daemon,
                "Keepster's daemon stopped unexpectedly",
                "Nothing is captured until it runs again."
            )]
        );
        assert_eq!(model.tick(start + DEATH_GRACE * 3), [], "said once");
        assert_eq!(model.view().icon, Icon::Stopped);
    }

    #[test]
    fn a_daemon_back_within_the_grace_was_restarted_and_nothing_is_said() {
        let start = Instant::now();
        let mut model = connected(two_roots());
        model.stream_ended(start);

        let back = model.connected(&status(two_roots(), vec![], "info"));

        assert_eq!(back, []);
        assert_eq!(model.tick(start + DEATH_GRACE * 2), []);
        assert_eq!(model.view().icon, Icon::Covered);
    }

    #[test]
    fn a_daemon_back_after_its_death_was_said_replaces_that_notice() {
        let start = Instant::now();
        let mut model = connected(two_roots());
        model.stream_ended(start);
        model.tick(start + DEATH_GRACE);

        let back = model.connected(&status(two_roots(), vec![], "info"));

        assert_eq!(
            back,
            [notice(
                NoticeKey::Daemon,
                "Keepster's daemon is running again",
                "Capture has resumed."
            )]
        );
    }

    #[test]
    fn a_death_ends_the_mute_the_daemon_held_so_the_death_and_the_return_are_said() {
        let start = Instant::now();
        let mut model = connected(two_roots());
        model.event(&Event::AlarmsChanged {
            alarms: vec![],
            raised: vec![],
            cleared: vec![],
            muted_until: Some("2026-10-13T12:00:00Z".to_owned()),
        });
        model.stream_ended(start);

        assert_eq!(
            model.tick(start + DEATH_GRACE),
            [notice(
                NoticeKey::Daemon,
                "Keepster's daemon stopped unexpectedly",
                "Nothing is captured until it runs again."
            )]
        );
        assert!(!model.view().muted, "the mute died with the daemon");
        assert_eq!(
            model.connected(&status(two_roots(), vec![], "info")),
            [notice(
                NoticeKey::Daemon,
                "Keepster's daemon is running again",
                "Capture has resumed."
            )]
        );
    }

    #[test]
    fn a_status_fetched_again_over_an_open_stream_says_what_changed_since_the_tray_last_knew() {
        let mut model = Model::default();
        model.connected(&status(two_roots(), vec![unavailable(NOTES)], "info"));

        let notices = model.refreshed(
            &status(two_roots(), vec![unavailable(PROJECT)], "info"),
            Refetch::Missed,
        );

        assert_eq!(
            notices,
            [
                notice(
                    NoticeKey::Root(PROJECT.to_owned()),
                    "project is not being captured",
                    "Keepster cannot capture it (root_missing); `keepster status` says why."
                ),
                notice(
                    NoticeKey::Root(NOTES.to_owned()),
                    "notes is covered again",
                    "Keepster captures it as promised."
                ),
            ]
        );
        assert_eq!(model.view().icon, Icon::Attention);
    }

    #[test]
    fn a_status_fetched_again_that_lists_what_the_tray_knew_says_nothing() {
        let mut model = Model::default();
        model.connected(&status(two_roots(), vec![unavailable(NOTES)], "info"));

        assert_eq!(
            model.refreshed(
                &status(two_roots(), vec![unavailable(NOTES)], "info"),
                Refetch::Missed
            ),
            []
        );
    }

    #[test]
    fn a_status_fetched_again_for_the_roots_leaves_the_alarms_to_the_stream() {
        let mut model = connected(two_roots());

        let notices = model.refreshed(
            &status(two_roots(), vec![unavailable(PROJECT)], "info"),
            Refetch::Roots,
        );

        assert_eq!(notices, [], "the stream says it, missing nothing");
        assert_eq!(model.view().icon, Icon::Attention);
    }

    #[test]
    fn a_status_fetched_again_while_muted_says_nothing() {
        let mut model = connected(two_roots());
        let mut muted = status(two_roots(), vec![unavailable(PROJECT)], "info");
        muted.daemon.muted_until = Some("2026-10-13T12:00:00Z".to_owned());

        assert_eq!(model.refreshed(&muted, Refetch::Missed), []);
        assert!(model.view().muted);
    }

    #[test]
    fn a_status_fetched_again_keeps_a_stop_announced_and_takes_the_roots_it_lists() {
        let start = Instant::now();
        let mut model = connected(two_roots());
        model.event(&Event::DaemonStopping);

        model.refreshed(
            &status(vec![root(PROJECT, "/p")], vec![], "info"),
            Refetch::Roots,
        );
        model.stream_ended(start);

        assert_eq!(
            model.tick(start + DEATH_GRACE * 2),
            [],
            "a stop, not a death"
        );
        model.connected(&status(vec![root(PROJECT, "/p")], vec![], "info"));
        assert_eq!(model.view().status, "Keepster is covering 1 root");
    }

    #[test]
    fn a_tray_that_never_reached_the_daemon_says_it_is_not_running_and_mourns_nothing() {
        let start = Instant::now();
        let mut model = Model::default();

        model.stream_ended(start);

        assert_eq!(model.tick(start + DEATH_GRACE * 2), []);
        assert_eq!(model.view().icon, Icon::Stopped);
        assert_eq!(model.view().status, "Keepster's daemon is not running");
    }

    #[test]
    fn the_log_level_is_the_daemons_from_status_then_from_events_and_stays_while_it_is_gone() {
        let start = Instant::now();
        let mut model = Model::default();
        assert_eq!(
            model.log_level(),
            "info",
            "before the daemon was ever reached"
        );

        model.connected(&status(two_roots(), vec![], "warn"));
        assert_eq!(model.log_level(), "warn");
        model.event(&Event::LogLevelChanged {
            level: "debug".to_owned(),
            until: Some("2026-10-13T12:00:00Z".to_owned()),
        });
        assert_eq!(model.log_level(), "debug");
        model.stream_ended(start);
        model.tick(start + DEATH_GRACE);

        assert_eq!(model.log_level(), "debug", "the last level known");
    }

    #[test]
    fn events_that_change_which_roots_exist_or_say_the_stream_missed_some_ask_for_the_status_again()
    {
        let mut model = connected(two_roots());

        for event in [
            Event::RootRegistered {
                root: PROJECT.to_owned(),
            },
            Event::RootUnregistered {
                root: PROJECT.to_owned(),
                purged: false,
            },
            Event::RootMoved {
                root: PROJECT.to_owned(),
            },
        ] {
            assert_eq!(
                model.event(&event).refetch,
                Some(Refetch::Roots),
                "{event:?}"
            );
        }
        assert_eq!(model.event(&Event::Resync).refetch, Some(Refetch::Missed));
        assert_eq!(
            model
                .event(&Event::ConfigReloaded {
                    root: PROJECT.to_owned()
                })
                .refetch,
            None
        );
    }

    #[test]
    fn each_notice_key_has_an_identifier_of_its_own() {
        let keys = [
            NoticeKey::Root("r1".to_owned()),
            NoticeKey::Root("r2".to_owned()),
            NoticeKey::Job("r1".to_owned(), "maintenance".to_owned()),
            NoticeKey::Job("r1".to_owned(), "export".to_owned()),
            NoticeKey::Job("r2".to_owned(), "maintenance".to_owned()),
            NoticeKey::Daemon,
        ];

        assert_eq!(
            keys.iter().map(NoticeKey::identifier).collect::<Vec<_>>(),
            [
                "root/r1",
                "root/r2",
                "job/r1/maintenance",
                "job/r1/export",
                "job/r2/maintenance",
                "daemon",
            ]
        );
    }
}
