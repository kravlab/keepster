//! `keepster mute` and `keepster unmute`: the tray's notifications, muted for a while or not at all
//! (ticket 67). The daemon holds the mute; the CLI and the tray's menu ask the same endpoint.

use std::io::{self, Write};
use std::path::Path;

use keepster_api::{MuteRequest, MuteStatus};

use crate::client::DaemonClient;
use crate::command::MuteArgs;
use crate::exit;
use crate::roots::answered;

/// Mute notifications for as long as `args` says, an hour when it says nothing, and say until when.
pub async fn mute(args: &MuteArgs, control_path: &Path) -> u8 {
    let minutes = match args.duration.as_deref().map(minutes).transpose() {
        Ok(minutes) => minutes,
        Err(refused) => {
            eprintln!("keepster: {refused}");
            return exit::USAGE;
        }
    };
    let body = serde_json::to_vec(&MuteRequest { minutes }).expect("the request serialises");
    let client = DaemonClient::new(control_path);

    match answered::<MuteStatus>(client.put_json("/v1/alarms/mute", &body).await) {
        Ok(muted) => say(&shown(&muted)),
        Err(code) => code,
    }
}

/// Lift the mute, and say so.
pub async fn unmute(control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);

    match answered::<MuteStatus>(client.delete("/v1/alarms/mute").await) {
        Ok(lifted) => say(&shown(&lifted)),
        Err(code) => code,
    }
}

/// `written`, a duration such as `2h`, as the whole minutes the daemon counts a mute in. How many it
/// takes is the daemon's to say.
fn minutes(written: &str) -> Result<i64, String> {
    match crate::selector::duration_seconds(written) {
        Some(seconds) if seconds % 60 == 0 => Ok(seconds / 60),
        Some(_) => Err(format!(
            "`--for {written}`: a mute lasts whole minutes, such as `30m` or `2h`"
        )),
        None => Err(format!(
            "`--for {written}` is not a duration: write one such as `30m` or `2h`"
        )),
    }
}

/// What a mute's state reads as.
fn shown(mute: &MuteStatus) -> String {
    match &mute.until {
        Some(until) => format!("notifications muted until {until}"),
        None => "notifications are not muted".to_owned(),
    }
}

/// Print `line`, which a closed pipe does not make a failure.
fn say(line: &str) -> u8 {
    let mut out = io::stdout().lock();
    match writeln!(out, "{line}").and_then(|()| out.flush()) {
        Ok(()) => exit::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => exit::SUCCESS,
        Err(error) => {
            eprintln!("keepster: could not write the answer: {error}");
            exit::OTHER
        }
    }
}

#[cfg(test)]
mod tests {
    use super::minutes;

    #[test]
    fn a_duration_in_whole_minutes_is_counted_in_minutes() {
        assert_eq!(minutes("30m"), Ok(30));
        assert_eq!(minutes("2h"), Ok(120));
        assert_eq!(minutes("120s"), Ok(2));
        assert_eq!(
            minutes("2d"),
            Ok(2_880),
            "how long is too long is the daemon's to say"
        );
    }

    #[test]
    fn a_duration_that_is_not_whole_minutes_or_not_a_duration_is_refused() {
        for written in ["90s", "soon", "", "-5m"] {
            assert!(minutes(written).is_err(), "{written}");
        }
    }
}
