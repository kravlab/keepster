//! The Linux tray, entirely over the session bus (ADR 0008): the icon is a StatusNotifierItem (`ksni`),
//! and notifications go to `org.freedesktop.Notifications`, replacing a shown one by its id. No GTK and
//! no X11, so a desktop that hosts no status icons (GNOME without the extension) still gets notifications,
//! and `status` says which of the two reach the person.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::thread::JoinHandle;

use ksni::TrayMethods;
use tokio::sync::mpsc;
use zbus::zvariant::Value;

use crate::menu;
use crate::model::{Icon, Notice, NoticeKey, View};
use crate::platform::capabilities::Capabilities;
use crate::platform::handoff::{Command, Handoff};
use crate::platform::{Connection, Shell};
use crate::run::{Action, Adapter, REPORT_EVERY};
use crate::strings;

/// The notification server's bus name, which is also its interface (Desktop Notifications
/// Specification).
const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
/// The notification server's object.
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";

/// Why a row is unavailable: the session bus cannot be reached.
const NO_SESSION_BUS: &str = "no_session_bus";
/// Why the icon row is unavailable: no StatusNotifierWatcher is registered, so no desktop hosts icons.
const NO_WATCHER: &str = "no_watcher";
/// Why the icon row is unavailable: a watcher is registered, but no host draws the icons.
const NO_HOST: &str = "no_host";
/// Why the notifications row is unavailable: no notification server answers.
const NO_SERVER: &str = "no_server";

/// The Linux shell: the bus loop, run on the main thread.
pub struct Linux {
    /// Where what is chosen from the menu goes.
    actions: mpsc::UnboundedSender<Action>,
}

impl Linux {
    /// A shell sending what is chosen from the menu to `actions`. Nothing touches the bus until
    /// [`Shell::run`].
    pub fn new(actions: mpsc::UnboundedSender<Action>) -> Self {
        Self { actions }
    }
}

impl Shell for Linux {
    fn run(self: Box<Self>, connection: Connection) {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::error!(%error, "the tray's bus loop could not be started");
                return;
            }
        };
        let connection = runtime.block_on(serve(self.actions, connection));
        let _ = connection.join();
    }
}

/// Set up the icon and the notifier, start `connection`, then draw and notify as it asks until it
/// ends. Returns its thread, to be joined.
async fn serve(actions: mpsc::UnboundedSender<Action>, connection: Connection) -> JoinHandle<()> {
    let report = Capabilities::default();
    let tray = StatusIcon {
        view: None,
        actions,
        report: report.clone(),
    };
    // Assumed available, so that an icon host that comes up after the tray (a login still settling, an
    // extension turned on later) is registered with when it does, rather than the tray giving up.
    report.set_icon(Ok(()));
    let icon = match tray.assume_sni_available(true).spawn().await {
        Ok(handle) => Some(handle),
        Err(error) => {
            tracing::warn!(%error, "the tray icon cannot be shown");
            report.set_icon(Err(icon_reason(&error)));
            None
        }
    };
    let mut notifier = match zbus::Connection::session().await {
        Ok(bus) => {
            let notifier = Notifier::new(bus);
            report.set_notifications(notifier.probe().await);
            Some(notifier)
        }
        Err(error) => {
            tracing::warn!(%error, "the session bus cannot be reached: no notifications");
            report.set_notifications(Err(NO_SESSION_BUS));
            None
        }
    };

    // Started only now, so that its first report already says what the icon and the notifier can do.
    let (commands_tx, mut commands) = mpsc::unbounded_channel();
    let adapter: Arc<dyn Adapter> = Arc::new(Handoff::new(report.clone(), move |command| {
        let _ = commands_tx.send(command);
    }));
    let connection = std::thread::spawn(move || connection(adapter));
    // A notification server may come up after the tray, or go away: asked again as often as the tray
    // reports, so that what it reports is never older than that.
    let mut probes =
        tokio::time::interval_at(tokio::time::Instant::now() + REPORT_EVERY, REPORT_EVERY);
    probes.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Ends once the connection drops the adapter, the last sender, which it does when it ends.
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Show(view)) => {
                    if let Some(icon) = &icon {
                        icon.update(|tray| tray.view = Some(view)).await;
                    }
                }
                Some(Command::Notify(notice)) => {
                    if let Some(notifier) = &mut notifier {
                        report.set_notifications(notifier.notify(&notice).await);
                    }
                }
                None => break,
            },
            _ = probes.tick() => {
                if let Some(notifier) = &notifier {
                    report.set_notifications(notifier.probe().await);
                }
            }
        }
    }
    if let Some(icon) = icon {
        icon.shutdown().await;
    }

    connection
}

/// Why the icon cannot be shown, as the report names it.
fn icon_reason(error: &ksni::Error) -> &'static str {
    match error {
        ksni::Error::Dbus(_) => NO_SESSION_BUS,
        ksni::Error::WontShow => NO_HOST,
        _ => NO_WATCHER,
    }
}

/// The StatusNotifierItem: what `ksni` asks the icon, its tooltip and its menu.
struct StatusIcon {
    /// What to show; `None` until the connection first says, while the item stays passive.
    view: Option<View>,
    /// Where what is chosen from the menu goes.
    actions: mpsc::UnboundedSender<Action>,
    /// The icon row, set as the watcher comes and goes.
    report: Capabilities,
}

impl ksni::Tray for StatusIcon {
    // A left click opens the same menu as a right click: the item does nothing else.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").to_owned()
    }

    fn title(&self) -> String {
        strings::APP_NAME.to_owned()
    }

    fn status(&self) -> ksni::Status {
        match self.view.as_ref().map(|view| view.icon) {
            None => ksni::Status::Passive,
            Some(Icon::Attention) => ksni::Status::NeedsAttention,
            Some(Icon::Covered | Icon::Stopped) => ksni::Status::Active,
        }
    }

    fn icon_name(&self) -> String {
        icon_name(self.view.as_ref().map_or(Icon::Stopped, |view| view.icon)).to_owned()
    }

    fn attention_icon_name(&self) -> String {
        icon_name(Icon::Attention).to_owned()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: strings::APP_NAME.to_owned(),
            description: self
                .view
                .as_ref()
                .map(|view| view.status.clone())
                .unwrap_or_default(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let Some(view) = &self.view else {
            return Vec::new();
        };
        menu::items(view)
            .into_iter()
            .map(|item| {
                let action = item.action;
                ksni::menu::StandardItem {
                    label: item.label,
                    enabled: action.is_some(),
                    activate: Box::new(move |tray: &mut Self| {
                        if let Some(action) = action {
                            let _ = tray.actions.send(action);
                        }
                    }),
                    ..Default::default()
                }
                .into()
            })
            .collect()
    }

    fn watcher_online(&self) {
        tracing::info!("the desktop hosts the tray icon again");
        self.report.set_icon(Ok(()));
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        tracing::info!(?reason, "the desktop hosts no tray icon");
        self.report.set_icon(Err(NO_WATCHER));
        // Kept running: the host may come back, and the icon with it.
        true
    }
}

/// The freedesktop icon theme's name for `icon`.
fn icon_name(icon: Icon) -> &'static str {
    match icon {
        Icon::Covered => "security-high",
        Icon::Attention => "dialog-warning",
        Icon::Stopped => "process-stop",
    }
}

/// Notifications through the server on the session bus, each replacing the last shown under its key.
struct Notifier {
    /// The session bus.
    bus: zbus::Connection,
    /// The server's id of the last notification shown under each key.
    shown: BTreeMap<NoticeKey, u32>,
}

impl Notifier {
    /// A notifier over `bus`, having shown nothing yet.
    fn new(bus: zbus::Connection) -> Self {
        Self {
            bus,
            shown: BTreeMap::new(),
        }
    }

    /// Whether a notification server answers, starting it if the bus can.
    async fn probe(&self) -> Result<(), &'static str> {
        self.bus
            .call_method(
                Some(NOTIFICATIONS),
                NOTIFICATIONS_PATH,
                Some(NOTIFICATIONS),
                "GetServerInformation",
                &(),
            )
            .await
            .map(|_| ())
            .map_err(|error| {
                tracing::debug!(%error, "no notification server answers");
                NO_SERVER
            })
    }

    /// Show `notice`, replacing the one shown under its key. A notification the person closed meanwhile
    /// is shown anew: the server replaces only what it still holds.
    async fn notify(&mut self, notice: &Notice) -> Result<(), &'static str> {
        let replaces = self.shown.get(&notice.key).copied().unwrap_or(0);
        let no_actions: &[&str] = &[];
        let no_hints: HashMap<&str, Value<'_>> = HashMap::new();
        // -1: the server's own default for how long it stays.
        let expire_timeout = -1_i32;
        let shown = self
            .bus
            .call_method(
                Some(NOTIFICATIONS),
                NOTIFICATIONS_PATH,
                Some(NOTIFICATIONS),
                "Notify",
                &(
                    strings::APP_NAME,
                    replaces,
                    "",
                    notice.title.as_str(),
                    notice.body.as_str(),
                    no_actions,
                    no_hints,
                    expire_timeout,
                ),
            )
            .await
            .and_then(|reply| reply.body().deserialize::<u32>());
        match shown {
            Ok(id) => {
                self.shown.insert(notice.key.clone(), id);
                Ok(())
            }
            Err(error) => {
                tracing::warn!(%error, title = %notice.title, "the notification could not be shown");
                Err(NO_SERVER)
            }
        }
    }
}
