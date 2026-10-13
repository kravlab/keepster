//! The connection's side of every shell: the connection runs on a thread of its own, and each toolkit
//! draws and notifies only from its own loop, so every call is handed over to that loop as a [`Command`].

use keepster_api::TrayReport;

use crate::model::{Notice, View};
use crate::platform::capabilities::Capabilities;
use crate::run::Adapter;

/// What the connection asks of the toolkit's loop.
pub(crate) enum Command {
    /// Draw the icon and its menu as this says.
    Show(View),
    /// Show this notification.
    Notify(Notice),
}

/// An [`Adapter`] that hands each call to the toolkit's loop through `deliver`, and reports what the
/// toolkit last found it can do.
///
/// `deliver` queues the command and wakes the loop for it. The loop learns the connection has ended
/// when its queue's last sender is gone, which is when this adapter is dropped with the closure that
/// holds it.
pub(crate) struct Handoff {
    /// Queues a command and wakes the toolkit's loop for it.
    deliver: Box<dyn Fn(Command) + Send + Sync>,
    /// What the toolkit last found it can do.
    report: Capabilities,
}

impl Handoff {
    /// An adapter handing every call to `deliver`, and reporting `report`.
    pub(crate) fn new(
        report: Capabilities,
        deliver: impl Fn(Command) + Send + Sync + 'static,
    ) -> Self {
        Self {
            deliver: Box::new(deliver),
            report,
        }
    }
}

impl Adapter for Handoff {
    fn show(&self, view: &View) {
        (self.deliver)(Command::Show(view.clone()));
    }

    fn notify(&self, notice: &Notice) {
        (self.deliver)(Command::Notify(notice.clone()));
    }

    fn report(&self) -> TrayReport {
        self.report.current()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use keepster_api::Reported;

    use super::{Command, Handoff};
    use crate::model::{Icon, Notice, NoticeKey, View};
    use crate::platform::capabilities::Capabilities;
    use crate::run::Adapter;

    #[test]
    fn every_call_is_handed_over_in_order_and_the_report_is_what_the_toolkit_found() {
        let handed: Arc<Mutex<Vec<String>>> = Arc::default();
        let report = Capabilities::default();
        let seen = Arc::clone(&handed);
        let adapter = Handoff::new(report.clone(), move |command| {
            seen.lock().expect("the log").push(match command {
                Command::Show(view) => format!("show {}", view.status),
                Command::Notify(notice) => format!("notify {}", notice.title),
            });
        });
        report.set_icon(Ok(()));
        report.set_notifications(Err("no_server"));

        adapter.show(&View {
            icon: Icon::Covered,
            status: "covered".to_owned(),
            muted: false,
            daemon_running: true,
        });
        adapter.notify(&Notice {
            key: NoticeKey::Daemon,
            title: "died".to_owned(),
            body: String::new(),
        });

        assert_eq!(
            *handed.lock().expect("the log"),
            ["show covered", "notify died"]
        );
        let reported = adapter.report();
        assert_eq!(reported.icon, Reported::Available);
        assert_eq!(reported.notifications, Reported::Unavailable);
        assert_eq!(reported.notifications_reason.as_deref(), Some("no_server"));
    }
}
