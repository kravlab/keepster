//! `keepster mute` and `keepster unmute` as the shipped binary runs them (ticket 67): a mute always
//! ends, `status` shows it while it lasts, and it changes no exit code.

use keepster_testkit::Harness;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test]
async fn a_mute_says_when_it_ends_and_status_shows_it_until_it_is_lifted() {
    let daemon = Harness::start().await;

    let muted = daemon.run_cli(KEEPSTER, &["mute", "--for", "2h"]).await;

    assert_eq!(muted.exit_code, Some(0), "{}", muted.stderr);
    let until = daemon.get_json("/v1/status").await["daemon"]["muted_until"]
        .as_str()
        .expect("muted")
        .to_owned();
    assert_eq!(
        text(&muted.stdout),
        format!("notifications muted until {until}\n")
    );
    let shown = daemon.run_cli(KEEPSTER, &["status"]).await;
    assert_eq!(shown.exit_code, Some(0), "a mute changes no exit code");
    assert!(
        text(&shown.stdout).contains(&format!("notifications muted until {until}")),
        "{}",
        text(&shown.stdout)
    );

    let lifted = daemon.run_cli(KEEPSTER, &["unmute"]).await;

    assert_eq!(lifted.exit_code, Some(0), "{}", lifted.stderr);
    assert_eq!(text(&lifted.stdout), "notifications are not muted\n");
    assert!(
        daemon.get_json("/v1/status").await["daemon"]
            .get("muted_until")
            .is_none()
    );
}

#[tokio::test]
async fn a_mute_without_a_duration_lasts_an_hour() {
    let daemon = Harness::start().await;

    let muted = daemon.run_cli(KEEPSTER, &["mute"]).await;

    assert_eq!(muted.exit_code, Some(0), "{}", muted.stderr);
    let until = daemon.get_json("/v1/status").await["daemon"]["muted_until"]
        .as_str()
        .expect("muted")
        .parse::<jiff::Timestamp>()
        .expect("a moment");
    let lasts = until.duration_since(jiff::Timestamp::now());
    assert!(
        (jiff::SignedDuration::from_mins(59)..=jiff::SignedDuration::from_mins(60))
            .contains(&lasts),
        "{lasts:?}"
    );
}

#[tokio::test]
async fn a_mute_that_is_not_whole_minutes_or_longer_than_a_day_is_a_usage_error() {
    let daemon = Harness::start().await;

    for duration in ["90s", "2d", "soon"] {
        let refused = daemon.run_cli(KEEPSTER, &["mute", "--for", duration]).await;

        assert_eq!(refused.exit_code, Some(2), "{duration}: {}", refused.stderr);
        assert!(refused.stdout.is_empty(), "{duration}");
    }
    assert!(
        daemon.get_json("/v1/status").await["daemon"]
            .get("muted_until")
            .is_none(),
        "nothing was muted"
    );
}
