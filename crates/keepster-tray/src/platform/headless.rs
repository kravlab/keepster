//! A tray with nothing to draw on: it follows the daemon and reports both rows unavailable, so that
//! `status` says no icon and no notification reach the person rather than leaving it to be assumed.

use std::sync::Arc;

use keepster_api::{Reported, TrayReport};
use tokio::sync::mpsc;

use crate::model::{Notice, View};
use crate::platform::{Connection, Shell};
use crate::run::{Action, Adapter};

/// Why both rows are unavailable: this build has no adapter for the platform.
const UNSUPPORTED: &str = "unsupported";

/// The shell without a toolkit.
pub struct Headless;

impl Headless {
    /// A shell whose menu never sends anything, there being none.
    pub fn new(_actions: mpsc::UnboundedSender<Action>) -> Self {
        Self
    }
}

impl Shell for Headless {
    /// With no loop of its own to run, the connection runs on this thread.
    fn run(self: Box<Self>, connection: Connection) {
        connection(Arc::new(NoAdapter));
    }
}

/// Draws nothing and shows nothing, and says so.
struct NoAdapter;

impl Adapter for NoAdapter {
    fn show(&self, view: &View) {
        tracing::debug!(status = %view.status, "nothing to draw on");
    }

    fn notify(&self, notice: &Notice) {
        tracing::debug!(title = %notice.title, "nothing to notify on");
    }

    fn report(&self) -> TrayReport {
        TrayReport {
            icon: Reported::Unavailable,
            icon_reason: Some(UNSUPPORTED.to_owned()),
            notifications: Reported::Unavailable,
            notifications_reason: Some(UNSUPPORTED.to_owned()),
        }
    }
}
