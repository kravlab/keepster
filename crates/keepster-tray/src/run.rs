//! The tray's connection to the daemon: the event stream and the status payload into the [`Model`], what
//! it decides out to the platform's [`Adapter`], and the menu's actions to the API or the service
//! manager — the same calls `keepster` makes, so the tray offers nothing the API does not.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use keepster_api::{Event, MuteRequest, Status, TrayReport, UiBootstrap};
use keepster_cli::DaemonClient;
use keepster_service::autostart;
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval};

use crate::log::TrayLog;
use crate::model::{Model, Notice, View};

/// How often the tray reports what it can do: three reports fit in the daemon's 90 seconds.
pub const REPORT_EVERY: Duration = Duration::from_secs(30);

/// How long between two attempts to reach a daemon that does not answer.
const RETRY_EVERY: Duration = Duration::from_secs(1);

/// How often the model is asked whether a death is owed.
const TICK_EVERY: Duration = Duration::from_secs(1);

/// What a platform draws and shows. Called from the connection's thread: an adapter whose toolkit
/// lives on another thread hands each call over itself.
pub trait Adapter: Send + Sync {
    /// Draw the icon and its menu as `view` says.
    fn show(&self, view: &View);
    /// Show `notice`, replacing one shown under the same key.
    fn notify(&self, notice: &Notice);
    /// What this tray can do here: draw its icon, show notifications.
    fn report(&self) -> TrayReport;
}

/// What a person chose from the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Open the web UI signed in, as `keepster ui` does.
    OpenHistory,
    /// Mute notifications for an hour, as `keepster mute` does.
    Mute,
    /// Lift the mute, as `keepster unmute` does.
    Unmute,
    /// Ask the service manager to restart the daemon, as `keepster service restart` does.
    RestartDaemon,
    /// Ask the service manager to start the daemon, as `keepster service start` does.
    StartDaemon,
    /// Withdraw the tray's report and end.
    Quit,
}

/// One tray's connection: the model it feeds and what it reports under.
struct Connection<'a> {
    /// The daemon's API, over its control channel.
    client: DaemonClient,
    /// What draws and notifies.
    adapter: Arc<dyn Adapter>,
    /// The tray's log, whose level follows the daemon's; `None` when it could not be opened.
    log: Option<&'a TrayLog>,
    /// What the tray knows and decides.
    model: Model,
    /// The level the log was last set to, so it is set again only when the daemon's changes.
    applied: String,
    /// The name the tray reports under, the same for as long as it runs.
    instance: String,
}

/// Serve the tray until the menu's Quit: reach the daemon, follow its stream, and reach it again
/// whenever the stream ends, for as long as the tray runs.
pub async fn run(
    control_path: PathBuf,
    adapter: Arc<dyn Adapter>,
    mut actions: mpsc::UnboundedReceiver<Action>,
    log: Option<&TrayLog>,
) {
    let mut connection = Connection {
        client: DaemonClient::new(control_path),
        adapter,
        log,
        model: Model::default(),
        applied: String::new(),
        instance: format!("tray-{}", std::process::id()),
    };
    let mut ticks = interval(TICK_EVERY);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut reports = interval(REPORT_EVERY);
    reports.set_missed_tick_behavior(MissedTickBehavior::Delay);
    connection.refresh();

    loop {
        if let Some(mut stream) = connection.connect().await {
            loop {
                tokio::select! {
                    next = stream.next() => match next {
                        Ok(Some(event)) => connection.event(&event.event, &event.data).await,
                        Ok(None) => break,
                        Err(error) => {
                            tracing::info!(%error, "the event stream failed");
                            break;
                        }
                    },
                    Some(action) = actions.recv() => {
                        if connection.act(action).await {
                            return;
                        }
                    }
                    _ = ticks.tick() => connection.tick(),
                    _ = reports.tick() => connection.report().await,
                }
            }
            tracing::info!("the event stream ended");
            connection.model.stream_ended(Instant::now());
            connection.refresh();
        }

        let retry = tokio::time::sleep(RETRY_EVERY);
        tokio::pin!(retry);
        loop {
            tokio::select! {
                () = &mut retry => break,
                Some(action) = actions.recv() => {
                    if connection.act(action).await {
                        return;
                    }
                }
                _ = ticks.tick() => connection.tick(),
            }
        }
    }
}

impl Connection<'_> {
    /// Open the stream, then read the status: in that order, so nothing that happens between the two
    /// is missed. `None` while the daemon does not answer.
    async fn connect(&mut self) -> Option<keepster_cli::EventStream> {
        let stream = self.client.events(None).await.ok()?;
        let status = self.status().await?;
        tracing::info!("connected to the daemon");
        let notices = self.model.connected(&status);
        self.deliver(&notices);
        self.report().await;

        Some(stream)
    }

    /// The status payload, or `None` when it could not be read.
    async fn status(&self) -> Option<Status> {
        let response = self.client.get("/v1/status").await.ok()?;
        if !response.status.is_success() {
            tracing::warn!(status = %response.status, "the daemon refused its status");
            return None;
        }
        serde_json::from_slice(&response.body)
            .map_err(|error| tracing::warn!(%error, "the daemon's status could not be read"))
            .ok()
    }

    /// One event, named `name` with `data` as its payload. An event this build does not know is passed
    /// over, as the stream's contract asks.
    async fn event(&mut self, name: &str, data: &str) {
        let Ok(event) = serde_json::from_str::<Event>(data) else {
            tracing::debug!(name, "an event this tray does not know was passed over");
            return;
        };
        let mut reaction = self.model.event(&event);
        if let Some(why) = reaction.refetch {
            if let Some(status) = self.status().await {
                reaction.notices.extend(self.model.refreshed(&status, why));
            }
        }
        self.deliver(&reaction.notices);
    }

    /// What time passing owes: a dead daemon's notice once its grace has passed.
    fn tick(&mut self) {
        let notices = self.model.tick(Instant::now());
        if !notices.is_empty() {
            self.deliver(&notices);
        }
    }

    /// Show `notices`, then draw what the model says now, and follow the daemon's log level.
    fn deliver(&mut self, notices: &[Notice]) {
        for notice in notices {
            tracing::info!(title = %notice.title, "notified");
            self.adapter.notify(notice);
        }
        self.refresh();
    }

    /// Draw what the model says now, and follow the daemon's log level.
    fn refresh(&mut self) {
        self.adapter.show(&self.model.view());
        let level = self.model.log_level();
        if level != self.applied {
            if let Some(log) = self.log {
                log.set_level(level);
            }
            self.applied = level.to_owned();
        }
    }

    /// Tell the daemon what this tray can do. A daemon that does not answer has nobody to tell.
    async fn report(&self) {
        let body = serde_json::to_vec(&self.adapter.report()).expect("the report serialises");
        if let Err(error) = self.client.put_json(&self.report_path(), &body).await {
            tracing::debug!(%error, "the tray's report was not delivered");
        }
    }

    /// Where this tray's report lives.
    fn report_path(&self) -> String {
        format!("/v1/trays/{}", self.instance)
    }

    /// Do what the menu asked; whether the tray is to end.
    async fn act(&mut self, action: Action) -> bool {
        tracing::info!(?action, "chosen from the menu");
        let failed = match action {
            Action::OpenHistory => self.open_history().await.err(),
            Action::Mute => {
                // No length: the daemon's own, an hour, which the menu's label says.
                let body = serde_json::to_vec(&MuteRequest { minutes: None })
                    .expect("the request serialises");
                answered(self.client.put_json("/v1/alarms/mute", &body).await).err()
            }
            Action::Unmute => answered(self.client.delete("/v1/alarms/mute").await).err(),
            Action::RestartDaemon => service(autostart::restart).await.err(),
            Action::StartDaemon => service(autostart::start).await.err(),
            Action::Quit => {
                let _ = self.client.delete(&self.report_path()).await;
                tracing::info!("the tray is quitting");
                return true;
            }
        };
        // Said in the log, never as a notification: a notice originates in the daemon (ADR 0008).
        if let Some(error) = failed {
            tracing::warn!(?action, %error, "what the menu asked failed");
        }

        false
    }

    /// Open the web UI signed in: the same bootstrap and the same opener as `keepster ui`.
    async fn open_history(&self) -> Result<(), String> {
        let response = answered(self.client.post("/v1/ui/bootstrap", b"{}").await)?;
        let bootstrap: UiBootstrap =
            serde_json::from_slice(&response.body).map_err(|error| error.to_string())?;
        tokio::task::spawn_blocking(move || keepster_cli::ui::open(&bootstrap.url))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())
    }
}

/// A response that succeeded, or why it did not.
fn answered(
    response: Result<keepster_cli::Response, keepster_cli::ClientError>,
) -> Result<keepster_cli::Response, String> {
    match response {
        Ok(response) if response.status.is_success() => Ok(response),
        Ok(response) => Err(format!(
            "the daemon answered {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        )),
        Err(error) => Err(error.to_string()),
    }
}

/// Ask the service manager for `step`, off the async runtime: each step starts a process and waits.
async fn service(step: fn() -> Result<(), autostart::ServiceError>) -> Result<(), String> {
    tokio::task::spawn_blocking(step)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}
