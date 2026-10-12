//! The `autostart` and `linger` rows of `status` (ticket 61): whether the daemon comes back at login,
//! and whether coverage survives logout — read from the platform, never assumed.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use keepster_api::Reported;
use keepster_service::autostart::{Autostart, Linger};

/// How long a reading stands. On Windows reading means running `schtasks`, and `status` is what a
/// shell prompt or a tray asks often; a unit installed a moment ago shows within this.
const FRESH_FOR: Duration = Duration::from_secs(5);

/// The two rows, with why each is unavailable.
#[derive(Debug, Clone)]
pub(crate) struct Rows {
    pub(crate) autostart: Reported,
    pub(crate) autostart_reason: Option<String>,
    pub(crate) linger: Reported,
    pub(crate) linger_reason: Option<String>,
}

/// The last reading, and when it was taken.
#[derive(Debug, Default)]
pub(crate) struct Readings {
    last: Mutex<Option<(Instant, Rows)>>,
}

impl Readings {
    /// The rows as they stand, read again once the last reading is older than [`FRESH_FOR`].
    pub(crate) async fn rows(&self) -> Rows {
        if let Some((taken, rows)) = self.last().as_ref() {
            if taken.elapsed() < FRESH_FOR {
                return rows.clone();
            }
        }
        let rows = tokio::task::spawn_blocking(read)
            .await
            .unwrap_or_else(|_| unknown());
        *self.last() = Some((Instant::now(), rows.clone()));

        rows
    }

    fn last(&self) -> std::sync::MutexGuard<'_, Option<(Instant, Rows)>> {
        self.last
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn read() -> Rows {
    let (autostart, autostart_reason) = match keepster_service::autostart::autostart() {
        Autostart::Installed => (Reported::Available, None),
        Autostart::Disabled => (Reported::Unavailable, Some("disabled")),
        Autostart::NotInstalled => (Reported::Unavailable, Some("not_installed")),
        Autostart::Unknown(_) => (Reported::Unknown, None),
    };
    let (linger, linger_reason) = match keepster_service::autostart::linger() {
        Linger::On => (Reported::Available, None),
        Linger::Off => (Reported::Unavailable, Some("disabled")),
        Linger::LogoutGap => (Reported::Unavailable, Some("logout_gap")),
        Linger::Unknown => (Reported::Unknown, None),
    };

    Rows {
        autostart,
        autostart_reason: autostart_reason.map(str::to_owned),
        linger,
        linger_reason: linger_reason.map(str::to_owned),
    }
}

fn unknown() -> Rows {
    Rows {
        autostart: Reported::Unknown,
        autostart_reason: None,
        linger: Reported::Unknown,
        linger_reason: None,
    }
}
