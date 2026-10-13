//! What a platform's tray can do, as it reports it: shared between the toolkit's loop, which learns it,
//! and the connection, which reports it.

use std::sync::{Arc, Mutex, PoisonError};

use keepster_api::{Reported, TrayReport};

/// What the tray reports it can do: written by the platform's loop as it learns, read by the connection
/// each time it reports.
#[derive(Clone)]
pub(crate) struct Capabilities(Arc<Mutex<TrayReport>>);

impl Default for Capabilities {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(TrayReport {
            icon: Reported::Unavailable,
            icon_reason: None,
            notifications: Reported::Unavailable,
            notifications_reason: None,
        })))
    }
}

impl Capabilities {
    /// Whether the icon is shown, or why not.
    pub(crate) fn set_icon(&self, shown: Result<(), &str>) {
        let mut report = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        (report.icon, report.icon_reason) = row(shown);
    }

    /// Whether notifications reach the person, or why not.
    pub(crate) fn set_notifications(&self, shown: Result<(), &str>) {
        let mut report = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        (report.notifications, report.notifications_reason) = row(shown);
    }

    /// The report as it stands.
    pub(crate) fn current(&self) -> TrayReport {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// One row of the report.
fn row(shown: Result<(), &str>) -> (Reported, Option<String>) {
    match shown {
        Ok(()) => (Reported::Available, None),
        Err(reason) => (Reported::Unavailable, Some(reason.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_row_says_available_or_why_not_independently_of_the_other() {
        let report = Capabilities::default();
        let reader = report.clone();

        report.set_icon(Err("no_watcher"));
        report.set_notifications(Ok(()));

        assert_eq!(
            reader.current(),
            TrayReport {
                icon: Reported::Unavailable,
                icon_reason: Some("no_watcher".to_owned()),
                notifications: Reported::Available,
                notifications_reason: None,
            }
        );
    }
}
