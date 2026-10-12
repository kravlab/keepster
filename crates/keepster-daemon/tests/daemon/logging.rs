//! The log level as an operation that expires (ticket 62), at the `/v1` seam: raised for a while, shown
//! in `status` for as long as it lasts, and back to the configured level by itself.

use std::time::{Duration, Instant};

use keepster_testkit::Harness;
use serde_json::{Value, json};

async fn log_row(daemon: &Harness) -> Value {
    daemon.get_json("/v1/status").await["daemon"]["log"].clone()
}

fn code_of(response: &keepster_cli::Response) -> Value {
    serde_json::from_slice::<Value>(&response.body).expect("a problem")["code"].clone()
}

#[tokio::test]
async fn the_level_is_the_configured_one_and_no_raise_is_shown() {
    let daemon = Harness::start_with_global_config(b"[log]\nlevel = \"warn\"\n").await;

    let row = log_row(&daemon).await;

    assert_eq!(row["level"], "warn", "{row}");
    assert_eq!(row["default"], "warn", "{row}");
    assert!(row.get("until").is_none(), "{row}");
}

#[tokio::test]
async fn a_raised_level_is_shown_with_when_it_ends() {
    let daemon = Harness::start().await;

    let raised = daemon
        .put_json("/v1/log/level", &json!({ "level": "debug", "for": "30m" }))
        .await;

    assert_eq!(
        raised.status,
        200,
        "{}",
        String::from_utf8_lossy(&raised.body)
    );
    let row = log_row(&daemon).await;
    assert_eq!(row["level"], "debug", "{row}");
    assert_eq!(row["default"], "info", "{row}");
    let until = row["until"].as_str().expect("when it ends");
    assert!(
        until.parse::<jiff::Timestamp>().is_ok(),
        "an RFC 3339 moment: {until}"
    );
}

#[tokio::test]
async fn a_raise_needs_a_duration_and_a_level_this_build_knows() {
    let daemon = Harness::start().await;

    for body in [
        json!({ "level": "debug" }),
        json!({ "level": "debug", "for": "0s" }),
        json!({ "level": "debug", "for": "8d" }),
        json!({ "level": "chatty", "for": "30m" }),
    ] {
        let refused = daemon.put_json("/v1/log/level", &body).await;
        assert_eq!(refused.status, 400, "{body}");
        assert_eq!(code_of(&refused), "log_level_invalid", "{body}");
    }
    assert!(log_row(&daemon).await.get("until").is_none());
}

#[tokio::test]
async fn ending_a_raise_returns_to_the_configured_level_at_once() {
    let daemon = Harness::start().await;
    daemon
        .put_json("/v1/log/level", &json!({ "level": "trace", "for": "1h" }))
        .await;

    let ended = daemon.delete("/v1/log/level").await;

    assert_eq!(ended.status, 200);
    let row = log_row(&daemon).await;
    assert_eq!(row["level"], "info", "{row}");
    assert!(row.get("until").is_none(), "{row}");
}

#[tokio::test]
async fn a_raise_expires_by_itself() {
    let daemon = Harness::start().await;
    daemon
        .put_json("/v1/log/level", &json!({ "level": "debug", "for": "1s" }))
        .await;

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let row = log_row(&daemon).await;
        if row["level"] == "info" && row.get("until").is_none() {
            break;
        }
        assert!(Instant::now() < deadline, "the raise never ended: {row}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn a_later_raise_replaces_an_earlier_one_and_its_expiry() {
    // The first raise's end must not end the second: the level comes back only at the second's, which
    // is at least three seconds after it was asked for — however late a loaded runner gets there.
    let daemon = Harness::start().await;
    daemon
        .put_json("/v1/log/level", &json!({ "level": "debug", "for": "1s" }))
        .await;
    let replaced = Instant::now();
    daemon
        .put_json("/v1/log/level", &json!({ "level": "trace", "for": "3s" }))
        .await;

    let deadline = replaced + Duration::from_secs(30);
    loop {
        let row = log_row(&daemon).await;
        if row["level"] == "info" {
            break;
        }
        assert_eq!(row["level"], "trace", "{row}");
        assert!(Instant::now() < deadline, "the raise never ended: {row}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(
        replaced.elapsed() >= Duration::from_secs(3),
        "ended after {:?}, by the first raise's timer",
        replaced.elapsed()
    );
}

#[tokio::test]
async fn a_level_no_more_verbose_than_the_configured_one_is_not_a_raise() {
    let daemon = Harness::start().await;

    for level in ["info", "warn", "error"] {
        let refused = daemon
            .put_json("/v1/log/level", &json!({ "level": level, "for": "30m" }))
            .await;
        assert_eq!(refused.status, 400, "{level}");
        assert_eq!(code_of(&refused), "log_level_invalid", "{level}");
    }
}
