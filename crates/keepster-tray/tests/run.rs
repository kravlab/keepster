//! The tray's connection at its seam (ticket 67): a real daemon from the testkit on one side, and on the
//! other a stand-in [`Adapter`] that records what the tray would draw and notify. What no test here sees
//! is a platform drawing it; following the daemon's log level is not covered either, since the tray's log
//! installs the process-wide subscriber.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use keepster_api::{Reported, TrayReport};
use keepster_testkit::Harness;
use keepster_tray::model::{Notice, View};
use keepster_tray::run::{self, Action, Adapter};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// How long a test waits for the tray to catch up before it gives up.
const PATIENCE: Duration = Duration::from_secs(30);

/// What the tray asked to draw and to notify, in order.
#[derive(Default)]
struct Recorder {
    views: Mutex<Vec<View>>,
    notices: Mutex<Vec<Notice>>,
}

impl Recorder {
    /// What the tray draws now.
    fn view(&self) -> Option<View> {
        self.views.lock().expect("the views").last().cloned()
    }

    /// Every notification it showed.
    fn notices(&self) -> Vec<Notice> {
        self.notices.lock().expect("the notices").clone()
    }
}

impl Adapter for Recorder {
    fn show(&self, view: &View) {
        self.views.lock().expect("the views").push(view.clone());
    }

    fn notify(&self, notice: &Notice) {
        self.notices
            .lock()
            .expect("the notices")
            .push(notice.clone());
    }

    fn report(&self) -> TrayReport {
        TrayReport {
            icon: Reported::Available,
            icon_reason: None,
            notifications: Reported::Available,
            notifications_reason: None,
        }
    }
}

/// A tray running against the daemon on `control_path`: what it shows, and its menu.
struct Tray {
    recorder: Arc<Recorder>,
    menu: mpsc::UnboundedSender<Action>,
    running: JoinHandle<()>,
}

impl Tray {
    /// Start a tray on `control_path`.
    fn start(control_path: PathBuf) -> Self {
        let recorder = Arc::new(Recorder::default());
        let (menu, chosen) = mpsc::unbounded_channel();
        let adapter: Arc<dyn Adapter> = recorder.clone();
        let running = tokio::spawn(run::run(control_path, adapter, chosen, None));

        Self {
            recorder,
            menu,
            running,
        }
    }

    /// Wait until what the tray draws satisfies `wanted`, and return it.
    async fn shows(&self, what: &str, wanted: impl Fn(&View) -> bool) -> View {
        eventually(what, || async {
            self.recorder.view().filter(|view| wanted(view))
        })
        .await
    }

    /// Choose `action` from the menu.
    fn choose(&self, action: Action) {
        self.menu.send(action).expect("the tray is running");
    }
}

/// Probe until `probe` finds something, for at most [`PATIENCE`].
async fn eventually<T, F, Fut>(what: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    tokio::time::timeout(PATIENCE, async {
        loop {
            if let Some(found) = probe().await {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("gave up waiting for {what}"))
}

/// The trays `status` lists.
async fn trays(daemon: &Harness) -> Value {
    daemon.get_json("/v1/status").await["daemon"]["trays"].clone()
}

#[tokio::test]
async fn a_tray_that_reaches_the_daemon_shows_it_running_and_reports_what_it_can_do() {
    let daemon = Harness::start().await;
    let tray = Tray::start(daemon.control_path().to_owned());

    tray.shows("the daemon running", |view| view.daemon_running)
        .await;

    let reported = eventually("the tray's report", || async {
        let trays = trays(&daemon).await;
        (trays != json!([])).then_some(trays)
    })
    .await;
    assert_eq!(
        reported,
        json!([{
            "instance": format!("tray-{}", std::process::id()),
            "icon": "available",
            "notifications": "available",
        }])
    );
}

#[tokio::test]
async fn quitting_withdraws_the_report_and_ends_the_tray() {
    let daemon = Harness::start().await;
    let tray = Tray::start(daemon.control_path().to_owned());
    eventually("the tray's report", || async {
        (trays(&daemon).await != json!([])).then_some(())
    })
    .await;

    tray.choose(Action::Quit);

    tokio::time::timeout(PATIENCE, tray.running)
        .await
        .expect("the tray ends")
        .expect("it ends without panicking");
    assert_eq!(trays(&daemon).await, json!([]));
}

#[tokio::test]
async fn muting_from_the_menu_mutes_the_daemon_and_unmuting_lifts_it() {
    let daemon = Harness::start().await;
    let tray = Tray::start(daemon.control_path().to_owned());
    tray.shows("the daemon running", |view| view.daemon_running)
        .await;

    tray.choose(Action::Mute);

    tray.shows("the mute", |view| view.muted).await;
    let muted = daemon.get_json("/v1/status").await["daemon"]["muted_until"].clone();
    assert!(muted.is_string(), "{muted}");

    tray.choose(Action::Unmute);

    tray.shows("the mute lifted", |view| !view.muted).await;
    assert_eq!(
        daemon.get_json("/v1/status").await["daemon"].get("muted_until"),
        None
    );
}

#[tokio::test]
async fn a_root_registered_while_the_tray_runs_is_counted_in_what_it_shows() {
    let daemon = Harness::start().await;
    let tray = Tray::start(daemon.control_path().to_owned());
    tray.shows("the daemon running", |view| view.daemon_running)
        .await;

    daemon
        .post_json("/v1/roots", &json!({ "path": daemon.scratch("project") }))
        .await;

    tray.shows("the new Root", |view| {
        view.status == "Keepster is covering 1 root"
    })
    .await;
}

#[tokio::test]
async fn a_root_that_stops_being_watched_is_notified_by_its_name() {
    let daemon = Harness::start().await;
    let project = daemon.scratch("project");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    eventually("the Root watched", || async {
        (daemon.get_json(&format!("/v1/roots/{root}")).await["state"] == "watching").then_some(())
    })
    .await;
    let tray = Tray::start(daemon.control_path().to_owned());
    tray.shows("the daemon running", |view| view.daemon_running)
        .await;

    std::fs::remove_dir_all(&project).expect("the directory goes away");

    let notice = eventually("the alarm's notice", || async {
        tray.recorder.notices().into_iter().next()
    })
    .await;
    assert_eq!(notice.title, "project is not being captured");
}

#[tokio::test]
async fn a_daemon_that_stops_is_shown_stopped_and_found_again_when_it_returns() {
    let daemon = Harness::start().await;
    let tray = Tray::start(daemon.control_path().to_owned());
    tray.shows("the daemon running", |view| view.daemon_running)
        .await;

    let stopped = daemon.stop().await;

    tray.shows("the daemon stopped", |view| !view.daemon_running)
        .await;
    assert_eq!(tray.recorder.notices(), [], "a stop is said by nobody");

    let _daemon = stopped.start().await;

    tray.shows("the daemon running again", |view| view.daemon_running)
        .await;
}
