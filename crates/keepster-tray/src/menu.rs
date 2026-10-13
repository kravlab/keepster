//! The tray's menu as values, the same on every platform: each adapter only turns these items into its
//! toolkit's, so what the menu offers is decided once, from the [`View`].

use crate::model::View;
use crate::run::Action;
use crate::strings;

/// One item of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// What the item says.
    pub label: String,
    /// What choosing it does; `None` for the status line, which is shown but cannot be chosen.
    pub action: Option<Action>,
}

/// The menu for `view`, top to bottom: the status line, then what can be done from where things stand.
pub fn items(view: &View) -> Vec<Item> {
    let item = |label: &str, action| Item {
        label: label.to_owned(),
        action: Some(action),
    };
    vec![
        Item {
            label: view.status.clone(),
            action: None,
        },
        item(strings::OPEN_HISTORY, Action::OpenHistory),
        if view.muted {
            item(strings::UNMUTE, Action::Unmute)
        } else {
            item(strings::MUTE_FOR_AN_HOUR, Action::Mute)
        },
        if view.daemon_running {
            item(strings::RESTART_DAEMON, Action::RestartDaemon)
        } else {
            item(strings::START_DAEMON, Action::StartDaemon)
        },
        item(strings::QUIT_TRAY, Action::Quit),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Icon;

    fn view(muted: bool, daemon_running: bool) -> View {
        View {
            icon: Icon::Covered,
            status: "where things stand".to_owned(),
            muted,
            daemon_running,
        }
    }

    fn shown(items: &[Item]) -> Vec<(&str, Option<Action>)> {
        items
            .iter()
            .map(|item| (item.label.as_str(), item.action))
            .collect()
    }

    #[test]
    fn a_running_daemon_offers_history_the_mute_a_restart_and_quit_under_the_status_line() {
        assert_eq!(
            shown(&items(&view(false, true))),
            [
                ("where things stand", None),
                (strings::OPEN_HISTORY, Some(Action::OpenHistory)),
                (strings::MUTE_FOR_AN_HOUR, Some(Action::Mute)),
                (strings::RESTART_DAEMON, Some(Action::RestartDaemon)),
                (strings::QUIT_TRAY, Some(Action::Quit)),
            ]
        );
    }

    #[test]
    fn a_mute_in_force_is_offered_to_be_lifted() {
        assert_eq!(
            shown(&items(&view(true, true)))[2],
            (strings::UNMUTE, Some(Action::Unmute))
        );
    }

    #[test]
    fn a_daemon_that_does_not_answer_is_offered_a_start_in_place_of_the_restart() {
        assert_eq!(
            shown(&items(&view(false, false)))[3],
            (strings::START_DAEMON, Some(Action::StartDaemon))
        );
    }
}
