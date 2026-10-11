//! `status` is the single authority on health (ticket 05), so its payload is asserted at the harness
//! seam — from a live daemon, never from a constructed value.

use keepster_testkit::Harness;

#[tokio::test]
async fn status_reports_the_daemon_and_no_roots_before_any_are_registered() {
    let daemon = Harness::start().await;

    let body = daemon.get_json("/v1/status").await;

    assert_eq!(body["daemon"]["build_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["daemon"]["api_version"], "v1");
    assert_eq!(body["daemon"]["pid"], std::process::id());

    // Uptime is elapsed time, not a moment: a daemon that just started has been up for seconds, and
    // this bound is what tells that apart from seconds since the epoch.
    let uptime = body["daemon"]["uptime_seconds"]
        .as_u64()
        .expect("uptime is a whole number of seconds");
    assert!(uptime < 60, "a freshly started daemon reported {uptime}s");

    // Rows whose owners have not landed report `unknown` rather than a comfortable default: ticket 05
    // forbids looking healthier than the truth. Ticket 61 fills autostart and linger. Maintenance is
    // ticket 52's, and depends on the machine's `git`, which the housekeeping tests look at.
    assert_eq!(body["daemon"]["autostart"], "unknown");
    assert_eq!(body["daemon"]["linger"], "unknown");
    assert!(
        ["available", "unavailable"]
            .contains(&body["daemon"]["maintenance"].as_str().unwrap_or("")),
        "{body}"
    );

    assert_eq!(body["roots"], serde_json::json!([]));
}
